//! Remote buttons while a video plays.
//!
//! mpv on the bare display has no terminal and no window system, so it hears nothing from the
//! keyboard. Instead the player reads the kernel's input devices (`/dev/input/event*`, which
//! also carry HID remotes and CEC-to-uinput bridges) and turns presses into mpv IPC commands.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

/// Size of `struct input_event` on 64-bit Linux: 16 bytes of timestamp, type, code, value.
const EVENT_SIZE: usize = 24;
const EV_KEY: u16 = 1;

/// The overlay script's name for a key code, if that key does anything during playback. Held
/// keys repeat only for the directions (seeking, moving through a list).
pub fn key_name(code: u16, repeat: bool) -> Option<&'static str> {
    match code {
        // back (BACK, ESC, BACKSPACE)
        1 | 14 | 158 if !repeat => Some("back"),
        // STOP, STOPCD, Q
        16 | 128 | 166 if !repeat => Some("stop"),
        // select (ENTER, KPENTER)
        28 | 96 if !repeat => Some("ok"),
        // SPACE, PLAYPAUSE, PLAY, PAUSE
        57 | 164 | 207 | 119 if !repeat => Some("playpause"),
        105 | 168 => Some("left"),  // LEFT, REWIND
        106 | 208 => Some("right"), // RIGHT, FASTFORWARD
        108 => Some("down"),
        103 => Some("up"),
        _ => None,
    }
}

/// The mpv IPC command that hands a key to the overlay script (`pausemenu.lua`).
pub fn command_for(code: u16, repeat: bool) -> Option<Value> {
    key_name(code, repeat)
        .map(|name| json!({ "command": ["script-message-to", "pausemenu", "key", name] }))
}

/// `(code, repeat)` of a key press (or auto-repeat) in a raw `input_event`; releases and other
/// event types are `None`.
pub fn decode(event: &[u8; EVENT_SIZE]) -> Option<(u16, bool)> {
    let kind = u16::from_le_bytes([event[16], event[17]]);
    let code = u16::from_le_bytes([event[18], event[19]]);
    let value = i32::from_le_bytes([event[20], event[21], event[22], event[23]]);
    (kind == EV_KEY && (value == 1 || value == 2)).then_some((code, value == 2))
}

fn send(socket: &PathBuf, command: &Value) {
    tracing::info!(socket = %socket.display(), command = %command, "sending key command over IPC");
    if let Ok(mut s) = UnixStream::connect(socket) {
        let _ = writeln!(s, "{command}");
        // mpv writes a reply for every command. If we drop the handle right after
        // writing, mpv's response write hits a closed socket and logs
        // "[e][ipc_N] Write error (Broken pipe)" for every seek/pause/quit. Give it
        // a moment to write its reply before we close, and swallow whatever comes
        // back. The reply is a single line; block briefly with a timeout so we
        // never hang the input thread if mpv has nothing to send.
        if s.set_read_timeout(Some(Duration::from_millis(50))).is_ok() {
            let mut reader = BufReader::new(s);
            let mut line = String::new();
            let _ = reader.read_line(&mut line);
        }
    }
}

/// Start one reader per input device; each lives until the player process exits.
pub fn watch(socket: PathBuf) {
    let Ok(dir) = std::fs::read_dir("/dev/input") else { return };
    for path in dir.filter_map(Result::ok).map(|e| e.path()) {
        let is_event = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("event"));
        if !is_event {
            continue;
        }
        let Ok(mut dev) = std::fs::File::open(&path) else {
            applog::debug!(device = %path.display(), "cannot open input device");
            continue;
        };
        let socket = socket.clone();
        applog::debug!(device = %path.display(), "watching input device");
        std::thread::spawn(move || {
            let mut buf = [0u8; EVENT_SIZE];
            while dev.read_exact(&mut buf).is_ok() {
                if let Some((code, repeat)) = decode(&buf) {
                    let cmd = command_for(code, repeat);
                    applog::debug!(code, repeat, command = ?cmd, "key during playback");
                    if let Some(cmd) = cmd {
                        send(&socket, &cmd);
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: u16, code: u16, value: i32) -> [u8; EVENT_SIZE] {
        let mut e = [0u8; EVENT_SIZE];
        e[16..18].copy_from_slice(&kind.to_le_bytes());
        e[18..20].copy_from_slice(&code.to_le_bytes());
        e[20..24].copy_from_slice(&value.to_le_bytes());
        e
    }

    #[test]
    fn presses_and_repeats_decode_releases_do_not() {
        assert_eq!(decode(&event(1, 1, 1)), Some((1, false)));
        assert_eq!(decode(&event(1, 106, 2)), Some((106, true)));
        assert_eq!(decode(&event(1, 1, 0)), None);
        assert_eq!(decode(&event(4, 4, 1)), None); // EV_MSC
    }

    #[test]
    fn keys_go_to_the_overlay_script() {
        let cmd = |c, r| command_for(c, r).map(|v| v.to_string());
        assert_eq!(
            cmd(1, false).as_deref(),
            Some(r#"{"command":["script-message-to","pausemenu","key","back"]}"#)
        );
        assert_eq!(cmd(1, true), None, "a held Esc must not act twice");
        assert_eq!(key_name(28, false), Some("ok"));
        assert_eq!(key_name(164, false), Some("playpause"));
        assert_eq!(key_name(16, false), Some("stop"));
        assert_eq!(key_name(106, true), Some("right"), "a held direction repeats");
        assert_eq!(key_name(30, false), None); // KEY_A
    }
}
