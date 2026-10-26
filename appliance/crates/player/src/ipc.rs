//! mpv's JSON IPC: watch `time-pos` and `duration`, report progress, note end of file.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Where playback stood when the player went away.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Final {
    pub position: f64,
    pub duration: f64,
    /// mpv reported it reached the end of the file.
    pub finished: bool,
}

/// Follow mpv until the socket closes. `on_progress(position, duration)` is called at most
/// once per `interval`. Returns the last known state.
pub fn monitor(
    stream: UnixStream,
    interval: Duration,
    mut on_progress: impl FnMut(f64, f64),
) -> std::io::Result<Final> {
    let mut writer = stream.try_clone()?;
    for (id, prop) in [(1, "time-pos"), (2, "duration")] {
        writeln!(
            writer,
            "{}",
            json!({ "command": ["observe_property", id, prop] })
        )?;
    }
    let mut last = Final::default();
    let mut last_report: Option<Instant> = None;
    // `duration` arrives before the first `time-pos`; reporting then would save position 0
    // over a resume point.
    let mut have_position = false;
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        applog::debug!(%line, "mpv ipc");
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match msg.get("event").and_then(Value::as_str) {
            Some("property-change") => {
                let value = msg.get("data").and_then(Value::as_f64);
                match (msg.get("name").and_then(Value::as_str), value) {
                    (Some("time-pos"), Some(v)) => {
                        last.position = v;
                        have_position = true;
                    }
                    (Some("duration"), Some(v)) => last.duration = v,
                    _ => continue,
                }
                if last_report.is_none_or(|t| t.elapsed() >= interval)
                    && have_position
                    && last.duration > 0.0
                {
                    on_progress(last.position, last.duration);
                    last_report = Some(Instant::now());
                }
            }
            Some("end-file") => {
                applog::debug!(event = %msg, "mpv end-file");
                last.finished = msg.get("reason").and_then(Value::as_str) == Some("eof")
            }
            _ => {}
        }
    }
    Ok(last)
}
