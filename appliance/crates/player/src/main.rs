use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use player::{Config, play_item};

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn main() -> ExitCode {
    applog::init("play", "PLAYER_LOG");
    let Some(item) = std::env::args().nth(1) else {
        eprintln!("usage: play <item id>   (ids come from mediad: GET /rows/everything)");
        return ExitCode::from(2);
    };
    let cfg = Config {
        mediad: env_or("PLAYER_MEDIAD", "127.0.0.1:8730"),
        mpv: env_or("PLAYER_MPV", "mpv").into(),
        retroarch: env_or("PLAYER_RETROARCH", "retroarch").into(),
        core_dir: PathBuf::from(env_or(
            "PLAYER_CORE_DIR",
            "/run/current-system/sw/lib/retroarch/cores",
        )),
        drm_connector: env_or("PLAYER_DRM_CONNECTOR", "HDMI-A-1"),
        audio_device: env_or("PLAYER_AUDIO_DEVICE", "alsa/hdmi:CARD=PCH,DEV=0"),
        mpv_extra: env_or("PLAYER_MPV_ARGS", "")
            .split_whitespace()
            .map(String::from)
            .collect(),
        progress_interval: Duration::from_secs(5),
    };
    tracing::info!(item = %item, debug_build = applog::DEBUG_BUILD, "player starting");
    applog::debug!(config = ?cfg, "player config");
    match play_item(&cfg, &item) {
        Ok(()) => {
            applog::flush();
            ExitCode::SUCCESS
        }
        Err(e) => {
            tracing::error!(error = %e, item = %item, "playback failed");
            applog::flush();
            ExitCode::from(1)
        }
    }
}
