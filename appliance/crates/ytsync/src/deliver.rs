//! Move finished downloads from the local work directory to the library on the NAS.
//!
//! The share is mounted `soft`, so a stalled write must never leave something the scanner
//! would take for a finished video: each file is copied to a `.part` name and renamed, and
//! the video itself goes last, after its `.info.json` and thumbnail.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const VIDEO_EXTS: &[&str] = &["mkv", "mp4", "webm", "m4v", "mov"];
const SIDE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp"];

#[derive(Debug, PartialEq, Eq)]
pub struct Delivered {
    /// The video id (as in the download archive).
    pub id: String,
    pub dest: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Failed {
    pub id: Option<String>,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct Outcome {
    pub delivered: Vec<Delivered>,
    pub failed: Vec<Failed>,
}

/// A folder name for a channel: no path separators or characters NFS/SMB clients dislike.
pub fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "/\\:*?\"<>|".contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned = cleaned.trim_matches('.').trim().to_string();
    if cleaned.is_empty() {
        "unknown".into()
    } else {
        cleaned
    }
}

/// Fields of yt-dlp's `.info.json` the appliance reads. The full file is ~10 MB of format lists.
const KEEP: &[&str] = &[
    "id", "title", "channel", "channel_id", "uploader", "upload_date", "timestamp", "duration",
    "description", "webpage_url",
];

fn slim_info(info: &serde_json::Value) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for k in KEEP {
        if let Some(v) = info.get(*k) {
            out.insert((*k).to_string(), v.clone());
        }
    }
    serde_json::Value::Object(out)
}

/// Put the channel picture (fetched into `work` as `channel.avatar_uncropped.jpg`) in each
/// channel folder that has no `poster.jpg` yet, where the scanner looks for artwork.
pub fn deliver_poster(work: &Path, channel_dirs: &[PathBuf]) {
    let avatar = work.join("channel.avatar_uncropped.jpg");
    if !avatar.is_file() {
        return;
    }
    for dir in channel_dirs {
        let poster = dir.join("poster.jpg");
        if !poster.exists() {
            let _ = copy_atomic(&avatar, &poster);
        }
    }
}

fn copy_atomic(from: &Path, to: &Path) -> io::Result<()> {
    let mut part = to.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    let result = fs::copy(from, &part).and_then(|_| fs::rename(&part, to));
    if result.is_err() {
        let _ = fs::remove_file(&part);
    }
    result
}

/// Deliver every complete download in `work` to `<youtube_root>/<channel>/`.
/// `fallback_channel` names the folder when the info.json has no channel.
pub fn deliver(work: &Path, youtube_root: &Path, fallback_channel: Option<&str>) -> Outcome {
    let mut outcome = Outcome::default();
    let Ok(read) = fs::read_dir(work) else {
        return outcome;
    };
    let mut infos: Vec<PathBuf> = read
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".info.json"))
        })
        .collect();
    infos.sort();
    for info_path in infos {
        let name = info_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let stem = name.trim_end_matches(".info.json").to_string();
        let info: Option<serde_json::Value> = fs::read_to_string(&info_path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok());
        let id = info
            .as_ref()
            .and_then(|i| i.get("id"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let Some(info) = info else {
            outcome.failed.push(Failed {
                id,
                reason: format!("unreadable {name}"),
            });
            continue;
        };
        // The work directory is scratch: shrink the sidecar in place before it is copied.
        let _ = fs::write(&info_path, slim_info(&info).to_string());
        let files: Vec<PathBuf> = fs::read_dir(work)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(&format!("{stem}.")))
            })
            .collect();
        let ext_of = |p: &PathBuf| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .unwrap_or_default()
        };
        let Some(video) = files
            .iter()
            .find(|p| VIDEO_EXTS.contains(&ext_of(p).as_str()))
            .cloned()
        else {
            // Interrupted download: leave nothing behind; the archive never recorded it.
            outcome.failed.push(Failed {
                id,
                reason: format!("no finished video for {stem}"),
            });
            for f in files {
                let _ = fs::remove_file(f);
            }
            continue;
        };
        let channel = info
            .get("channel")
            .or_else(|| info.get("uploader"))
            .and_then(|v| v.as_str())
            .or(fallback_channel)
            .map_or_else(|| "unknown".to_string(), safe_name);
        let dir = youtube_root.join(&channel);
        let mut ordered: Vec<&PathBuf> = files
            .iter()
            .filter(|p| **p == info_path || SIDE_EXTS.contains(&ext_of(p).as_str()))
            .collect();
        ordered.push(&video);
        let result = fs::create_dir_all(&dir).and_then(|_| {
            for f in &ordered {
                copy_atomic(f, &dir.join(f.file_name().expect("file has a name")))?;
            }
            Ok(())
        });
        match result {
            Ok(()) => {
                for f in files {
                    let _ = fs::remove_file(f);
                }
                outcome.delivered.push(Delivered {
                    id: id.unwrap_or(stem),
                    dest: dir.join(video.file_name().expect("file has a name")),
                });
            }
            Err(e) => {
                // Take back what was copied so the scanner never sees half a video.
                for f in &ordered {
                    let _ = fs::remove_file(dir.join(f.file_name().expect("file has a name")));
                }
                outcome.failed.push(Failed {
                    id,
                    reason: format!("copy to {} failed: {e}", dir.display()),
                });
            }
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(dir: &Path, name: &str, body: &str) {
        fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn slims_the_sidecar_and_delivers_the_poster() {
        let work = tempfile::tempdir().unwrap();
        let lib = tempfile::tempdir().unwrap();
        put(
            work.path(),
            "2024-05-01 - Hi [abc].info.json",
            r#"{"id":"abc","channel":"Chan","upload_date":"20240501","duration":61,"formats":[{"x":1}],"automatic_captions":{}}"#,
        );
        put(work.path(), "2024-05-01 - Hi [abc].mkv", "video");
        put(work.path(), "channel.avatar_uncropped.jpg", "avatar");
        let out = deliver(work.path(), lib.path(), None);
        assert_eq!(out.delivered.len(), 1);
        let text = fs::read_to_string(lib.path().join("Chan/2024-05-01 - Hi [abc].info.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["upload_date"], "20240501");
        assert_eq!(v["duration"], 61);
        assert!(v.get("formats").is_none() && v.get("automatic_captions").is_none());
        let dirs = [lib.path().join("Chan")];
        deliver_poster(work.path(), &dirs);
        assert_eq!(fs::read_to_string(dirs[0].join("poster.jpg")).unwrap(), "avatar");
        // An existing poster is never replaced.
        fs::write(dirs[0].join("poster.jpg"), "mine").unwrap();
        deliver_poster(work.path(), &dirs);
        assert_eq!(fs::read_to_string(dirs[0].join("poster.jpg")).unwrap(), "mine");
    }

    #[test]
    fn safe_names() {
        assert_eq!(safe_name("AC/DC: Live?"), "AC DC Live");
        assert_eq!(safe_name("  ..  "), "unknown");
        assert_eq!(safe_name("ערוץ חדשות"), "ערוץ חדשות");
    }

    #[test]
    fn delivers_video_with_sidecars_and_cleans_up() {
        let work = tempfile::tempdir().unwrap();
        let lib = tempfile::tempdir().unwrap();
        put(
            work.path(),
            "2024-05-01 - Hi [abc123def45].info.json",
            r#"{"id":"abc123def45","channel":"My/Chan","upload_date":"20240501"}"#,
        );
        put(work.path(), "2024-05-01 - Hi [abc123def45].mkv", "video");
        put(work.path(), "2024-05-01 - Hi [abc123def45].webp", "thumb");
        let out = deliver(work.path(), lib.path(), None);
        assert_eq!(out.delivered.len(), 1);
        assert_eq!(out.delivered[0].id, "abc123def45");
        let dest = lib.path().join("My Chan");
        assert_eq!(
            fs::read_to_string(dest.join("2024-05-01 - Hi [abc123def45].mkv")).unwrap(),
            "video"
        );
        assert!(
            dest.join("2024-05-01 - Hi [abc123def45].info.json")
                .exists()
        );
        assert!(dest.join("2024-05-01 - Hi [abc123def45].webp").exists());
        assert_eq!(fs::read_dir(work.path()).unwrap().count(), 0);
        assert!(
            fs::read_dir(&dest).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".part"))
        );
    }

    #[test]
    fn unfinished_download_is_dropped_and_reported() {
        let work = tempfile::tempdir().unwrap();
        let lib = tempfile::tempdir().unwrap();
        put(
            work.path(),
            "v [zzz].info.json",
            r#"{"id":"zzz","channel":"C"}"#,
        );
        put(work.path(), "v [zzz].mkv.part", "half");
        let out = deliver(work.path(), lib.path(), None);
        assert!(out.delivered.is_empty());
        assert_eq!(out.failed[0].id.as_deref(), Some("zzz"));
        assert_eq!(fs::read_dir(work.path()).unwrap().count(), 0);
        assert_eq!(fs::read_dir(lib.path()).unwrap().count(), 0);
    }

    #[test]
    fn copy_failure_leaves_the_library_clean_and_the_source_intact() {
        let work = tempfile::tempdir().unwrap();
        let lib = tempfile::tempdir().unwrap();
        // The channel folder cannot be created: a file is in the way.
        put(lib.path(), "C", "i am a file");
        put(
            work.path(),
            "v [zzz].info.json",
            r#"{"id":"zzz","channel":"C"}"#,
        );
        put(work.path(), "v [zzz].mkv", "video");
        let out = deliver(work.path(), lib.path(), None);
        assert!(out.delivered.is_empty());
        assert_eq!(out.failed[0].id.as_deref(), Some("zzz"));
        assert!(work.path().join("v [zzz].mkv").exists());
    }

    #[test]
    fn fallback_channel_name_is_used() {
        let work = tempfile::tempdir().unwrap();
        let lib = tempfile::tempdir().unwrap();
        put(work.path(), "v [q].info.json", r#"{"id":"q"}"#);
        put(work.path(), "v [q].mp4", "x");
        deliver(work.path(), lib.path(), Some("Named"));
        assert!(lib.path().join("Named/v [q].mp4").exists());
    }
}
