use std::path::PathBuf;
use std::time::Duration;

use mediad::{AppState, router};
use scanner::ReleaseDb;
use store::Store;
use tracing_subscriber::EnvFilter;

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() {
    let filter = EnvFilter::try_from_env("MEDIAD_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    if env_or("MEDIAD_LOG_FORMAT", "json") == "json" {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }

    let media_root = PathBuf::from(env_or("MEDIAD_MEDIA_ROOT", "/media"));
    let db_path = PathBuf::from(env_or("MEDIAD_DB", "/persist/mediad/mediad.sqlite"));
    let bind = env_or("MEDIAD_BIND", "127.0.0.1:8730");
    let rescan_secs: u64 = match env_or("MEDIAD_RESCAN_SECS", "900").parse() {
        Ok(n) => n,
        Err(e) => {
            tracing::error!(error = %e, "MEDIAD_RESCAN_SECS must be a number of seconds");
            std::process::exit(2);
        }
    };
    let release_db = std::env::var("MEDIAD_RELEASE_DB").map_or_else(
        |_| ReleaseDb::empty(),
        |d| ReleaseDb::load_dir(&PathBuf::from(d)),
    );

    if let Some(dir) = db_path.parent()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        tracing::error!(dir = %dir.display(), error = %e, "cannot create database directory");
        std::process::exit(1);
    }
    let store = match Store::open(&db_path) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(db = %db_path.display(), error = %e, "cannot open database");
            std::process::exit(1);
        }
    };
    tracing::info!(media_root = %media_root.display(), db = %db_path.display(), %bind, rescan_secs, "mediad starting");

    let mut state = AppState::new(store, media_root, release_db);
    if let Ok(dir) = std::env::var("MEDIAD_ART_CACHE")
        && !dir.is_empty()
    {
        state = state.with_art_cache(PathBuf::from(dir));
    }

    if rescan_secs > 0 {
        let state = state.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(rescan_secs));
            loop {
                tick.tick().await; // first tick is immediate: scan at startup
                let s = state.clone();
                match tokio::task::spawn_blocking(move || s.rescan()).await {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => {
                        tracing::error!(error = %e, "rescan failed; keeping the existing catalog")
                    }
                    Err(e) => tracing::error!(error = %e, "rescan task panicked"),
                }
            }
        });
    }

    let listener = match tokio::net::TcpListener::bind(&bind).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(%bind, error = %e, "cannot bind");
            std::process::exit(1);
        }
    };
    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("shutting down");
    };
    if let Err(e) = axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown)
        .await
    {
        tracing::error!(error = %e, "server error");
        std::process::exit(1);
    }
}
