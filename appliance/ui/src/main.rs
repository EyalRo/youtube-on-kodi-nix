//! Spike: poster rows from mediad, drawn by Slint straight to DRM/KMS, driven by the remote.
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

// Bluetooth controller setup (logic is unit-tested without D-Bus).
mod bt;
mod bluez;
mod gamepad;

slint::include_modules!();

/// Debug logging that exists only in debug builds (`RUSTFLAGS="--cfg appliance_debug"`).
const DEBUG_BUILD: bool = applog::DEBUG_BUILD;

const MAX_TILES: usize = 40;
/// Exit status that asks the supervisor to run `play` (id in `$UI_STATE/launch`).
const EXIT_LAUNCH: i32 = 10;

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

/// The numeric parts of a dotted version ("0.2.10" -> [0, 2, 10]); a leading "v" is ignored.
fn version_parts(v: &str) -> Vec<u64> {
    v.trim().trim_start_matches('v').split('.').map_while(|p| p.parse().ok()).collect()
}

/// The version the update checker left in `path`, if it is newer than `installed`; "" otherwise
/// (no file, unreadable, or not an upgrade).
fn available_upgrade(path: &str, installed: &str) -> String {
    let Ok(text) = std::fs::read_to_string(path) else { return String::new() };
    let offered = text.trim();
    if !offered.is_empty() && version_parts(offered) > version_parts(installed) {
        offered.trim_start_matches('v').to_string()
    } else {
        String::new()
    }
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
fn load_art(addr: &str, id: &str, cache: &Path, live: bool) -> Option<Image> {
    // A poster already on disk is used as it is: decoding it is cheap, fetching it is not.
    for ext in ["jpg", "png", "webp"] {
        let file = cache.join(format!("{id}.{ext}"));
        if file.exists() {
            if let Ok(img) = Image::load_from_path(&file) {
                return Some(img);
            }
        }
    }
    if !live {
        return None;
    }
    load_image(addr, &format!("/items/{id}/art"), id, cache)
}

/// Fetch an image from mediad (`path`) into the cache under `key` and decode it.
fn load_image(addr: &str, path: &str, key: &str, cache: &Path) -> Option<Image> {
    let (status, ctype, body) = match get(addr, path) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(%path, error = %e, "cannot fetch an image");
            return None;
        }
    };
    if status != 200 {
        tracing::warn!(%path, status, "image not available");
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
    if let Err(e) = std::fs::write(&file, body) {
        tracing::warn!(file = %file.display(), error = %e, "cannot cache an image");
        return None;
    }
    Image::load_from_path(&file)
        .map_err(|e| tracing::warn!(file = %file.display(), error = ?e, "cannot decode an image"))
        .ok()
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
    #[cfg(appliance_debug)]
    let started = std::time::Instant::now();
    let v = get_json(addr, &format!("/items/{id}"))?;
    applog::debug!(%id, ms = started.elapsed().as_millis() as u64, "loaded a page");
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
        Ok(_) => {
            tracing::warn!(%id, "the page has no seasons; staying on the home screen");
            return None;
        }
        Err(e) => {
            tracing::error!(%id, error = %e, "cannot open the page");
            return None;
        }
    };
    let season = season.min(detail.seasons.len() - 1);
    ui.set_detail_title(detail.title.as_str().into());
    ui.set_detail_id(id.into());
    ui.set_seasons(ModelRc::new(VecModel::from(
        detail.seasons.iter().map(|s| SharedString::from(s.label.as_str())).collect::<Vec<_>>(),
    )));
    ui.set_sel_season(season as i32);
    set_season(ui, addr, &detail, season, cache);
    let eps = ui.get_episodes().row_count();
    applog::debug!(%id, season, ep, episodes = eps, "page contents set");
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
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::error!(dir = %dir.display(), error = %e, "cannot create the state directory");
    }
    if inproc {
        // Spike (cage): UI_HANDOFF=child keeps the window mapped; the compositor puts the player
        // in front, so the display is never released.
        let child = env_or("UI_HANDOFF", "exit") == "child";
        if !child {
            let _ = ui.hide();
        }
        PLAYING.store(true, std::sync::atomic::Ordering::SeqCst);
        let status = Command::new(play).arg(id).status();
        PLAYING.store(false, std::sync::atomic::Ordering::SeqCst);
        match &status {
            Ok(s) if s.success() => tracing::info!(?status, "play returned"),
            _ => tracing::error!(?status, %id, "play failed"),
        }
        if !child {
            let _ = ui.show();
        }
        ui.set_loading(false);
    } else {
        // Without this file the supervisor will not run `play`, so the press would do nothing.
        if let Err(e) = std::fs::write(dir.join("launch"), id) {
            tracing::error!(file = %dir.join("launch").display(), %id, error = %e, "cannot write the launch request; nothing will play");
        }
        let _ = slint::quit_event_loop();
        LAUNCH.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Show the loader, give it a moment to reach the screen, then hand the display over: the UI exits
/// right after and its last frame stays up until the splash is painted.
fn begin_launch(ui: &App, id: &str, inproc: bool, play: &str) {
    ui.set_loading(true);
    let (weak, id, play) = (ui.as_weak(), id.to_string(), play.to_string());
    slint::Timer::single_shot(Duration::from_millis(250), move || {
        if let Some(ui) = weak.upgrade() {
            launch(&ui, &id, inproc, &play);
        }
    });
}

/// A JSON document from mediad. Live fetches are also saved under `cache`, so the next start can
/// paint from disk (`live = false`) before mediad has answered.
fn rows_json(addr: &str, path: &str, cache: &Path, live: bool) -> Result<Value, String> {
    let file = cache.join("json").join(format!("{}.json", path.trim_matches('/').replace('/', "_")));
    if !live {
        let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        return serde_json::from_str(&text).map_err(|e| format!("{}: {e}", file.display()));
    }
    let v = get_json(addr, path)?;
    let _ = std::fs::create_dir_all(cache.join("json"));
    if let Err(e) = std::fs::write(&file, v.to_string()) {
        tracing::warn!(file = %file.display(), error = %e, "cannot cache a listing");
    }
    Ok(v)
}

/// The home rows. `live = false` builds them from the cache alone (no network, no downloads).
fn load_rows(addr: &str, cache: &Path, live: bool) -> Result<Vec<RowData>, String> {
    let _ = std::fs::create_dir_all(cache);
    let listing = rows_json(addr, "/rows", cache, live)?;
    let mut out = Vec::new();
    for info in listing["rows"].as_array().ok_or("no rows")? {
        if info["count"].as_u64().unwrap_or(0) == 0 {
            continue;
        }
        let id = info["id"].as_str().unwrap_or_default();
        let row = rows_json(addr, &format!("/rows/{id}"), cache, live)?;
        let items = row["items"].as_array().map(|a| a.as_slice()).unwrap_or_default();
        let mut tiles: Vec<TileData> = items
            .iter()
            .take(MAX_TILES)
            .map(|it| {
                let tid = it["id"].as_str().unwrap_or_default();
                let art = it["art"].is_string().then(|| load_art(addr, tid, cache, live)).flatten();
                tile_data(it, art)
            })
            .collect();
        // A row shows at most MAX_TILES; its last tile opens the whole row as a list.
        if items.len() > MAX_TILES {
            tiles.push(TileData {
                id: id.into(),
                kind: "all".into(),
                title: "Show all".into(),
                subtitle: format!("{} items", items.len()).into(),
                has_art: false,
                art: Default::default(),
                viewed: false,
            });
        }
        out.push(RowData {
            id: id.into(),
            label: info["label"].as_str().unwrap_or(id).into(),
            col: 0,
            tiles: ModelRc::new(VecModel::from(tiles)),
        });
    }
    Ok(out)
}

fn tile_data(it: &Value, art: Option<Image>) -> TileData {
    TileData {
        id: it["id"].as_str().unwrap_or_default().into(),
        kind: it["kind"].as_str().unwrap_or_default().into(),
        title: it["title"].as_str().unwrap_or_default().into(),
        subtitle: it["label2"].as_str().unwrap_or_default().into(),
        has_art: art.is_some(),
        art: art.unwrap_or_default(),
        viewed: it["viewed"].as_bool().unwrap_or(false),
    }
}

/// Swap in freshly loaded rows, keeping each row's remembered column (matched by row id).
fn set_rows_keep_cols(ui: &App, mut rows: Vec<RowData>) {
    let old = ui.get_rows();
    for r in rows.iter_mut() {
        let prev = (0..old.row_count()).filter_map(|i| old.row_data(i)).find(|o| o.id == r.id);
        if let Some(o) = prev {
            r.col = o.col.min(r.tiles.row_count() as i32 - 1).max(0);
        }
    }
    ui.set_rows(ModelRc::new(VecModel::from(rows)));
}

/// The raw row listing, to tell whether the library changed without fetching every row.
fn listing_sig(addr: &str) -> Option<String> {
    let (status, _, body) = get(addr, "/rows").ok()?;
    (status == 200).then(|| String::from_utf8_lossy(&body).into_owned())
}

fn state_dir() -> PathBuf {
    PathBuf::from(env_or("UI_STATE", "/tmp/appliance-ui"))
}

/// What the menu button should act on, given the current focus. Returns
/// `(id, season, label)` where `season = Some(n)` means the whole season `n` of
/// a show/channel, and `None` means the single movie/game/episode, or the whole
/// show/channel. `None` overall means nothing is menuable here.
fn menu_target(ui: &App) -> Option<(String, Option<i32>, String)> {
    // A show or channel page: an episode (detail-focus 1), a season tab (0), or
    // the top bar (-1). The bar has nothing to mark.
    if ui.get_detail() {
        match ui.get_detail_focus() {
            1 => {
                let ep = ui.get_episodes().row_data(ui.get_sel_ep() as usize)?;
                Some((ep.id.to_string(), None, ep.title.to_string()))
            }
            0 => {
                let season_label = ui
                    .get_seasons()
                    .row_data(ui.get_sel_season() as usize)?
                    .to_string();
                // "Season N" -> N; a channel year or "Specials"/"Undated" -> None.
                let n = season_label
                    .strip_prefix("Season ")
                    .and_then(|s| s.trim().parse::<i32>().ok());
                let id = ui.get_detail_id().to_string();
                Some((id, n, season_label))
            }
            _ => None,
        }
    } else {
        // Home screen: the focused tile. Shows/channels are the whole show;
        // movies/games are themselves.
        let rows = ui.get_rows();
        let tile = rows
            .row_data(ui.get_sel_row() as usize)?
            .tiles
            .row_data(ui.get_sel_col() as usize)?;
        if ui.get_all_open() || tile.kind == "all" {
            return None;
        }
        Some((tile.id.to_string(), None, tile.title.to_string()))
    }
}

/// A bare HTTP/1.0 POST with a JSON body; returns Ok on 2xx.
fn post_json(addr: &str, path: &str, body: &serde_json::Value) -> Result<(), String> {
    let mut s = TcpStream::connect(addr).map_err(|e| format!("cannot reach mediad at {addr}: {e}"))?;
    s.set_read_timeout(Some(Duration::from_secs(10))).ok();
    let payload = body.to_string();
    write!(
        s,
        "POST {path} HTTP/1.0\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{payload}",
        payload.len()
    )
    .map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).map_err(|e| e.to_string())?;
    let head = String::from_utf8_lossy(&raw)
        .split("\r\n\r\n")
        .next()
        .unwrap_or_default()
        .to_string();
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or("malformed status line")?;
    if status >= 200 && status < 300 {
        Ok(())
    } else {
        Err(format!("{path}: HTTP {status}"))
    }
}

/// After a watched toggle, reload the rows (home) or reopen the page (detail) so
/// the ✓/progress marks reflect the change.
fn refresh_after_toggle(
    ui: &App,
    mediad: &str,
    page: &std::cell::RefCell<Option<Detail>>,
    cache_dir: &Path,
    cache: &Path,
    inproc: &bool,
    play: &str,
) {
    let _ = (inproc, play, cache_dir);
    let id = page.borrow().as_ref().map(|d| d.id.clone());
    if let Some(id) = id {
        // Only the watched/progress marks changed: swap in fresh data and patch the visible
        // tiles in place, keeping their already-decoded artwork (no image fetches or decodes).
        match load_detail(mediad, &id) {
            Ok(fresh) => {
                let model = ui.get_episodes();
                let by_id: std::collections::HashMap<&str, &Value> = fresh
                    .seasons
                    .iter()
                    .flat_map(|s| s.episodes.iter())
                    .filter_map(|e| Some((e["id"].as_str()?, e)))
                    .collect();
                for i in 0..model.row_count() {
                    let Some(mut row) = model.row_data(i) else { continue };
                    let Some(e) = by_id.get(row.id.as_str()) else { continue };
                    let (pos, dur) = (e["position"].as_f64().unwrap_or(0.0), e["duration"].as_f64().unwrap_or(0.0));
                    row.watched = e["watched"].as_bool().unwrap_or(false);
                    row.progress = if dur > 0.0 { (pos / dur).clamp(0.0, 1.0) as f32 } else { 0.0 };
                    model.set_row_data(i, row);
                }
                *page.borrow_mut() = Some(fresh);
            }
            Err(e) => tracing::warn!(%id, error = %e, "cannot refresh the page after watched toggle"),
        }
    } else {
        match load_rows(mediad, cache, true) {
            Ok(rows) => set_rows_keep_cols(ui, rows),
            Err(e) => tracing::warn!(error = %e, "cannot reload rows after watched toggle"),
        }
    }
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


/// A power event goes to the journal (tracing), the system console and VictoriaLogs.
///
/// The VictoriaLogs push is synchronous (`applog::sync_ship`): a power event must land before
/// the box powers off. A missing log server never holds up the action (short timeouts, no retry).
fn power_event(event: &str, action: &str, detail: &str) {
    tracing::info!(event, action, detail, "power");
    let line = format!("appliance-ui: power {event}: {action} {detail}\n");
    if let Ok(mut c) = std::fs::OpenOptions::new().append(true).open("/dev/console") {
        let _ = c.write_all(line.as_bytes());
    }
    applog::sync_ship(
        "appliance-ui",
        &format!("power {event}: {action} {detail}").trim_end(),
        &[
            ("event", serde_json::Value::from(event)),
            ("action", serde_json::Value::from(action)),
            ("detail", serde_json::Value::from(detail)),
        ],
    );
}

const TICK_RATE: u32 = 44_100;

/// A 30 ms decaying 1.8 kHz blip as 16-bit mono samples, so there is no sound asset to ship.
fn tick_samples() -> Vec<i16> {
    let n = TICK_RATE as usize * 30 / 1000;
    (0..n)
        .map(|i| {
            let t = i as f32 / TICK_RATE as f32;
            let s = (t * 1800.0 * std::f32::consts::TAU).sin() * (-t * 150.0).exp() * 0.2;
            (s * i16::MAX as f32) as i16
        })
        .collect()
}

fn open_tick_pcm(device: &str) -> Result<alsa::PCM, alsa::Error> {
    use alsa::pcm::{Access, Format, HwParams};
    let pcm = alsa::PCM::new(device, alsa::Direction::Playback, false)?;
    {
        let hw = HwParams::any(&pcm)?;
        hw.set_channels(1)?;
        hw.set_rate(TICK_RATE, alsa::ValueOr::Nearest)?;
        hw.set_format(Format::s16())?;
        hw.set_access(Access::RWInterleaved)?;
        pcm.hw_params(&hw)?;
    }
    Ok(pcm)
}

/// Plays the tick straight to ALSA (`UI_TICK_DEVICE`, default "default") from a thread that owns
/// the PCM. A tick that arrives while the previous one is still playing is dropped, so holding a
/// direction cannot pile them up. `UI_TICK=off` silences it.
fn tick_player() -> Option<impl FnMut()> {
    if env_or("UI_TICK", "on") == "off" {
        return None;
    }
    let device = env_or("UI_TICK_DEVICE", "default");
    let samples = tick_samples();
    let (tx, rx) = std::sync::mpsc::sync_channel::<()>(1);
    std::thread::spawn(move || {
        let mut pcm: Option<alsa::PCM> = None;
        let mut warned = false;
        for () in rx {
            if pcm.is_none() {
                match open_tick_pcm(&device) {
                    Ok(p) => pcm = Some(p),
                    Err(e) => {
                        if !std::mem::replace(&mut warned, true) {
                            tracing::warn!(%device, error = %e, "cannot open the sound device for the tick");
                        }
                        continue;
                    }
                }
            }
            let Some(p) = pcm.as_ref() else { continue };
            let played = p.io_i16().and_then(|io| io.writei(&samples)).and_then(|_| p.drain());
            if let Err(e) = played {
                // Underrun or a vanished device: try to recover, else reopen on the next tick.
                if p.try_recover(e, true).is_err() {
                    pcm = None;
                }
            }
        }
    });
    Some(move || {
        let _ = tx.try_send(());
    })
}

/// Put a Bluetooth snapshot on the Controllers page. Runs on the UI thread.
fn apply_bt(ui: &App, paths: &std::sync::Mutex<Vec<String>>, snap: &bt::Snapshot) {
    let rows: Vec<BtDevice> = snap
        .views
        .iter()
        .map(|v| BtDevice {
            name: v.name.clone().into(),
            status: match v.state {
                bt::State::Connected => "Connected",
                bt::State::Paired => "Paired",
                bt::State::Available => "Available",
            }
            .into(),
            kind: match v.kind {
                bt::Kind::Other => 0,
                bt::Kind::Controller(None) => 1,
                bt::Kind::Controller(Some(bt::Model::M30)) => 2,
                bt::Kind::Controller(Some(bt::Model::Lite2)) => 3,
            },
            state: match v.state {
                bt::State::Available => 0,
                bt::State::Paired => 1,
                bt::State::Connected => 2,
            },
        })
        .collect();
    let n = rows.len() as i32;
    *paths.lock().unwrap() = snap.views.iter().map(|v| v.path.clone()).collect();
    ui.set_bt_devices(ModelRc::new(VecModel::from(rows)));
    ui.set_bt_available(snap.available);
    ui.set_bt_scanning(snap.scanning);
    ui.set_bt_busy(false);
    ui.set_bt_message(snap.message.as_str().into());
    if ui.get_bt_sel() > n {
        ui.set_bt_sel(n);
    }
}

/// The Bluetooth worker: owns the BlueZ session, serves the Controllers page's commands, and while
/// the page is open refreshes it about once a second. BlueZ calls (pairing takes seconds) must not
/// block the UI thread, so they all happen here and the results come back as snapshots.
fn bt_worker(ui: slint::Weak<App>, paths: std::sync::Arc<std::sync::Mutex<Vec<String>>>) -> std::sync::mpsc::Sender<bt::Command> {
    use std::sync::mpsc::RecvTimeoutError;
    let (tx, rx) = std::sync::mpsc::channel::<bt::Command>();
    let spawned = std::thread::Builder::new().name("bluetooth".into()).spawn(move || {
        let mut session: Option<bt::Session<bluez::Bluez>> = None;
        let mut open = false;
        let mut last: Option<(String, bool, String)> = None;
        loop {
            let cmd = match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(c) => c,
                Err(RecvTimeoutError::Timeout) if open => bt::Command::Poll,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return,
            };
            match cmd {
                bt::Command::Open => open = true,
                bt::Command::Close => open = false,
                _ => {}
            }
            if session.is_none() {
                match bluez::Bluez::connect() {
                    Ok(b) => session = Some(bt::Session::new(b)),
                    Err(e) => {
                        tracing::warn!(error = %e, "cannot reach the system D-Bus for Bluetooth");
                        let snap = bt::Snapshot { message: format!("Bluetooth is not available ({e})"), ..Default::default() };
                        let paths = paths.clone();
                        let _ = ui.upgrade_in_event_loop(move |ui| apply_bt(&ui, &paths, &snap));
                        continue;
                    }
                }
            }
            if let Some(s) = session.as_mut() {
                let snap = s.handle(cmd);
                let summary = (
                    snap.views.iter().map(|v| format!("{} [{:?}]", v.name, v.state)).collect::<Vec<_>>().join(", "),
                    snap.scanning,
                    snap.message.clone(),
                );
                if last.as_ref() != Some(&summary) {
                    tracing::info!(devices = %summary.0, scanning = summary.1, message = %summary.2, "controllers updated");
                    last = Some(summary);
                }
                let paths = paths.clone();
                let _ = ui.upgrade_in_event_loop(move |ui| apply_bt(&ui, &paths, &snap));
            }
        }
    });
    if let Err(e) = spawned {
        tracing::error!(error = %e, "cannot start the Bluetooth worker");
    }
    tx
}

/// True while a player runs as a child of the UI: the UI is behind it and must not react to the
/// buttons pressed for the game (they would also queue up and fire when the player exits).
static PLAYING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn playing() -> bool {
    PLAYING.load(std::sync::atomic::Ordering::SeqCst)
}

/// The range of one absolute axis, from the kernel (EVIOCGABS); a typical byte range if unknown.
fn abs_range(fd: i32, code: u16) -> (i32, i32) {
    let mut info = [0i32; 6]; // struct input_absinfo: value, minimum, maximum, fuzz, flat, resolution
    // EVIOCGABS(abs) = _IOR('E', 0x40 + abs, struct input_absinfo), 24 bytes.
    let req: libc::c_ulong = 0x8018_4540 + code as libc::c_ulong;
    // SAFETY: `info` is the 24-byte struct the request writes, and `fd` is an open evdev file.
    let r = unsafe { libc::ioctl(fd, req, info.as_mut_ptr()) };
    if r == 0 && info[2] > info[1] { (info[1], info[2]) } else { (0, 255) }
}

/// Make the UI see a navigation key from a game controller.
fn press_nav(ui: &slint::Weak<App>, nav: gamepad::Nav) {
    use slint::platform::{Key, WindowEvent};
    // Back is Backspace (go back a level), not Escape, which quits the UI on the home screen.
    let key: SharedString = match nav {
        gamepad::Nav::Up => Key::UpArrow.into(),
        gamepad::Nav::Down => Key::DownArrow.into(),
        gamepad::Nav::Left => Key::LeftArrow.into(),
        gamepad::Nav::Right => Key::RightArrow.into(),
        gamepad::Nav::Select => Key::Return.into(),
        gamepad::Nav::Back => Key::Backspace.into(),
    };
    let _ = ui.upgrade_in_event_loop(move |ui| {
        let w = ui.window();
        w.dispatch_event(WindowEvent::KeyPressed { text: key.clone() });
        w.dispatch_event(WindowEvent::KeyReleased { text: key });
    });
}

/// Read one input device for as long as it exists. A game controller drives the UI like arrow keys;
/// anything else is a remote, whose Back, Home and menu keys the key mapping delivers as an empty key
/// (text "\0"), the same for every button it does not know, so we read the kernel's events as the
/// player does. Codes: KEY_BACK 158, KEY_HOMEPAGE 172.
fn watch_device(path: PathBuf, ui: slint::Weak<App>, watched: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<PathBuf>>>) {
    use std::os::fd::AsRawFd;
    use std::sync::Mutex;
    const KEY_BACK: u16 = 158;
    const KEY_HOMEPAGE: u16 = 172;
    // Menu-type keys remotes send: KEY_MENU 139, KEY_SETUP 141, KEY_COMPOSE 127, KEY_OPTION 357,
    // KEY_INFO 358, KEY_CONTEXT_MENU 438.
    const MENU_KEYS: [u16; 6] = [139, 141, 127, 357, 358, 438];
    // The remote shows up as several devices; one press must act once.
    static LAST: Mutex<[Option<std::time::Instant>; 3]> = Mutex::new([None, None, None]);
    let mut dev = match std::fs::File::open(&path) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(device = %path.display(), error = %e, "cannot open an input device");
            watched.lock().unwrap().remove(&path);
            return;
        }
    };
    let node = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
    let caps = std::fs::read_to_string(format!("/sys/class/input/{node}/device/capabilities/key")).unwrap_or_default();
    let name = std::fs::read_to_string(format!("/sys/class/input/{node}/device/name")).unwrap_or_default();
    let mut pad = gamepad::is_gamepad(&caps).then(|| {
        let fd = dev.as_raw_fd();
        let (x, y) = (abs_range(fd, 0), abs_range(fd, 1));
        gamepad::Pad::new(gamepad::Axis::new(x.0, x.1), gamepad::Axis::new(y.0, y.1))
    });
    tracing::info!(device = %path.display(), name = %name.trim(), gamepad = pad.is_some(), "watching an input device");
    std::thread::spawn(move || {
        let mut ev = [0u8; 24];
        while dev.read_exact(&mut ev).is_ok() {
            let (kind, code, value) = (
                u16::from_le_bytes([ev[16], ev[17]]),
                u16::from_le_bytes([ev[18], ev[19]]),
                i32::from_le_bytes([ev[20], ev[21], ev[22], ev[23]]),
            );
            if playing() {
                continue;
            }
            if let Some(pad) = pad.as_mut() {
                if let Some(nav) = pad.event(kind, code, value) {
                    applog::debug!(?nav, "game controller navigation");
                    press_nav(&ui, nav);
                }
                continue;
            }
            if kind != 1 || value != 1 {
                continue; // button presses only
            }
            applog::debug!(code, "input device button press");
            let slot = match code {
                KEY_BACK => 0,
                KEY_HOMEPAGE => 1,
                c if MENU_KEYS.contains(&c) => 2,
                _ => {
                    if !(code < 100 || [103, 105, 106, 108].contains(&code)) {
                        applog::debug!(code, "unmapped remote key");
                    }
                    continue;
                }
            };
            {
                let mut last = LAST.lock().unwrap();
                if last[slot].is_some_and(|t| t.elapsed() < Duration::from_millis(250)) {
                    continue;
                }
                last[slot] = Some(std::time::Instant::now());
            }
            tracing::info!(button = match slot { 0 => "back", 1 => "home", _ => "menu" }, "remote button");
            let _ = ui.upgrade_in_event_loop(move |ui| {
                if slot == 0 { ui.invoke_go_back() }
                else if slot == 1 { ui.invoke_go_home() }
                else { ui.invoke_menu() }
            });
        }
        tracing::info!(device = %path.display(), "input device closed");
        // Let the rescan watch this node again if a device comes back under the same name.
        watched.lock().unwrap().remove(&path);
    });
}

/// Watch every input device, including ones that appear later: a Bluetooth controller connects
/// after the UI has started, so the device list is rescanned every couple of seconds.
fn watch_remote(ui: slint::Weak<App>) {
    std::thread::spawn(move || {
        let watched = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::<PathBuf>::new()));
        loop {
            match std::fs::read_dir("/dev/input") {
                Ok(dir) => {
                    for path in dir.filter_map(Result::ok).map(|e| e.path()) {
                        let is_event = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("event"));
                        if is_event && watched.lock().unwrap().insert(path.clone()) {
                            watch_device(path, ui.clone(), watched.clone());
                        }
                    }
                }
                Err(e) => tracing::warn!(error = %e, "cannot list /dev/input; the remote and controllers will not work"),
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}

fn main() -> Result<(), slint::PlatformError> {
    applog::init_with_default(
        "appliance-ui",
        "UI_LOG",
        if DEBUG_BUILD { "info,appliance_ui=debug" } else { "info" },
    );
    tracing::info!(debug_build = DEBUG_BUILD, "appliance-ui starting");
    let mediad = env_or("UI_MEDIAD", "127.0.0.1:8730");
    let cache = PathBuf::from(env_or("UI_CACHE", "/tmp/appliance-ui/art"));
    let inproc = matches!(env_or("UI_HANDOFF", "exit").as_str(), "inproc" | "child");
    let play = env_or("UI_PLAY", "play");
    // In child mode (cage) no supervisor loop clears the saved page and focus (session.sh does it in
    // exit mode), and the UI process lives across playbacks, so any start of it is a fresh start:
    // open on the home screen instead of a page saved before the last reboot.
    if env_or("UI_HANDOFF", "exit") == "child" {
        for f in ["focus", "detail"] {
            let _ = std::fs::remove_file(state_dir().join(f));
        }
    }

    let ui = App::new()?;
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    // The update checker writes the newest published version to this file; the settings button
    // wears a badge while it is newer than ours. Polled, so it appears without a restart.
    let update_file = env_or("UI_UPDATE_FILE", "/var/lib/appliance/available-version");
    ui.set_available(available_upgrade(&update_file, env!("CARGO_PKG_VERSION")).into());
    let update_poll = slint::Timer::default();
    {
        let (weak, file) = (ui.as_weak(), update_file.clone());
        update_poll.start(slint::TimerMode::Repeated, Duration::from_secs(60), move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_available(available_upgrade(&file, env!("CARGO_PKG_VERSION")).into());
            }
        });
    }
    // mediad may still be starting (or rescanning); show an empty UI rather than dying.
    // The rows of the last run are on disk: paint those at once (no network, no downloads) so the
    // screen is never blank while mediad answers, then replace them with the live ones.
    let cached = load_rows(&mediad, &cache, false).ok().filter(|r| !r.is_empty());
    let from_cache = cached.is_some();
    let rows = cached.unwrap_or_else(|| {
        load_rows(&mediad, &cache, true).unwrap_or_else(|e| {
            tracing::error!(error = %e, "cannot load rows");
            Vec::new()
        })
    });
    tracing::info!(rows = rows.len(), from_cache, "rows loaded");
    ui.set_rows(ModelRc::new(VecModel::from(rows)));
    let (r, c) = restore_focus();
    ui.set_sel_row(r.max(0));
    ui.set_sel_col(c.max(0));
    let first_refresh = slint::Timer::default();
    if from_cache {
        let (weak, mediad, cache) = (ui.as_weak(), mediad.clone(), cache.clone());
        first_refresh.start(slint::TimerMode::SingleShot, Duration::from_millis(300), move || {
            let Some(ui) = weak.upgrade() else { return };
            match load_rows(&mediad, &cache, true) {
                Ok(rows) => {
                    tracing::info!(rows = rows.len(), "rows refreshed from mediad");
                    set_rows_keep_cols(&ui, rows);
                    let last = ui.get_rows().row_count() as i32 - 1;
                    ui.set_sel_row(ui.get_sel_row().min(last));
                }
                Err(e) => tracing::warn!(error = %e, "cannot refresh the cached rows"),
            }
        });
    }

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
            match load_rows(&mediad, &cache, true) {
                Ok(rows) => {
                    tracing::info!(rows = rows.len(), "rows refreshed");
                    set_rows_keep_cols(&ui, rows);
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
    ui.on_config(|| tracing::info!(version = env!("CARGO_PKG_VERSION"), "settings opened"));
    // Bluetooth controllers (Settings -> Controllers). Pairing hints come from 8BitDo's manuals.
    ui.set_bt_hint_m30(bt::hint(bt::Model::M30).into());
    ui.set_bt_hint_lite2(bt::hint(bt::Model::Lite2).into());
    ui.set_bt_again_m30(format!("To reconnect later: {}", bt::reconnect_hint(bt::Model::M30)).into());
    ui.set_bt_again_lite2(format!("To reconnect later: {}", bt::reconnect_hint(bt::Model::Lite2)).into());
    let bt_paths = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let bt_tx = bt_worker(ui.as_weak(), bt_paths.clone());
    {
        let tx = bt_tx.clone();
        ui.on_bt_open(move || {
            tracing::info!("controllers page opened");
            let _ = tx.send(bt::Command::Open);
        });
        let tx = bt_tx.clone();
        ui.on_bt_close(move || {
            tracing::info!("controllers page closed");
            let _ = tx.send(bt::Command::Close);
        });
        let tx = bt_tx.clone();
        ui.on_bt_scan(move || {
            let _ = tx.send(bt::Command::ToggleScan);
        });
        // The action callbacks get the index into the list on screen; the paths are in the same order.
        let pick = move |paths: &std::sync::Mutex<Vec<String>>, i: i32| paths.lock().unwrap().get(i as usize).cloned();
        let (tx, paths) = (bt_tx.clone(), bt_paths.clone());
        ui.on_bt_pair(move |i| {
            if let Some(p) = pick(&paths, i) {
                tracing::info!(path = %p, "pair requested");
                let _ = tx.send(bt::Command::Pair(p));
            }
        });
        let (tx, paths) = (bt_tx.clone(), bt_paths.clone());
        ui.on_bt_connect(move |i| {
            if let Some(p) = pick(&paths, i) {
                tracing::info!(path = %p, "connect requested");
                let _ = tx.send(bt::Command::Connect(p));
            }
        });
        let (tx, paths) = (bt_tx.clone(), bt_paths.clone());
        ui.on_bt_disconnect(move |i| {
            if let Some(p) = pick(&paths, i) {
                tracing::info!(path = %p, "disconnect requested");
                let _ = tx.send(bt::Command::Disconnect(p));
            }
        });
        let (tx, paths) = (bt_tx.clone(), bt_paths.clone());
        ui.on_bt_forget(move |i| {
            if let Some(p) = pick(&paths, i) {
                tracing::info!(path = %p, "forget requested");
                let _ = tx.send(bt::Command::Forget(p));
            }
        });
    }
    // Look for new videos now instead of at the next timer run. `--no-block` queues the oneshot and
    // returns; a run already in progress absorbs the request.
    ui.on_sync(|| {
        let cmd = env_or("UI_SYNC_CMD", "systemctl");
        let unit = env_or("UI_SYNC_UNIT", "ytsync.service");
        match Command::new(&cmd).args(["start", "--no-block", &unit]).status() {
            Ok(s) if s.success() => tracing::info!(%unit, "sync requested from the top bar"),
            Ok(s) => tracing::error!(%unit, status = %s, "sync request refused; is the polkit rule for the UI user in place?"),
            Err(e) => tracing::error!(%unit, error = %e, "cannot run {cmd} to request a sync"),
        }
    });
    #[cfg(appliance_debug)]
    ui.on_key_debug(|text| {
        let codes: Vec<u32> = text.chars().map(u32::from).collect();
        applog::debug!(?codes, "remote button reached the UI");
    });
    ui.on_arm(|action| power_event("armed", action.as_str(), "press select again within 3 s to confirm"));
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
            applog::debug!("back to the home screen");
            let _ = std::fs::remove_file(state_dir().join("detail"));
            *page.borrow_mut() = None;
            ui.set_detail(false);
        });
    }
    {
        let (weak, page, play) = (ui.as_weak(), page.clone(), play.clone());
        ui.on_play_episode(move |i| {
            let Some(ui) = weak.upgrade() else { return };
            let Some(ep) = ui.get_episodes().row_data(i as usize) else {
                tracing::warn!(index = i, "play pressed on an episode that is not in the list");
                return;
            };
            tracing::info!(episode = %ep.id, title = %ep.title, "episode chosen");
            if let Some(d) = page.borrow().as_ref() {
                remember_page(&d.id, ui.get_sel_season(), i);
            }
            begin_launch(&ui, ep.id.as_str(), inproc, &play);
        });
    }
    {
        let (page, mediad_c, cache_dir, play) = (page.clone(), mediad.clone(), cache.join("episodes"), play.clone());
        // Open a show/channel page, or start the movie/game/video.
        let choose = std::rc::Rc::new(move |ui: &App, tile: &TileData| {
            if matches!(tile.kind.as_str(), "show" | "channel") {
                let opened = open_detail(ui, &mediad_c, tile.id.as_str(), 0, 0, &cache_dir);
                if opened.is_some() {
                    let _ = std::fs::write(state_dir().join("detail"), format!("{} 0 0", tile.id));
                }
                *page.borrow_mut() = opened;
            } else {
                begin_launch(ui,tile.id.as_str(), inproc, &play);
            }
        });
        let (weak, choose_tile, mediad, cache) = (ui.as_weak(), choose.clone(), mediad.clone(), cache.clone());
        ui.on_activate(move |row, col| {
            let Some(ui) = weak.upgrade() else { return };
            let rows = ui.get_rows();
            let Some(tile) = rows.row_data(row as usize).and_then(|r| r.tiles.row_data(col as usize)) else {
                tracing::warn!(row, col, "activate on a tile that does not exist");
                return;
            };
            if tile.kind == "all" {
                // The whole row as a plain list: no artwork, so a big library opens at once.
                let label = rows.row_data(row as usize).map(|r| r.label).unwrap_or_default();
                match rows_json(&mediad, &format!("/rows/{}", tile.id), &cache, true) {
                    Ok(v) => {
                        let mut items: Vec<TileData> = v["items"]
                            .as_array()
                            .map(|a| a.as_slice())
                            .unwrap_or_default()
                            .iter()
                            .map(|it| tile_data(it, None))
                            .collect();
                        // The full list is for finding a title, so it is alphabetical whatever the row's order.
                        items.sort_by_cached_key(|t| t.title.to_lowercase());
                        tracing::info!(row, items = items.len(), "show all opened");
                        ui.set_all_title(label);
                        ui.set_all_items(ModelRc::new(VecModel::from(items)));
                        ui.set_sel_all(0);
                        ui.set_all_open(true);
                    }
                    Err(e) => tracing::warn!(error = %e, "cannot load the full row"),
                }
                return;
            }
            let _ = std::fs::create_dir_all(state_dir());
            let _ = std::fs::write(state_dir().join("focus"), format!("{row} {col}"));
            tracing::info!(row, col, id = %tile.id, kind = %tile.kind, title = %tile.title, "tile chosen");
            choose_tile(&ui, &tile);
        });
        let weak = ui.as_weak();
        ui.on_all_activate(move |i| {
            let Some(ui) = weak.upgrade() else { return };
            let Some(tile) = ui.get_all_items().row_data(i as usize) else { return };
            tracing::info!(id = %tile.id, kind = %tile.kind, title = %tile.title, "tile chosen from the full list");
            ui.set_all_open(false);
            choose(&ui, &tile);
        });
        // Home puts every row back at its first tile.
        let weak = ui.as_weak();
        ui.on_home_reset(move || {
            let Some(ui) = weak.upgrade() else { return };
            let rows = ui.get_rows();
            for i in 0..rows.row_count() {
                if let Some(mut r) = rows.row_data(i) {
                    r.col = 0;
                    rows.set_row_data(i, r);
                }
            }
        });
    }

    // The watched-toggle menu. Pressing menu on a highlighted tile or an episode
    // (or a season tab) opens a one-entry menu: "Mark watched"/"Mark unwatched",
    // decided by the target's current watched state. Shows and seasons are
    // groupings: a show is watched when all its episodes are.
    {
        let (weak, mediad, page, cache_dir, cache, inproc, play) = (
            ui.as_weak(),
            mediad.clone(),
            page.clone(),
            cache.join("episodes"),
            cache.clone(),
            inproc,
            play.clone(),
        );
        ui.on_menu({
            let (weak, mediad) = (weak.clone(), mediad.clone());
            move || {
                let Some(ui) = weak.upgrade() else { return };
                // A target is (id, season, label). season = Some(n) means a season tab
                // of a show/channel; None means the whole thing (tile or episode).
                let target = menu_target(&ui);
                let Some((id, season, label)) = target else {
                    applog::debug!("menu pressed but nothing is menuable here");
                    return;
                };
                let season_q = season.map(|s| format!("&season={s}")).unwrap_or_default();
                let watched = get_json(&mediad, &format!("/items/{id}/watched{season_q}"))
                    .and_then(|v| v["watched"].as_bool().ok_or_else(|| "no watched field".to_string()))
                    .unwrap_or_else(|e| {
                        tracing::warn!(id, error = %e, "cannot read watched state; assuming unwatched");
                        false
                    });
                ui.set_menu_title(format!("{label}").into());
                ui.set_menu_action(if watched { "Mark unwatched".into() } else { "Mark watched".into() });
                ui.set_menu_open(true);
                tracing::info!(id, season, watched, "menu opened");
            }
        });
        ui.on_menu_close({
            let weak = weak.clone();
            move || {
                if let Some(ui) = weak.upgrade() {
                    ui.set_menu_open(false);
                }
            }
        });
        ui.on_menu_activate({
            let (weak, mediad) = (weak.clone(), mediad.clone());
            move || {
                let Some(ui) = weak.upgrade() else { return };
                let Some((id, season, _)) = menu_target(&ui) else { return };
                let watching = ui.get_menu_action().as_str().starts_with("Mark watched");
                let base = format!("/items/{id}/watched");
                let uri = match (season, watching) {
                    (Some(s), true) => format!("{base}?season={s}"),
                    (Some(s), false) => format!("{base}?season={s}&watched=false"),
                    (None, true) => base,
                    (None, false) => format!("{base}?watched=false"),
                };
                let res = post_json(&mediad, &uri, &serde_json::json!({}));
                match &res {
                    Ok(()) => tracing::info!(id, season, watching, "watched toggle applied"),
                    Err(e) => tracing::error!(id, season, watching, error = %e, "watched toggle failed"),
                }
                ui.set_menu_open(false);
                // Refresh whatever page is open so the ✓/progress marks move.
                refresh_after_toggle(&ui, &mediad, &page, &cache_dir, &cache, &inproc, &play);
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
    watch_remote(ui.as_weak());
    ui.run()?;
    if LAUNCH.load(std::sync::atomic::Ordering::SeqCst) {
        std::process::exit(EXIT_LAUNCH);
    }
    Ok(())
}

static LAUNCH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
mod update_tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(version_parts("0.2.10") > version_parts("0.2.4"));
        assert!(version_parts("v1.0.0") > version_parts("0.9.9"));
        assert_eq!(version_parts("0.2.4"), version_parts("v0.2.4"));
    }

    #[test]
    fn only_a_newer_version_is_offered() {
        let dir = std::env::temp_dir().join(format!("ui-update-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("available-version");
        let path = f.to_str().unwrap();
        assert_eq!(available_upgrade(path, "0.2.4"), "");
        std::fs::write(&f, "v0.3.0\n").unwrap();
        assert_eq!(available_upgrade(path, "0.2.4"), "0.3.0");
        std::fs::write(&f, "0.2.4\n").unwrap();
        assert_eq!(available_upgrade(path, "0.2.4"), "");
        std::fs::write(&f, "0.2.3").unwrap();
        assert_eq!(available_upgrade(path, "0.2.4"), "");
        std::fs::remove_dir_all(&dir).ok();
    }
}
