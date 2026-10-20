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
    let (status, ctype, body) = get(addr, &format!("/items/{id}/art")).ok()?;
    if status != 200 {
        return None;
    }
    let ext = match ctype.as_str() {
        "image/png" => "png",
        "image/webp" => "webp",
        _ => "jpg",
    };
    let file = cache.join(format!("{id}.{ext}"));
    std::fs::write(&file, body).ok()?;
    Image::load_from_path(&file).ok()
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

    let weak = ui.as_weak();
    ui.on_quit(|| {
        let _ = slint::quit_event_loop();
    });
    ui.on_activate(move |row, col| {
        let Some(ui) = weak.upgrade() else { return };
        let rows = ui.get_rows();
        let Some(tile) = rows
            .row_data(row as usize)
            .and_then(|r| r.tiles.row_data(col as usize))
        else {
            return;
        };
        let id: SharedString = tile.id;
        tracing::info!(%id, inproc, "handing the display to play");
        let dir = state_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("focus"), format!("{row} {col}"));
        if inproc {
            let _ = ui.hide();
            let status = Command::new(&play).arg(id.as_str()).status();
            tracing::info!(?status, "play returned");
            let _ = ui.show();
        } else {
            let _ = std::fs::write(dir.join("launch"), id.as_str());
            let _ = slint::quit_event_loop();
            LAUNCH.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    });
    ui.run()?;
    if LAUNCH.load(std::sync::atomic::Ordering::SeqCst) {
        std::process::exit(EXIT_LAUNCH);
    }
    Ok(())
}

static LAUNCH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
