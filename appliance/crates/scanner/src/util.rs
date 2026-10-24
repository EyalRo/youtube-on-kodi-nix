use std::fs;
use std::path::{Path, PathBuf};

use catalog::{DateSource, Episode};
use chrono::{DateTime, NaiveDate, Utc};
use regex::Regex;

pub const VIDEO_EXTS: &[&str] = &[
    "mkv", "mp4", "m4v", "avi", "mov", "webm", "ts", "m2ts", "wmv", "mpg", "mpeg",
];

pub fn ext_lower(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

pub fn is_video(path: &Path) -> bool {
    VIDEO_EXTS.contains(&ext_lower(path).as_str())
}

pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Modification time as a date: when a file arrived, not when it aired.
pub fn mtime_date(path: &Path) -> Option<NaiveDate> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    Some(DateTime::<Utc>::from(modified).date_naive())
}

/// The text of the first of `tags` found in an NFO file.
pub fn nfo_tag(path: &Path, tags: &[&str]) -> Option<String> {
    let text = String::from_utf8_lossy(&fs::read(path).ok()?).into_owned();
    for tag in tags {
        let re = Regex::new(&format!(r"(?is)<{tag}>\s*([^<]+?)\s*</{tag}>")).ok()?;
        if let Some(c) = re.captures(&text) {
            return Some(c[1].to_string());
        }
    }
    None
}

/// Artwork next to a media folder: poster/folder/cover as jpg or png.
pub fn find_art(dir: &Path) -> Option<PathBuf> {
    for stem in ["poster", "folder", "cover", "thumb"] {
        for ext in ["jpg", "jpeg", "png", "webp"] {
            let p = dir.join(format!("{stem}.{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// Immediate subdirectories, sorted, skipping hidden ones.
pub fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = read
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir() && !file_name(p).starts_with('.'))
        .collect();
    dirs.sort();
    dirs
}

/// The episode that decides a show's date: the newest with a real air date; only
/// when none has one, the newest arrival time. A recently downloaded old episode
/// must not outrank a genuine air date.
pub fn newest_episode(episodes: &[Episode]) -> Option<&Episode> {
    episodes
        .iter()
        .filter(|e| e.aired.is_some() && e.date_source != DateSource::Mtime)
        .max_by_key(|e| e.aired)
        .or_else(|| {
            episodes
                .iter()
                .filter(|e| e.aired.is_some())
                .max_by_key(|e| e.aired)
        })
}

/// A thumbnail next to a video file: `<stem>-thumb.jpg` (Kodi, Sonarr) or `<stem>.jpg` (yt-dlp).
pub fn episode_art(video: &Path) -> Option<PathBuf> {
    let dir = video.parent()?;
    let stem = file_stem(video);
    for suffix in ["-thumb", ""] {
        for ext in ["jpg", "jpeg", "png", "webp"] {
            let p = dir.join(format!("{stem}{suffix}.{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}
