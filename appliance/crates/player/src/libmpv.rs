//! In-process libmpv playback.
//!
//! The player used to exec an external `mpv` binary and talk to it over a JSON-IPC Unix
//! socket. Now it links libmpv directly (`mpv` crate, which `#[link(name = "mpv")]`s
//! `libmpv.so` from nixpkgs' `mpv-unwrapped`) and drives the same engine in-process.
//!
//! libmpv serves the JSON-IPC control socket itself (`--input-ipc-server`), so the
//! existing remote-key readers (`keys::watch`) and the `test-player.nix` harness that
//! talk over `/tmp/play-*.sock` keep working unchanged. We build the handler from the
//! same static options `mpv_args` produces (so the VM-test overrides still apply), load
//! the file, and pump events to track progress and end-of-file.

use std::path::Path;
use std::time::{Duration, Instant};

use mpv::{EndFileReason, Event, Format, MpvHandlerBuilder};

use crate::{Config, Play, PlayError, ipc};

// The `mpv` crate (0.2.3) wraps the C API but does not re-export `mpv_request_log_messages`;
// declare it here so we can ask libmpv to deliver its diagnostics as LogMessage events.
#[link(name = "mpv")]
unsafe extern "C" {
    fn mpv_request_log_messages(ctx: *mut std::ffi::c_void, min_level: *const std::os::raw::c_char) -> std::os::raw::c_int;
}

/// Follow playback in-process and return where it stood when it shut down. `on_progress`
/// is called at most once per `interval` once a real position is known (the same rule the
/// old `ipc::monitor` applied over the socket).
pub fn run(
    cfg: &Config,
    play: &Play,
    socket: &Path,
    interval: Duration,
    mut on_progress: impl FnMut(f64, f64),
) -> Result<ipc::Final, PlayError> {
    // libmpv parses option values with the C locale; without this, `--start=125.5` and
    // friends would be mis-parsed on a locale whose decimal separator is not '.'.
    // This is single-threaded at this point; safe here.
    unsafe { std::env::set_var("LC_NUMERIC", "C") };

    let args = crate::mpv_args(cfg, play, socket);
    let mut builder = MpvHandlerBuilder::new()
        .map_err(|e| PlayError::Launch(format!("cannot initialise libmpv: {e}")))?;
    // Route mpv's own log to a file beside the socket so failures are diagnosable
    // even when libmpv never delivers a LogMessage event.
    let mpv_log = socket.with_file_name(format!("{}.mpv.log", socket.file_name().unwrap_or_default().to_string_lossy()));
    if let Err(e) = builder.set_option("log-file", mpv_log.display().to_string().as_str()) {
        tracing::debug!(error = %e, "libmpv cannot enable its log file");
    }
    for arg in &args {
        if *arg == "--" || !arg.starts_with("--") {
            continue; // the end-of-options marker and the trailing file path
        }
        let body = &arg[2..];
        // --no-<opt> is a flag turned off; --<opt>=<val> a string option; --<opt> a flag on.
        let set = match body.strip_prefix("no-") {
            Some(opt) => builder.set_option(opt, false),
            None => match body.split_once('=') {
                Some((opt, val)) => builder.set_option(opt, val),
                None => builder.set_option(body, true),
            },
        };
        if let Err(err) = set {
            // A stray or already-overridden option (e.g. `--vo=null` after `--vo=gpu`) is
            // harmless; surface it in a debug build rather than abort the whole playback.
            tracing::debug!(option = %body, error = %err, "libmpv ignored an option");
        }
    }
    // In-process we manage the player's lifetime: mpv must NOT auto-quit the moment it is
    // idle (e.g. before the file is loaded, or after EOF), or a slow-starting libmpv in a
    // busy VM will shut down before our `loadfile` is processed. We keep it alive
    // (`idle=yes`, overriding the `--idle=no` that `mpv_args` emits for the external
    // binary) and issue `quit` ourselves when the file reaches its end.
    if let Err(e) = builder.set_option("idle", true) {
        tracing::debug!(error = %e, "libmpv cannot set idle=yes");
    }
    let mut handler = builder
        .build()
        .map_err(|e| PlayError::Launch(format!("cannot start libmpv: {e}")))?;

    applog::debug!(socket = %socket.display(), "libmpv built; its control socket is live");
    tracing::info!(socket = %socket.display(), "libmpv built; its control socket is live");
    // Remote buttons send JSON-IPC commands over the socket libmpv serves.
    crate::keys::watch(socket.to_path_buf());

    // Ask libmpv to surface its own log so load failures are diagnosable.
    unsafe {
        let min = std::ffi::CString::new("v").unwrap();
        mpv_request_log_messages(handler.raw().cast::<std::ffi::c_void>(), min.as_ptr());
    }

    handler
        .observe_property::<f64>("time-pos", 1)
        .map_err(|e| PlayError::Launch(format!("cannot observe time-pos: {e}")))?;
    handler
        .observe_property::<f64>("duration", 2)
        .map_err(|e| PlayError::Launch(format!("cannot observe duration: {e}")))?;

    if !play.path.exists() {
        tracing::warn!(path = %play.path.display(), "the file to play does not exist (NAS mount down?)");
    }
    tracing::info!(path = %play.path.display(), "loading file");
    handler
        .command(&["loadfile", play.path.to_str().unwrap_or_default(), "replace"])
        .map_err(|e| PlayError::Launch(format!("cannot load {}: {e}", play.path.display())))?;

    let mut fin = ipc::Final::default();
    let mut have_position = false;
    let mut last_report: Option<Instant> = None;
    let mut load_error: Option<String> = None;

    while load_error.is_none() {
        match handler.wait_event(0.05) {
            Some(Event::PropertyChange { name, change, .. }) => {
                match (name, change) {
                    ("time-pos", Format::Double(p)) => {
                        fin.position = p;
                        have_position = true;
                    }
                    ("duration", Format::Double(d)) => fin.duration = d,
                    _ => {}
                }
                if last_report.is_none_or(|t| t.elapsed() >= interval)
                    && have_position
                    && fin.duration > 0.0
                {
                    on_progress(fin.position, fin.duration);
                    last_report = Some(Instant::now());
                }
            }
            Some(Event::FileLoaded) => tracing::info!("file loaded"),
            Some(Event::LogMessage { prefix, level, text, .. }) => {
                // mpv's own diagnostics are the only way to see why a load failed; log
                // them at info so they survive into the journal even in a release build.
                tracing::info!(%prefix, %level, %text, "mpv");
            }
            Some(Event::EndFile(reason)) => match reason {
                // Reaching the end marks the item watched (finished => final_report uses
                // the duration as the position, exactly like the external-mpv path).
                Ok(EndFileReason::MPV_END_FILE_REASON_EOF) => {
                    tracing::info!("end of file reached");
                    fin.finished = true;
                    // idle=yes means mpv stays alive after the file ends; quit so the
                    // process can finish and report the watched state.
                    if let Err(e) = handler.command(&["quit"]) {
                        tracing::warn!(error = %e, "could not quit after end of file");
                    }
                }
                Ok(EndFileReason::MPV_END_FILE_REASON_ERROR) => {
                    load_error = Some("mpv reported an error at end of file".into())
                }
                Err(e) => load_error = Some(format!("mpv failed: {e}")),
                // STOP / QUIT / REDIRECT: the user (or remote) stopped it early, so the
                // resume point is preserved (finished stays false).
                _ => {}
            },
            Some(Event::Shutdown) => {
                tracing::info!("libmpv shutting down");
                break;
            }
            _ => {}
        }
    }
    // Dropping the handler tears the libmpv context down (mpv_terminate_destroy).
    drop(handler);
    let _ = std::fs::remove_file(socket);
    let _ = std::fs::remove_file(&mpv_log);

    match load_error {
        Some(e) => {
            tracing::error!(id = %play.id, path = %play.path.display(), error = %e, "playback failed");
            Err(PlayError::Launch(e))
        }
        None => Ok(fin),
    }
}