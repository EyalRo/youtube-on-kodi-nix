use std::path::PathBuf;
use std::process::ExitCode;

use tracing_subscriber::EnvFilter;
use ytsync::{Config, YtDlp, channels, notify_rescan, run};

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn env_num<T: std::str::FromStr>(name: &str, default: T) -> Result<T, String> {
    match std::env::var(name) {
        Ok(v) => v
            .parse()
            .map_err(|_| format!("{name} must be a number, got {v:?}")),
        Err(_) => Ok(default),
    }
}

fn config() -> Result<Config, String> {
    Ok(Config {
        media_root: env_or("YTSYNC_MEDIA_ROOT", "/media").into(),
        state_dir: env_or("YTSYNC_STATE", "/persist/ytsync").into(),
        ytdlp: env_or("YTSYNC_YTDLP", "yt-dlp").into(),
        max_per_channel: env_num("YTSYNC_MAX_PER_CHANNEL", 5)?,
        max_height: env_num("YTSYNC_MAX_HEIGHT", 1080)?,
        max_age_days: env_num("YTSYNC_MAX_AGE_DAYS", 30)?,
        max_channels: env_num("YTSYNC_MAX_CHANNELS", 0)?,
        mediad: Some(env_or("YTSYNC_MEDIAD", "127.0.0.1:8730")).filter(|a| !a.is_empty()),
    })
}

/// `ytsync follow <entry> [name]`, `ytsync unfollow <entry-or-name>` and `ytsync list` edit the
/// channels file in place (so its owner and mode stay as they are); no arguments run the sync.
fn manage(list: &std::path::Path, args: &[String]) -> Option<ExitCode> {
    let cmd = args.first()?.as_str();
    let text = std::fs::read_to_string(list).unwrap_or_default();
    let save = |new: String| match std::fs::write(list, new) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cannot write {}: {e}", list.display());
            ExitCode::from(2)
        }
    };
    Some(match (cmd, args.len()) {
        ("list", 1) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        ("follow", 2 | 3) => match channels::follow(&text, &args[1], args.get(2).map(String::as_str)) {
            Ok(Some(new)) => {
                println!("following {}", args[1]);
                save(new)
            }
            Ok(None) => {
                println!("already following {}", args[1]);
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::from(2)
            }
        },
        ("unfollow", 2) => match channels::unfollow(&text, &args[1]) {
            Some(new) => {
                println!("unfollowed {}", args[1]);
                save(new)
            }
            None => {
                eprintln!("not following {:?}", args[1]);
                ExitCode::from(1)
            }
        },
        _ => {
            eprintln!("usage: ytsync [list | follow <@handle|url|UC-id> [name] | unfollow <entry|name>]");
            ExitCode::from(2)
        }
    })
}

fn main() -> ExitCode {
    let filter = EnvFilter::try_from_env("YTSYNC_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    if env_or("YTSYNC_LOG_FORMAT", "json") == "json" {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }
    let cfg = match config() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "bad configuration");
            return ExitCode::from(2);
        }
    };
    let list = PathBuf::from(env_or("YTSYNC_CHANNELS", "/persist/ytsync/channels.conf"));
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = manage(&list, &args) {
        return code;
    }
    let text = match std::fs::read_to_string(&list) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(file = %list.display(), error = %e, "cannot read the channels file");
            return ExitCode::from(2);
        }
    };
    let parsed = channels::parse(&text);
    for s in &parsed.skipped {
        tracing::warn!(entry = %s, "channel entry not understood (use @handle, a channel URL or a UC... id); skipped");
    }
    tracing::info!(
        channels = parsed.channels.len(),
        max_per_channel = cfg.max_per_channel,
        max_height = cfg.max_height,
        "ytsync starting"
    );
    let summary = run(&cfg, &parsed.channels, &YtDlp(&cfg));
    if summary.delivered > 0
        && let Some(addr) = &cfg.mediad
        && let Err(e) = notify_rescan(addr)
    {
        tracing::warn!(error = %e, "could not ask mediad to rescan; it will pick the videos up on its next timed scan");
    }
    if summary.failed_channels > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
