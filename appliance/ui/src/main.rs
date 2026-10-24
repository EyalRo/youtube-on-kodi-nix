//! Spike: poster rows from mediad, drawn by Slint straight to DRM/KMS, driven by key events.
//!
//! Choosing an item hands the display to `play`. Two ways, selected by `UI_HANDOFF`:
//! `exit` (default) saves the focus, writes the item id to `$UI_STATE/launch` and exits 10 so a
//! supervisor (`session.sh`) runs `play` and restarts the UI; `inproc` hides the window and runs
//! `play` from this process, to find out if Slint lets go of DRM master on `hide()`.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::Value;
use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};

slint::include_modules!();

const MAX_TILES: usize = 40;
/// Exit status that asks the supervisor to run `play` (id in `$UI_STATE/launch`).
const EXIT_LAUNCH: i32 = 10;

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

/// A bare HTTP/1.0 GET: mediad is on localhost and replies with a Content-Length.
fn get(addr: &str, path: &str) -> Result<(u16, String, Vec<u8>), String> {
    let mut s = TcpStream::connect(addr).map_err(|e| format!("cannot reach mediad at {addr}: {e}"))?;
    s.set_read_timeout(Some(Duration::from_secs(10))).ok();
    write!(s, "GET {path} HTTP/1.0\r\nHost: {addr}\r\n\r\n").map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).map_err(|e| e.to_string())?;
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("malformed reply")?;
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or("malformed status line")?;
    let ctype = head
        .lines()
        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-type:").map(|v| v.trim().to_string()))
        .unwrap_or_default();
    Ok((status, ctype, raw[split + 4..].to_vec()))
}

fn get_json(addr: &str, path: &str) -> Result<Value, String> {
    let (status, _, body) = get(addr, path)?;
    if status != 200 {
        return Err(format!("{path}: HTTP {status}"));
    }
    serde_json::from_slice(&body).map_err(|e| format!("{path}: {e}"))
}

/// Fetch an item's artwork into the cache and decode it.
fn load_art(addr: &str, id: &str, cache: &Path) -> Option<Image> {
    load_image(addr, &format!("/items/{id}/art"), id, cache)
}

/// Fetch an image from mediad (`path`) into the cache under `key` and decode it.
fn load_image(addr: &str, path: &str, key: &str, cache: &Path) -> Option<Image> {
    let (status, ctype, body) = get(addr, path).ok()?;
    if status != 200 {
        return None;
    }
    // The bytes decide, not the header or the name: a channel avatar is a PNG called poster.jpg.
    let ext = if body.starts_with(b"\x89PNG") {
        "png"
    } else if body.starts_with(b"RIFF") {
        "webp"
    } else if ctype == "image/png" {
        "png"
    } else {
        "jpg"
    };
    let file = cache.join(format!("{key}.{ext}"));
    std::fs::write(&file, body).ok()?;
    Image::load_from_path(&file).ok()
}

/// One season tab and its episodes, as mediad lists them.
struct Season {
    label: String,
    episodes: Vec<Value>,
}

/// A show or channel page's data.
struct Detail {
    id: String,
    title: String,
    seasons: Vec<Season>,
}

/// Group `/items/{id}` episodes into seasons in the order mediad gives them: a show ascending,
/// a channel newest first (its "seasons" are upload years).
fn load_detail(addr: &str, id: &str) -> Result<Detail, String> {
    let v = get_json(addr, &format!("/items/{id}"))?;
    let channel = v["item"]["kind"].as_str() == Some("channel");
    let mut seasons: Vec<(i64, Vec<Value>)> = Vec::new();
    for ep in v["episodes"].as_array().ok_or("no episodes")? {
        let s = ep["season"].as_i64().unwrap_or(0);
        match seasons.iter_mut().find(|(n, _)| *n == s) {
            Some((_, list)) => list.push(ep.clone()),
            None => seasons.push((s, vec![ep.clone()])),
        }
    }
    Ok(Detail {
        id: id.to_string(),
        title: v["item"]["title"].as_str().unwrap_or_default().to_string(),
        seasons: seasons
            .into_iter()
            .map(|(n, episodes)| Season {
                label: match (channel, n) {
                    (true, 0) => "Undated".into(),
                    (true, y) => y.to_string(),
                    (false, 0) => "Specials".into(),
                    (false, n) => format!("Season {n}"),
                },
                episodes,
            })
            .collect(),
    })
}

/// The tiles of one season; thumbnails are fetched now, for just these episodes.
fn season_tiles(addr: &str, season: &Season, channel_view: bool, cache: &Path) -> Vec<EpisodeData> {
    let _ = std::fs::create_dir_all(cache);
    season
        .episodes
        .iter()
        .map(|e| {
            let id = e["id"].as_str().unwrap_or_default();
            let art = e["has_art"]
                .as_bool()
                .unwrap_or(false)
                .then(|| load_image(addr, &format!("/episodes/{id}/art"), id, cache))
                .flatten();
            let aired = e["aired"].as_str().unwrap_or_default();
            let number = e["episode"].as_i64().unwrap_or(0);
            let subtitle = if channel_view || number == 0 || number >= 100 {
                aired.to_string()
            } else if aired.is_empty() {
                format!("E{number}")
            } else {
                format!("E{number} · {aired}")
            };
            let (pos, dur) = (e["position"].as_f64().unwrap_or(0.0), e["duration"].as_f64().unwrap_or(0.0));
            EpisodeData {
                id: id.into(),
                title: e["title"].as_str().unwrap_or_default().into(),
                subtitle: subtitle.into(),
                has_art: art.is_some(),
                art: art.unwrap_or_default(),
                watched: e["watched"].as_bool().unwrap_or(false),
                progress: if dur > 0.0 { (pos / dur).clamp(0.0, 1.0) as f32 } else { 0.0 },
            }
        })
        .collect()
}

fn set_season(ui: &App, addr: &str, detail: &Detail, idx: usize, cache: &Path) {
    let Some(season) = detail.seasons.get(idx) else { return };
    // A channel's tabs are years (numeric labels); a show's say "Season".
    let channel_view = !season.label.starts_with("Season") && season.label != "Specials";
    ui.set_episodes(ModelRc::new(VecModel::from(season_tiles(addr, season, channel_view, cache))));
}

/// Open the page for a show or channel at the given season and episode.
fn open_detail(ui: &App, addr: &str, id: &str, season: usize, ep: usize, cache: &Path) -> Option<Detail> {
    let detail = match load_detail(addr, id) {
        Ok(d) if !d.seasons.is_empty() => d,
        Ok(_) => return None,
        Err(e) => {
            tracing::error!(%id, error = %e, "cannot open the page");
            return None;
        }
    };
    let season = season.min(detail.seasons.len() - 1);
    ui.set_detail_title(detail.title.as_str().into());
    ui.set_seasons(ModelRc::new(VecModel::from(
        detail.seasons.iter().map(|s| SharedString::from(s.label.as_str())).collect::<Vec<_>>(),
    )));
    ui.set_sel_season(season as i32);
    set_season(ui, addr, &detail, season, cache);
    let eps = ui.get_episodes().row_count();
    ui.set_sel_ep(ep.min(eps.saturating_sub(1)) as i32);
    ui.set_detail_focus(1.min(eps as i32));
    ui.set_detail(true);
    tracing::info!(%id, seasons = detail.seasons.len(), "page opened");
    Some(detail)
}

/// Hand the display to `play` for `id`: exit for the supervisor (default) or run it in-process.
fn launch(ui: &App, id: &str, inproc: bool, play: &str) {
    tracing::info!(%id, inproc, "handing the display to play");
    let dir = state_dir();
    let _ = std::fs::create_dir_all(&dir);
    if inproc {
        let _ = ui.hide();
        let status = Command::new(play).arg(id).status();
        tracing::info!(?status, "play returned");
        let _ = ui.show();
    } else {
        let _ = std::fs::write(dir.join("launch"), id);
        let _ = slint::quit_event_loop();
        LAUNCH.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

fn load_rows(addr: &str, cache: &Path) -> Result<Vec<RowData>, String> {
    let _ = std::fs::create_dir_all(cache);
    let listing = get_json(addr, "/rows")?;
    let mut out = Vec::new();
    for info in listing["rows"].as_array().ok_or("no rows")? {
        if info["count"].as_u64().unwrap_or(0) == 0 {
            continue;
        }
        let id = info["id"].as_str().unwrap_or_default();
        let row = get_json(addr, &format!("/rows/{id}"))?;
        let tiles: Vec<TileData> = row["items"]
            .as_array()
            .map(|a| a.as_slice())
            .unwrap_or_default()
            .iter()
            .take(MAX_TILES)
            .map(|it| {
                let tid = it["id"].as_str().unwrap_or_default();
                let art = it["art"].is_string().then(|| load_art(addr, tid, cache)).flatten();
                TileData {
                    id: tid.into(),
                    kind: it["kind"].as_str().unwrap_or_default().into(),
                    title: it["title"].as_str().unwrap_or_default().into(),
                    subtitle: it["label2"].as_str().unwrap_or_default().into(),
                    has_art: art.is_some(),
                    art: art.unwrap_or_default(),
                }
            })
            .collect();
        out.push(RowData {
            label: info["label"].as_str().unwrap_or(id).into(),
            tiles: ModelRc::new(VecModel::from(tiles)),
        });
    }
    Ok(out)
}

/// The raw row listing, to tell whether the library changed without fetching every row.
fn listing_sig(addr: &str) -> Option<String> {
    let (status, _, body) = get(addr, "/rows").ok()?;
    (status == 200).then(|| String::from_utf8_lossy(&body).into_owned())
}

fn state_dir() -> PathBuf {
    PathBuf::from(env_or("UI_STATE", "/tmp/appliance-ui"))
}

fn restore_focus() -> (i32, i32) {
    std::fs::read_to_string(state_dir().join("focus"))
        .ok()
        .and_then(|t| {
            let mut n = t.split_whitespace().filter_map(|x| x.parse().ok());
            Some((n.next()?, n.next()?))
        })
        .unwrap_or((0, 0))
}


/// One-line JSON event to VictoriaLogs (`UI_VLOGS=host:port`; unset or empty disables it).
/// Best effort with short timeouts: a missing log server must never hold up a power action.
fn ship_event(event: &str, action: &str, detail: &str) {
    use std::net::ToSocketAddrs;
    let Ok(addr) = std::env::var("UI_VLOGS") else { return };
    let Some(sock) = addr.to_socket_addrs().ok().and_then(|mut a| a.next()) else { return };
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default();
    let body = serde_json::json!({
        "_msg": format!("power {event}: {action} {detail}").trim_end(),
        "app": "appliance-ui",
        "host": host.trim(),
        "event": event,
        "action": action,
        "detail": detail,
    })
    .to_string();
    let t = Duration::from_secs(2);
    let sent = TcpStream::connect_timeout(&sock, t).and_then(|mut s| {
        s.set_read_timeout(Some(t))?;
        s.set_write_timeout(Some(t))?;
        write!(
            s,
            "POST /insert/jsonline?_stream_fields=app,host&_msg_field=_msg HTTP/1.0\r\nHost: {addr}\r\nContent-Type: application/stream+json\r\nContent-Length: {}\r\n\r\n{body}\n",
            body.len() + 1
        )?;
        let mut status = String::new();
        s.read_to_string(&mut status)?; // wait for the reply so the event is in before we power off
        Ok(status)
    });
    if let Err(e) = sent {
        tracing::warn!(error = %e, "could not ship the event to VictoriaLogs");
    }
}

/// A power event goes to the journal (tracing), the system console and VictoriaLogs.
fn power_event(event: &str, action: &str, detail: &str) {
    tracing::info!(event, action, detail, "power");
    let line = format!("appliance-ui: power {event}: {action} {detail}\n");
    if let Ok(mut c) = std::fs::OpenOptions::new().append(true).open("/dev/console") {
        let _ = c.write_all(line.as_bytes());
    }
    ship_event(event, action, detail);
}

/// A 30 ms decaying 1.8 kHz blip as a 16-bit mono WAV, so there is no sound asset to ship.
fn tick_wav() -> Vec<u8> {
    const RATE: u32 = 44_100;
    let n = RATE as usize * 30 / 1000;
    let mut pcm = Vec::with_capacity(n * 2);
    for i in 0..n {
        let t = i as f32 / RATE as f32;
        let s = (t * 1800.0 * std::f32::consts::TAU).sin() * (-t * 150.0).exp() * 0.6;
        pcm.extend_from_slice(&((s * i16::MAX as f32) as i16).to_le_bytes());
    }
    let mut w = Vec::with_capacity(44 + pcm.len());
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&RATE.to_le_bytes());
    w.extend_from_slice(&(RATE * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(&pcm);
    w
}

/// Plays the tick through `UI_TICK_PLAYER` (default `aplay -q`). A tick that arrives while the
/// previous one is still playing is dropped, so holding an arrow key cannot pile up processes.
/// `UI_TICK=off` silences it.
fn tick_player() -> Option<impl FnMut()> {
    if env_or("UI_TICK", "on") == "off" {
        return None;
    }
    let wav = state_dir().join("tick.wav");
    std::fs::create_dir_all(state_dir()).ok()?;
    std::fs::write(&wav, tick_wav()).ok()?;
    let player = env_or("UI_TICK_PLAYER", "aplay -q");
    let mut child: Option<std::process::Child> = None;
    Some(move || {
        if let Some(c) = child.as_mut() {
            if matches!(c.try_wait(), Ok(None)) {
                return;
            }
        }
        let mut parts = player.split_whitespace();
        let Some(prog) = parts.next() else { return };
        child = Command::new(prog)
            .args(parts)
            .arg(&wav)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| tracing::warn!(error = %e, "cannot play the tick"))
            .ok();
    })
}

fn main() -> Result<(), slint::PlatformError> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("UI_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let mediad = env_or("UI_MEDIAD", "127.0.0.1:8730");
    let cache = PathBuf::from(env_or("UI_CACHE", "/tmp/appliance-ui/art"));
    let inproc = env_or("UI_HANDOFF", "exit") == "inproc";
    let play = env_or("UI_PLAY", "play");

    let ui = App::new()?;
    // mediad may still be starting (or rescanning); show an empty UI rather than dying.
    let rows = load_rows(&mediad, &cache).unwrap_or_else(|e| {
        tracing::error!(error = %e, "cannot load rows");
        Vec::new()
    });
    tracing::info!(rows = rows.len(), "rows loaded");
    ui.set_rows(ModelRc::new(VecModel::from(rows)));
    let (r, c) = restore_focus();
    ui.set_sel_row(r.max(0));
    ui.set_sel_col(c.max(0));

    // mediad scans in the background (a NAS library takes a while the first time, and new
    // downloads arrive later), so keep checking: when its row listing changes, reload the rows.
    let seen = std::rc::Rc::new(std::cell::RefCell::new(listing_sig(&mediad).unwrap_or_default()));
    let refresh = slint::Timer::default();
    {
        let (weak, mediad, cache) = (ui.as_weak(), mediad.clone(), cache.clone());
        refresh.start(slint::TimerMode::Repeated, Duration::from_secs(10), move || {
            let Some(ui) = weak.upgrade() else { return };
            let Some(sig) = listing_sig(&mediad) else { return };
            if sig == *seen.borrow() {
                return;
            }
            match load_rows(&mediad, &cache) {
                Ok(rows) => {
                    tracing::info!(rows = rows.len(), "rows refreshed");
                    ui.set_rows(ModelRc::new(VecModel::from(rows)));
                    let last = ui.get_rows().row_count() as i32 - 1;
                    ui.set_sel_row(ui.get_sel_row().min(last));
                    if let Some(r) = ui.get_rows().row_data(ui.get_sel_row().max(0) as usize) {
                        ui.set_sel_col(ui.get_sel_col().min(r.tiles.row_count() as i32 - 1).max(0));
                    }
                    *seen.borrow_mut() = sig;
                }
                Err(e) => tracing::warn!(error = %e, "cannot refresh rows"),
            }
        });
    }

    if let Some(mut tick) = tick_player() {
        ui.on_moved(move || tick());
    }
    ui.on_config(|| tracing::info!("settings requested (not built yet)"));
    ui.on_arm(|action| power_event("armed", action.as_str(), "press Enter again within 3 s to confirm"));
    ui.on_disarm(|action, why| power_event("cancelled", action.as_str(), why.as_str()));
    ui.on_power(|action| {
        power_event("requested", action.as_str(), "confirmed on the remote");
        let cmd = env_or("UI_POWER_CMD", "systemctl");
        match Command::new(&cmd).arg(action.as_str()).status() {
            Ok(s) if s.success() => power_event("accepted", action.as_str(), &format!("{cmd} exited {s}")),
            Ok(s) => power_event("failed", action.as_str(), &format!("{cmd} exited {s}")),
            Err(e) => power_event("failed", action.as_str(), &format!("cannot run {cmd}: {e}")),
        }
    });

    ui.on_quit(|| {
        let _ = slint::quit_event_loop();
    });
    // The page being shown, kept so season tabs can switch without another round trip.
    let page: std::rc::Rc<std::cell::RefCell<Option<Detail>>> = Default::default();
    let remember_page = |id: &str, season: i32, ep: i32| {
        let _ = std::fs::write(state_dir().join("detail"), format!("{id} {season} {ep}"));
    };

    {
        let (weak, page, mediad, cache_dir) = (ui.as_weak(), page.clone(), mediad.clone(), cache.join("episodes"));
        ui.on_season_changed(move |idx| {
            let Some(ui) = weak.upgrade() else { return };
            if let Some(d) = page.borrow().as_ref() {
                set_season(&ui, &mediad, d, idx as usize, &cache_dir);
                ui.set_sel_ep(0);
            }
        });
    }
    {
        let (weak, page) = (ui.as_weak(), page.clone());
        ui.on_back(move || {
            let Some(ui) = weak.upgrade() else { return };
            let _ = std::fs::remove_file(state_dir().join("detail"));
            *page.borrow_mut() = None;
            ui.set_detail(false);
        });
    }
    {
        let (weak, page, play) = (ui.as_weak(), page.clone(), play.clone());
        ui.on_play_episode(move |i| {
            let Some(ui) = weak.upgrade() else { return };
            let Some(ep) = ui.get_episodes().row_data(i as usize) else { return };
            if let Some(d) = page.borrow().as_ref() {
                remember_page(&d.id, ui.get_sel_season(), i);
            }
            launch(&ui, ep.id.as_str(), inproc, &play);
        });
    }
    {
        let (weak, page, mediad, cache_dir, play) =
            (ui.as_weak(), page.clone(), mediad.clone(), cache.join("episodes"), play.clone());
        ui.on_activate(move |row, col| {
            let Some(ui) = weak.upgrade() else { return };
            let rows = ui.get_rows();
            let Some(tile) = rows.row_data(row as usize).and_then(|r| r.tiles.row_data(col as usize)) else {
                return;
            };
            let _ = std::fs::create_dir_all(state_dir());
            let _ = std::fs::write(state_dir().join("focus"), format!("{row} {col}"));
            if matches!(tile.kind.as_str(), "show" | "channel") {
                let opened = open_detail(&ui, &mediad, tile.id.as_str(), 0, 0, &cache_dir);
                if opened.is_some() {
                    let _ = std::fs::write(state_dir().join("detail"), format!("{} 0 0", tile.id));
                }
                *page.borrow_mut() = opened;
            } else {
                launch(&ui, tile.id.as_str(), inproc, &play);
            }
        });
    }

    // Coming back from playing an episode: reopen the page it was started from.
    if let Ok(text) = std::fs::read_to_string(state_dir().join("detail")) {
        let mut p = text.split_whitespace();
        if let (Some(id), Some(s), Some(e)) = (p.next(), p.next().and_then(|x| x.parse().ok()), p.next().and_then(|x| x.parse().ok())) {
            *page.borrow_mut() = open_detail(&ui, &mediad, id, s, e, &cache.join("episodes"));
        }
    }
    ui.run()?;
    if LAUNCH.load(std::sync::atomic::Ordering::SeqCst) {
        std::process::exit(EXIT_LAUNCH);
    }
    Ok(())
}

static LAUNCH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
