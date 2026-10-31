//! The menu for a running game: Back, Home or Menu (on a remote, or a pad's Home button) freezes
//! RetroArch and runs a menu program in front of it, which exits with `EXIT_GAME` to leave the
//! game and with anything else to resume.
//!
//! RetroArch itself ignores a remote's Back/Home/Menu keys, so like the video player the game
//! player reads the kernel's input devices.

use std::collections::HashSet;
use std::io::Read;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const EVENT_SIZE: usize = 24;
const EV_KEY: u16 = 1;

/// Exit status of the menu program that means "leave the game".
pub const EXIT_GAME: i32 = 3;

/// A press of one of these opens the menu: KEY_BACK, KEY_HOMEPAGE, BTN_MODE (a pad's Home button),
/// and the menu-type keys remotes send (KEY_MENU, KEY_SETUP, KEY_COMPOSE, KEY_OPTION, KEY_INFO,
/// KEY_CONTEXT_MENU).
pub fn opens_menu(code: u16) -> bool {
    matches!(code, 158 | 172 | 316 | 139 | 141 | 127 | 357 | 358 | 438)
}

/// Whether a raw `input_event` is a (first) press of a menu button.
fn is_menu_press(event: &[u8; EVENT_SIZE]) -> bool {
    let kind = u16::from_le_bytes([event[16], event[17]]);
    let code = u16::from_le_bytes([event[18], event[19]]);
    let value = i32::from_le_bytes([event[20], event[21], event[22], event[23]]);
    kind == EV_KEY && value == 1 && opens_menu(code)
}

fn signal(pid: u32, sig: libc::c_int) {
    // SAFETY: plain kill(2) on the child this process spawned.
    unsafe {
        libc::kill(pid as libc::pid_t, sig);
    }
}

/// Watch every input device (rescanning for ones that connect later) and, on a menu button, freeze
/// the game `pid`, run `menu` and then resume it, or (menu says exit) set `left` and end it.
pub fn watch(pid: u32, menu: Vec<String>, left: Arc<AtomicBool>) {
    let (tx, rx) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        let watched = Arc::new(Mutex::new(HashSet::<PathBuf>::new()));
        loop {
            if let Ok(dir) = std::fs::read_dir("/dev/input") {
                for path in dir.filter_map(Result::ok).map(|e| e.path()) {
                    let is_event = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("event"));
                    if !is_event || !watched.lock().unwrap().insert(path.clone()) {
                        continue;
                    }
                    let Ok(mut dev) = std::fs::File::open(&path) else {
                        watched.lock().unwrap().remove(&path);
                        continue;
                    };
                    let (tx, watched) = (tx.clone(), watched.clone());
                    std::thread::spawn(move || {
                        let mut buf = [0u8; EVENT_SIZE];
                        while dev.read_exact(&mut buf).is_ok() {
                            if is_menu_press(&buf) {
                                let _ = tx.send(());
                            }
                        }
                        watched.lock().unwrap().remove(&path);
                    });
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
    std::thread::spawn(move || {
        let Some((prog, args)) = menu.split_first() else { return };
        let mut closed = Instant::now() - Duration::from_secs(1);
        for () in &rx {
            // One press can arrive from several devices, and the press that closes the menu is also
            // read here: ignore anything just after a menu ended.
            if closed.elapsed() < Duration::from_millis(500) {
                continue;
            }
            tracing::info!("game menu opened");
            signal(pid, libc::SIGSTOP);
            let status = Command::new(prog).args(args).status();
            let leave = matches!(&status, Ok(s) if s.code() == Some(EXIT_GAME));
            if let Err(e) = &status {
                tracing::error!(error = %e, "cannot run the game menu");
            }
            if leave {
                tracing::info!("leaving the game from the menu");
                left.store(true, Ordering::SeqCst);
                signal(pid, libc::SIGTERM);
            }
            signal(pid, libc::SIGCONT);
            if leave {
                return;
            }
            closed = Instant::now();
            while rx.try_recv().is_ok() {}
        }
    });
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
    fn back_home_and_menu_open_it_game_keys_do_not() {
        for code in [158, 172, 316, 139, 438] {
            assert!(is_menu_press(&event(1, code, 1)), "{code}");
        }
        assert!(!is_menu_press(&event(1, 158, 0)), "a release");
        assert!(!is_menu_press(&event(1, 158, 2)), "a held key repeating");
        assert!(!is_menu_press(&event(1, 28, 1)), "enter belongs to the game");
        assert!(!is_menu_press(&event(1, 304, 1)), "BTN_SOUTH belongs to the game");
        assert!(!is_menu_press(&event(4, 158, 1)), "not a key event");
    }
}
