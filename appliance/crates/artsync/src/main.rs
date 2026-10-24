use std::path::PathBuf;
use std::process::ExitCode;

use artsync::{Config, list_items, notify_rescan, run};
use tracing_subscriber::EnvFilter;

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn main() -> ExitCode {
    let filter = EnvFilter::try_from_env("ARTSYNC_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    if env_or("ARTSYNC_LOG_FORMAT", "json") == "json" {
        tracing_subscriber::fmt().json().with_env_filter(filter).init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }
    let tmdb_key = std::env::var("ARTSYNC_TMDB_KEY_FILE")
        .ok()
        .filter(|p| !p.is_empty())
        .and_then(|p| match std::fs::read_to_string(&p) {
            Ok(k) => Some(k.trim().to_string()).filter(|k| !k.is_empty()),
            Err(e) => {
                tracing::warn!(file = %p, error = %e, "cannot read the TMDB key; movies and shows will be skipped");
                None
            }
        });
    let max_lookups = match env_or("ARTSYNC_MAX_LOOKUPS", "0").parse() {
        Ok(n) => n,
        Err(_) => {
            tracing::error!("ARTSYNC_MAX_LOOKUPS must be a number");
            return ExitCode::from(2);
        }
    };
    let cfg = Config {
        mediad: env_or("ARTSYNC_MEDIAD", "127.0.0.1:8730"),
        cache: PathBuf::from(env_or("ARTSYNC_CACHE", "/persist/mediad/art")),
        tmdb_key,
        max_lookups,
    };
    let items = match list_items(&cfg.mediad) {
        Ok(i) => i,
        Err(e) => {
            tracing::error!(error = %e, "cannot list the library");
            return ExitCode::from(1);
        }
    };
    tracing::info!(without_art = items.len(), tmdb = cfg.tmdb_key.is_some(), "artsync starting");
    let s = run(&cfg, &items);
    tracing::info!(
        wanted = s.wanted, fetched = s.fetched, no_artwork_found = s.missed,
        failed = s.failed,
        "artsync finished"
    );
    if s.fetched > 0
        && let Err(e) = notify_rescan(&cfg.mediad)
    {
        tracing::warn!(error = %e, "could not ask mediad to rescan; it will pick the artwork up on its next timed scan");
    }
    ExitCode::SUCCESS
}
