//! Playback orchestration: ask mediad what to play, run mpv (or RetroArch for games),
//! report progress back. The display is the child's for as long as it runs; the UI waits.

pub mod http;
pub mod ipc;
pub mod keys;
pub mod libmpv;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

/// What mediad says to play (`GET /items/{id}/play`).
#[derive(Debug, Clone, PartialEq)]
pub struct Play {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub path: PathBuf,
    pub resume_position: f64,
    pub system: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum PlayError {
    #[error(transparent)]
    Http(#[from] http::HttpError),
    #[error("mediad's play reply is missing {0}")]
    Reply(&'static str),
    #[error("{0}")]
    Launch(String),
}

impl Play {
    pub fn from_json(v: &Value) -> Result<Play, PlayError> {
        let s = |k: &'static str| {
            v.get(k)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or(PlayError::Reply(k))
        };
        Ok(Play {
            id: s("id")?,
            kind: s("kind")?,
            title: s("title")?,
            path: PathBuf::from(s("path")?),
            resume_position: v
                .get("resume_position")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            system: v.get("system").and_then(Value::as_str).map(str::to_string),
        })
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub mediad: String,
    pub mpv: PathBuf,
    pub retroarch: PathBuf,
    /// Directory holding `<core>_libretro.so` files.
    pub core_dir: PathBuf,
    /// The DRM connector and ALSA device of the one TV.
    pub drm_connector: String,
    pub audio_device: String,
    /// Extra mpv options, placed after the defaults so they win (VM tests use `--vo=null`).
    pub mpv_extra: Vec<String>,
    pub progress_interval: Duration,
}

/// The libretro core for a system, by file stem (matches the `<core>_libretro.so`
/// nixpkgs installs; see `kiosk.nix`). mupen64plus-next is the N64 core; nixpkgs
/// names its `.so` with underscores (`mupen64plus_next_libretro.so`), so the stem
/// uses `_` too.
pub fn core_for(system: &str) -> Option<&'static str> {
    Some(match system {
        "nes" => "nestopia",
        "snes" => "snes9x",
        "genesis" | "sms" | "gamegear" => "genesis_plus_gx",
        "gb" | "gbc" | "gba" => "mgba",
        "n64" => "mupen64plus_next",
        "neogeo" => "fbneo",
        _ => return None,
    })
}

/// The overlay script, written beside the IPC socket for mpv to load. Its file name is its script
/// name, which `keys.rs` addresses.
pub const PAUSE_MENU: &str = include_str!("pausemenu.lua");

pub fn pause_menu_path(socket: &Path) -> PathBuf {
    socket.with_file_name("pausemenu.lua")
}

/// mpv on the bare display: DRM output, VA-API decode, AC-3/E-AC-3 passthrough to the TV.
/// **Untested on the target hardware**: these values are what the NUC spike has to confirm.
pub fn mpv_args(cfg: &Config, play: &Play, socket: &Path) -> Vec<String> {
    let mut a: Vec<String> = [
        "--no-terminal",
        "--vo=gpu",
        "--gpu-context=drm",
        "--hwdec=vaapi",
        "--keep-open=no",
        "--idle=no",
        "--save-position-on-quit=no",
        "--audio-spdif=ac3,eac3",
        "--audio-channels=auto-safe",
        // Playback overlay: `pausemenu.lua` (see `PAUSE_MENU`) replaces mpv's OSC. The UI and mpv
        // are separate processes (D2), so the overlay must be mpv-rendered, never a Slint screen.
        // osd-level 1 keeps volume messages; the script shows the bar after a seek itself.
        "--osc=no",
        "--osd-level=1",
        "--osd-on-seek=no",
        "--osd-bar=no",
    ]
    .map(String::from)
    .to_vec();
    // mpv has no terminal here, so its own diagnosis goes to a log file (see `run_mpv`).
    a.push(format!("--msg-level=all={}", if applog::DEBUG_BUILD { "v" } else { "warn" }));
    // Spike (cage): PLAYER_WAYLAND=1 makes mpv a fullscreen Wayland client instead of taking the DRM master.
    if std::env::var_os("PLAYER_WAYLAND").is_some_and(|v| v == "1") {
        a.retain(|x| x != "--gpu-context=drm");
        a.push("--gpu-context=wayland".into());
        a.push("--fs".into());
    } else {
        a.push(format!("--drm-connector={}", cfg.drm_connector));
    }
    a.push(format!("--audio-device={}", cfg.audio_device));
    a.push(format!("--input-ipc-server={}", socket.display()));
    a.push(format!("--script={}", pause_menu_path(socket).display()));
    a.extend(cfg.mpv_extra.iter().cloned());
    if play.resume_position > 0.0 {
        a.push(format!("--start={}", play.resume_position));
    }
    a.push(format!("--force-media-title={}", play.title));
    a.push("--".into());
    a.push(play.path.display().to_string());
    a
}

/// RetroArch settings that have no command-line flag (it rejects `--video-driver`), passed
/// through `--appendconfig`. Bare DRM/KMS kiosk (no X11/Wayland/compositor, D2 handoff):
/// RetroArch's default GL/X11 driver finds no display server and aborts, so select the
/// "drm" driver, which renders on the DRM plane via GBM/EGL like mpv does.
const RETROARCH_OVERRIDES: &str =
    "video_driver = \"drm\"\nvideo_fullscreen = \"true\"\nui_companion_start_on_boot = \"false\"\ndesktop_menu_enable = \"false\"\n";

/// The overrides plus audio: the same ALSA device mpv plays through (the TV's HDMI). Without
/// them RetroArch picks its own default driver and device, and the game runs silent. The 48 kHz
/// rate matches the HDMI sink, and the 128 ms buffer (default 64) rides out frame-time hitches
/// that would otherwise underrun the ALSA ring and chop the sound.
/// `system` is the game's system id: on a digital-only system the left stick also acts as the D-pad (see below).
pub fn retroarch_overrides(cfg: &Config, system: Option<&str>) -> String {
    let device = cfg.audio_device.strip_prefix("alsa/").unwrap_or(&cfg.audio_device);
    // Spike (cage): PLAYER_WAYLAND=1 runs RetroArch as a Wayland client (GL) instead of the DRM driver.
    let base = if std::env::var_os("PLAYER_WAYLAND").is_some_and(|v| v == "1") {
        RETROARCH_OVERRIDES.replace("video_driver = \"drm\"", "video_driver = \"gl\"\ninput_driver = \"udev\"")
    } else {
        RETROARCH_OVERRIDES.to_string()
    };
    let mut out = format!("{base}audio_driver = \"alsa\"\naudio_device = \"{device}\"\naudio_enable = \"true\"\naudio_mute_enable = \"false\"\naudio_out_rate = \"48000\"\naudio_latency = \"128\"\naudio_sync = \"true\"\nvideo_vsync = \"true\"\n");
    // Some pads can put their D-pad on the left stick (the 8BitDo M30 does, by a button chord), where
    // RetroArch's profile expects a D-pad hat and ignores it. For digital-only systems let the left
    // stick act as the D-pad, so such a pad still works. Not for N64: there the stick is a real analog.
    if system != Some("n64") {
        for p in 1..=4 {
            out.push_str(&format!("input_player{p}_analog_dpad_mode = \"1\"\n"));
        }
    }
    out
}

pub fn retroarch_args(cfg: &Config, play: &Play, overrides: &Path) -> Result<Vec<String>, PlayError> {
    let system = play
        .system
        .as_deref()
        .ok_or_else(|| PlayError::Launch("the game has no system".into()))?;
    let core = core_for(system)
        .ok_or_else(|| PlayError::Launch(format!("no libretro core for system {system}")))?;
    Ok(vec![
        format!("--appendconfig={}", overrides.display()),
        "-L".into(),
        cfg.core_dir
            .join(format!("{core}_libretro.so"))
            .display()
            .to_string(),
        "--".into(),
        play.path.display().to_string(),
    ])
}

fn progress_body(position: f64, duration: f64) -> Value {
    json!({ "position": position, "duration": duration })
}

/// The final report: reaching the end counts as having played to the end.
pub fn final_report(f: ipc::Final) -> Option<(f64, f64)> {
    // No position yet (quit while loading): nothing to say, and 0 would erase a resume point.
    if f.duration <= 0.0 || (f.position <= 0.0 && !f.finished) {
        return None;
    }
    Some((if f.finished { f.duration } else { f.position }, f.duration))
}

fn run_mpv(cfg: &Config, play: &Play) -> Result<(), PlayError> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from);
    let socket = dir.join(format!("play-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&socket);
    let script = pause_menu_path(&socket);
    std::fs::write(&script, PAUSE_MENU)
        .map_err(|e| PlayError::Launch(format!("cannot write {}: {e}", script.display())))?;
    tracing::info!(id = %play.id, "libmpv playing in-process");
    let fin = libmpv::run(cfg, play, &socket, cfg.progress_interval, |pos, dur| {
        applog::debug!(id = %play.id, position = pos, duration = dur, "reporting progress");
        if let Err(e) = http::post_json(
            &cfg.mediad,
            &format!("/items/{}/progress", play.id),
            &progress_body(pos, dur),
        ) {
            tracing::warn!(error = %e, "could not save progress");
        }
    });
    let _ = std::fs::remove_file(&script);
    let fin = fin?;
    tracing::info!(id = %play.id, position = fin.position, duration = fin.duration, finished = fin.finished, "playback ended");
    // The UI test and dashboards key on the "mpv exited ... exit status: 0" line the
    // external-binary player emitted; keep the same contract now that libmpv runs in-process.
    tracing::info!(id = %play.id, status = "exit status: 0", "mpv exited");
    if let Some((pos, dur)) = final_report(fin) {
        http::post_json(
            &cfg.mediad,
            &format!("/items/{}/progress", play.id),
            &progress_body(pos, dur),
        )?;
    }
    Ok(())
}

/// How loud a RetroArch log line is: `[ERROR]`, `[WARN]`, or anything else (info/debug chatter).
/// Cores log as `[libretro ERROR]` and so on.
fn retroarch_level(line: &str) -> tracing::Level {
    let tag = line.trim_start().strip_prefix('[').and_then(|r| r.split_once(']')).map_or("", |(t, _)| t);
    match tag.rsplit(' ').next().unwrap_or("") {
        "ERROR" => tracing::Level::ERROR,
        "WARN" => tracing::Level::WARN,
        _ => tracing::Level::DEBUG,
    }
}

/// Log one RetroArch line. Errors and warnings in every build; the audio and video setup lines
/// (driver, device, rate, buffer) at info, since they are what explains choppy or silent games;
/// the rest only in a debug build.
fn log_retroarch_line(id: &str, line: &str) {
    match retroarch_level(line) {
        tracing::Level::ERROR => tracing::error!(%id, line, "retroarch"),
        tracing::Level::WARN => tracing::warn!(%id, line, "retroarch"),
        _ if line.contains("[Audio]") || line.contains("[Video]") || line.contains("ALSA") => {
            tracing::info!(%id, line, "retroarch")
        }
        _ => {
            applog::debug!(%id, line, "retroarch");
        }
    }
}

/// Follow RetroArch's log file while the game runs, so its lines reach the log live (and survive a
/// crash or hang). Stops, after a last drain, once `stop` is set.
fn follow_log_file(path: PathBuf, id: String, stop: std::sync::Arc<std::sync::atomic::AtomicBool>) -> std::thread::JoinHandle<()> {
    use std::io::BufRead;
    use std::sync::atomic::Ordering;
    std::thread::spawn(move || {
        let mut reader = None;
        loop {
            let stopping = stop.load(Ordering::Relaxed);
            if reader.is_none() {
                reader = std::fs::File::open(&path).ok().map(std::io::BufReader::new);
            }
            if let Some(r) = reader.as_mut() {
                let mut buf = String::new();
                while r.read_line(&mut buf).map_or(false, |n| n > 0) {
                    if buf.ends_with('\n') {
                        let line = buf.trim_end();
                        if !line.is_empty() {
                            log_retroarch_line(&id, line);
                        }
                        buf.clear();
                    }
                }
            }
            if stopping {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    })
}

/// Pump a RetroArch output stream into the log, a line at a time. Errors and warnings are kept in
/// every build; the rest only exists in a debug build (where `--verbose` produces it).
fn forward_log(stream: impl std::io::Read + Send + 'static, id: &str) -> std::thread::JoinHandle<()> {
    use std::io::BufRead;
    let id = id.to_string();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stream).lines().map_while(Result::ok) {
            let line = line.trim_end();
            if line.is_empty() {
                continue;
            }
            log_retroarch_line(&id, line);
        }
    })
}

fn run_game(cfg: &Config, play: &Play) -> Result<(), PlayError> {
    let overrides = std::env::temp_dir().join(format!("retroarch-overrides-{}.cfg", std::process::id()));
    std::fs::write(&overrides, retroarch_overrides(cfg, play.system.as_deref())).map_err(|e| {
        tracing::error!(path = %overrides.display(), error = %e, "cannot write retroarch overrides");
        PlayError::Launch(format!("cannot write {}: {e}", overrides.display()))
    })?;
    let mut args = retroarch_args(cfg, play, &overrides)?;
    // Its own log (driver choice, buffer sizes, underruns) is not on stderr; followed live below.
    // These flags must come before `--`, or RetroArch takes them for content paths.
    let log_file = std::env::temp_dir().join(format!("retroarch-{}.log", std::process::id()));
    let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    args.splice(end..end, ["--verbose".to_string(), "--log-file".to_string(), log_file.display().to_string()]);
    http::post_json(
        &cfg.mediad,
        &format!("/items/{}/played", play.id),
        &json!({}),
    )?;
    applog::debug!(retroarch = %cfg.retroarch.display(), args = ?args, "starting retroarch");
    let _ = std::fs::remove_file(&log_file);
    let mut child = Command::new(&cfg.retroarch)
        // nixpkgs builds RetroArch with its Qt desktop companion, which aborts with no X11/Wayland;
        // the offscreen platform lets it initialise without a display (the game uses the drm driver).
        .env("QT_QPA_PLATFORM", "offscreen")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| PlayError::Launch(format!("cannot start {}: {e}", cfg.retroarch.display())))?;
    // RetroArch logs its audio/video driver choice and any device or core error to stderr;
    // forward every line so it reaches the journal and VictoriaLogs instead of vanishing.
    let pumps: Vec<_> = [
        child.stdout.take().map(|s| forward_log(s, &play.id)),
        child.stderr.take().map(|s| forward_log(s, &play.id)),
    ]
    .into_iter()
    .flatten()
    .collect();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let follower = follow_log_file(log_file.clone(), play.id.clone(), stop.clone());
    let status = child
        .wait()
        .map_err(|e| PlayError::Launch(format!("waiting for retroarch: {e}")))?;
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = follower.join();
    for p in pumps {
        let _ = p.join();
    }
    let _ = std::fs::remove_file(&overrides);
    let _ = std::fs::remove_file(&log_file);
    tracing::info!(id = %play.id, %status, "retroarch exited");
    if !status.success() {
        tracing::error!(id = %play.id, path = %play.path.display(), %status, "retroarch failed");
    }
    if status.success() {
        Ok(())
    } else {
        Err(PlayError::Launch(format!("retroarch exited with {status}")))
    }
}

/// Play an item (movie, game, episode, or a show/channel that resolves to an episode).
pub fn play_item(cfg: &Config, item_id: &str) -> Result<(), PlayError> {
    tracing::info!(item = %item_id, "play requested");
    let reply = http::get_json(&cfg.mediad, &format!("/items/{item_id}/play"))?;
    applog::debug!(reply = %reply, "mediad's play reply");
    let play = Play::from_json(&reply)?;
    tracing::info!(id = %play.id, kind = %play.kind, title = %play.title, resume = play.resume_position, "starting playback");
    if play.kind == "game" {
        run_game(cfg, &play)
    } else {
        run_mpv(cfg, &play)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::os::unix::net::UnixListener;

    fn cfg() -> Config {
        Config {
            mediad: "127.0.0.1:1".into(),
            mpv: "mpv".into(),
            retroarch: "retroarch".into(),
            core_dir: "/cores".into(),
            drm_connector: "HDMI-A-1".into(),
            audio_device: "alsa/hdmi:CARD=PCH,DEV=0".into(),
            mpv_extra: vec![],
            progress_interval: Duration::from_millis(0),
        }
    }

    fn play(kind: &str, resume: f64) -> Play {
        Play {
            id: "m-1".into(),
            kind: kind.into(),
            title: "Alien".into(),
            path: "/media/movies/Alien.mkv".into(),
            resume_position: resume,
            system: None,
        }
    }

    #[test]
    fn mpv_args_target_the_tv_and_resume() {
        let a = mpv_args(&cfg(), &play("movie", 125.5), Path::new("/run/p.sock")).join(" ");
        assert!(
            a.contains("--drm-connector=HDMI-A-1")
                && a.contains("--audio-spdif=ac3,eac3")
                && a.contains("--hwdec=vaapi")
        );
        assert!(a.contains("--start=125.5") && a.contains("--input-ipc-server=/run/p.sock"));
        assert!(
            a.contains("--osc=no") && a.contains("--script=/run/pausemenu.lua"),
            "the pause overlay script replaces the OSC"
        );
        assert!(a.ends_with("-- /media/movies/Alien.mkv"));
        assert!(
            !mpv_args(&cfg(), &play("movie", 0.0), Path::new("/s"))
                .join(" ")
                .contains("--start")
        );
    }

    #[test]
    fn mpv_extra_args_come_after_the_defaults() {
        let mut c = cfg();
        c.mpv_extra = vec!["--vo=null".into(), "--pause".into()];
        let a = mpv_args(&c, &play("movie", 0.0), Path::new("/s"));
        let pos = |s: &str| a.iter().position(|x| x == s).unwrap();
        assert!(pos("--vo=null") > pos("--vo=gpu") && pos("--pause") < pos("--"));
    }

    #[test]
    fn retroarch_plays_through_the_tv_audio_device() {
        let o = retroarch_overrides(&cfg(), Some("genesis"));
        assert!(o.contains("audio_driver = \"alsa\"") && o.contains("audio_device = \"hdmi:CARD=PCH,DEV=0\""), "{o}");
    }

    #[test]
    fn a_pad_with_its_dpad_on_the_left_stick_still_works_on_digital_systems() {
        for system in ["genesis", "snes", "neogeo", "nes"] {
            let o = retroarch_overrides(&cfg(), Some(system));
            for p in 1..=4 {
                assert!(o.contains(&format!("input_player{p}_analog_dpad_mode = \"1\"")), "{system}: {o}");
            }
        }
    }

    #[test]
    fn n64_keeps_the_stick_as_an_analog_stick() {
        let o = retroarch_overrides(&cfg(), Some("n64"));
        assert!(!o.contains("analog_dpad_mode"), "{o}");
    }

    #[test]
    fn retroarch_log_lines_are_leveled() {
        use tracing::Level;
        assert_eq!(retroarch_level("[ERROR] [Audio]: Failed to open device"), Level::ERROR);
        assert_eq!(retroarch_level("[libretro ERROR] bad rom"), Level::ERROR);
        assert_eq!(retroarch_level("[WARN] [Audio]: Underrun"), Level::WARN);
        assert_eq!(retroarch_level("[INFO] [Audio]: Using alsa"), Level::DEBUG);
        assert_eq!(retroarch_level("plain text"), Level::DEBUG);
    }

    #[test]
    fn retroarch_args_pick_the_core() {
        let mut p = play("game", 0.0);
        p.system = Some("genesis".into());
        assert_eq!(
            retroarch_args(&cfg(), &p, Path::new("/o.cfg")).unwrap(),
            [
                "--appendconfig=/o.cfg",
                "-L",
                "/cores/genesis_plus_gx_libretro.so",
                "--",
                "/media/movies/Alien.mkv"
            ]
        );
        p.system = Some("n64".into());
        assert_eq!(
            retroarch_args(&cfg(), &p, Path::new("/o.cfg")).unwrap(),
            [
                "--appendconfig=/o.cfg",
                "-L",
                "/cores/mupen64plus_next_libretro.so",
                "--",
                "/media/movies/Alien.mkv"
            ]
        );
        p.system = None;
        assert!(retroarch_args(&cfg(), &p, Path::new("/o.cfg")).is_err());
    }

    #[test]
    fn play_parses_and_rejects_incomplete_replies() {
        let v = json!({"id":"e-1","kind":"episode","title":"T","path":"/m/x.mkv","resume_position":30.0,"system":null});
        let p = Play::from_json(&v).unwrap();
        assert_eq!(
            (p.id.as_str(), p.resume_position, p.system),
            ("e-1", 30.0, None)
        );
        assert!(matches!(
            Play::from_json(&json!({"id":"x"})),
            Err(PlayError::Reply("kind"))
        ));
    }

    #[test]
    fn final_report_rules() {
        assert_eq!(
            final_report(ipc::Final {
                position: 50.0,
                duration: 100.0,
                finished: false
            }),
            Some((50.0, 100.0))
        );
        assert_eq!(
            final_report(ipc::Final {
                position: 95.0,
                duration: 100.0,
                finished: true
            }),
            Some((100.0, 100.0))
        );
        assert_eq!(final_report(ipc::Final::default()), None);
        assert_eq!(
            final_report(ipc::Final {
                position: 0.0,
                duration: 100.0,
                finished: false
            }),
            None,
            "quit while loading"
        );
    }

    #[test]
    fn monitor_follows_a_fake_mpv() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("mpv.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let fake = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(s.try_clone().unwrap());
            let (mut a, mut b) = (String::new(), String::new());
            reader.read_line(&mut a).unwrap();
            reader.read_line(&mut b).unwrap();
            for l in [
                r#"{"event":"property-change","id":2,"name":"duration","data":1000.0}"#,
                r#"{"event":"property-change","id":1,"name":"time-pos","data":10.5}"#,
                r#"{"event":"property-change","id":1,"name":"time-pos"}"#,
                r#"{"event":"property-change","id":1,"name":"time-pos","data":42.0}"#,
                r#"not json"#,
                r#"{"event":"end-file","reason":"quit"}"#,
            ] {
                writeln!(s, "{l}").unwrap();
            }
            (a, b)
        });
        let mut seen = Vec::new();
        let fin = ipc::monitor(
            UnixStream::connect(&sock).unwrap(),
            Duration::from_millis(0),
            |p, d| seen.push((p, d)),
        )
        .unwrap();
        let (a, b) = fake.join().unwrap();
        assert!(a.contains("observe_property") && a.contains("time-pos") && b.contains("duration"));
        assert_eq!(
            fin,
            ipc::Final {
                position: 42.0,
                duration: 1000.0,
                finished: false
            }
        );
        assert_eq!(seen, [(10.5, 1000.0), (42.0, 1000.0)]);
    }

    use std::os::unix::net::UnixStream;

    #[test]
    fn monitor_throttles_progress_and_notes_eof() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("mpv.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let fake = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(50));
            for l in [
                r#"{"event":"property-change","name":"duration","data":100.0}"#,
                r#"{"event":"property-change","name":"time-pos","data":1.0}"#,
                r#"{"event":"property-change","name":"time-pos","data":2.0}"#,
                r#"{"event":"property-change","name":"time-pos","data":99.0}"#,
                r#"{"event":"end-file","reason":"eof"}"#,
            ] {
                writeln!(s, "{l}").unwrap();
            }
        });
        let mut seen = 0;
        let fin = ipc::monitor(
            UnixStream::connect(&sock).unwrap(),
            Duration::from_secs(3600),
            |_, _| seen += 1,
        )
        .unwrap();
        fake.join().unwrap();
        assert_eq!(seen, 1, "one report per interval");
        assert!(fin.finished && fin.position == 99.0);
    }

    #[test]
    fn http_roundtrip_with_a_fake_mediad() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let t = std::thread::spawn(move || {
            let mut reqs = Vec::new();
            for reply in [
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\r\n{\"id\":\"m-1\",\"kind\":\"movie\",\"title\":\"A\",\"path\":\"/x.mkv\",\"resume_position\":0.0,\"system\":null}",
                "HTTP/1.1 204 No Content\r\n\r\n",
                "HTTP/1.1 404 Not Found\r\n\r\n{\"error\":\"no such item\"}",
            ] {
                let (mut c, _) = l.accept().unwrap();
                let mut got = Vec::new();
                let mut buf = [0u8; 1024];
                loop {
                    let n = c.read(&mut buf).unwrap();
                    got.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&got).into_owned();
                    if let Some((head, body)) = text.split_once("\r\n\r\n") {
                        let want = head
                            .lines()
                            .find_map(|l| l.strip_prefix("Content-Length: "))
                            .and_then(|v| v.parse::<usize>().ok())
                            .unwrap_or(0);
                        if body.len() >= want {
                            break;
                        }
                    }
                }
                reqs.push(String::from_utf8_lossy(&got).into_owned());
                c.write_all(reply.as_bytes()).unwrap();
            }
            reqs
        });
        let v = http::get_json(&addr, "/items/m-1/play").unwrap();
        assert_eq!(Play::from_json(&v).unwrap().path, PathBuf::from("/x.mkv"));
        http::post_json(&addr, "/items/m-1/progress", &progress_body(5.0, 10.0)).unwrap();
        assert!(matches!(
            http::get_json(&addr, "/items/zz/play"),
            Err(http::HttpError::Status { status: 404, .. })
        ));
        let reqs = t.join().unwrap();
        assert!(reqs[0].starts_with("GET /items/m-1/play HTTP/1.0"));
        assert!(
            reqs[1].starts_with("POST /items/m-1/progress")
                && reqs[1].ends_with(r#"{"duration":10.0,"position":5.0}"#)
        );
        assert!(matches!(
            http::get_json("127.0.0.1:1", "/"),
            Err(http::HttpError::Connect { .. })
        ));
    }
}
