//! Playback orchestration: ask mediad what to play, run mpv (or RetroArch for games),
//! report progress back. The display is the child's for as long as it runs; the UI waits.

pub mod http;
pub mod ipc;
pub mod keys;

use std::path::{Path, PathBuf};
use std::process::Command;
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

/// The libretro core for a system, by file stem. N64 is deliberately absent (post-MVP).
pub fn core_for(system: &str) -> Option<&'static str> {
    Some(match system {
        "nes" => "nestopia",
        "snes" => "snes9x",
        "genesis" | "sms" | "gamegear" => "genesis_plus_gx",
        "gb" | "gbc" | "gba" => "mgba",
        _ => return None,
    })
}

/// mpv on the bare display: DRM output, VA-API decode, AC-3/E-AC-3 passthrough to the TV.
/// **Untested on the target hardware**: these values are what the NUC spike has to confirm.
pub fn mpv_args(cfg: &Config, play: &Play, socket: &Path) -> Vec<String> {
    let mut a: Vec<String> = [
        "--no-terminal",
        "--really-quiet",
        "--fullscreen",
        "--vo=gpu",
        "--gpu-context=drm",
        "--hwdec=vaapi",
        "--keep-open=no",
        "--idle=no",
        "--save-position-on-quit=no",
        "--audio-spdif=ac3,eac3",
        "--audio-channels=auto-safe",
    ]
    .map(String::from)
    .to_vec();
    a.push(format!("--drm-connector={}", cfg.drm_connector));
    a.push(format!("--audio-device={}", cfg.audio_device));
    a.push(format!("--input-ipc-server={}", socket.display()));
    a.extend(cfg.mpv_extra.iter().cloned());
    if play.resume_position > 0.0 {
        a.push(format!("--start={}", play.resume_position));
    }
    a.push(format!("--force-media-title={}", play.title));
    a.push("--".into());
    a.push(play.path.display().to_string());
    a
}

pub fn retroarch_args(cfg: &Config, play: &Play) -> Result<Vec<String>, PlayError> {
    let system = play
        .system
        .as_deref()
        .ok_or_else(|| PlayError::Launch("the game has no system".into()))?;
    let core = core_for(system)
        .ok_or_else(|| PlayError::Launch(format!("no libretro core for system {system}")))?;
    Ok(vec![
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
    let mut child = Command::new(&cfg.mpv)
        .args(mpv_args(cfg, play, &socket))
        .spawn()
        .map_err(|e| PlayError::Launch(format!("cannot start {}: {e}", cfg.mpv.display())))?;
    keys::watch(socket.clone());
    // mpv creates the socket a moment after starting.
    let mut stream = None;
    for _ in 0..100 {
        if let Ok(s) = std::os::unix::net::UnixStream::connect(&socket) {
            stream = Some(s);
            break;
        }
        if child.try_wait().ok().flatten().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let fin = match stream {
        Some(s) => ipc::monitor(s, cfg.progress_interval, |pos, dur| {
            if let Err(e) = http::post_json(
                &cfg.mediad,
                &format!("/items/{}/progress", play.id),
                &progress_body(pos, dur),
            ) {
                tracing::warn!(error = %e, "could not save progress");
            }
        })
        .unwrap_or_default(),
        None => {
            tracing::error!("mpv never opened its control socket; progress will not be saved");
            ipc::Final::default()
        }
    };
    let status = child.wait().map_err(|e| PlayError::Launch(e.to_string()))?;
    let _ = std::fs::remove_file(&socket);
    tracing::info!(id = %play.id, %status, position = fin.position, duration = fin.duration, finished = fin.finished, "mpv exited");
    if let Some((pos, dur)) = final_report(fin) {
        http::post_json(
            &cfg.mediad,
            &format!("/items/{}/progress", play.id),
            &progress_body(pos, dur),
        )?;
    }
    if status.success() {
        Ok(())
    } else {
        Err(PlayError::Launch(format!("mpv exited with {status}")))
    }
}

fn run_game(cfg: &Config, play: &Play) -> Result<(), PlayError> {
    let args = retroarch_args(cfg, play)?;
    http::post_json(
        &cfg.mediad,
        &format!("/items/{}/played", play.id),
        &json!({}),
    )?;
    let status = Command::new(&cfg.retroarch)
        .args(args)
        .status()
        .map_err(|e| PlayError::Launch(format!("cannot start {}: {e}", cfg.retroarch.display())))?;
    tracing::info!(id = %play.id, %status, "retroarch exited");
    if status.success() {
        Ok(())
    } else {
        Err(PlayError::Launch(format!("retroarch exited with {status}")))
    }
}

/// Play an item (movie, game, episode, or a show/channel that resolves to an episode).
pub fn play_item(cfg: &Config, item_id: &str) -> Result<(), PlayError> {
    let reply = http::get_json(&cfg.mediad, &format!("/items/{item_id}/play"))?;
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
    fn retroarch_args_pick_the_core() {
        let mut p = play("game", 0.0);
        p.system = Some("genesis".into());
        assert_eq!(
            retroarch_args(&cfg(), &p).unwrap(),
            [
                "-L",
                "/cores/genesis_plus_gx_libretro.so",
                "--",
                "/media/movies/Alien.mkv"
            ]
        );
        p.system = Some("n64".into());
        assert!(retroarch_args(&cfg(), &p).is_err());
        p.system = None;
        assert!(retroarch_args(&cfg(), &p).is_err());
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
