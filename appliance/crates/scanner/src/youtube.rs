use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use catalog::{DateSource, Episode, Kind, MediaEntry, parse_release};
use chrono::{DateTime, Datelike, NaiveDate};
use regex::Regex;

use crate::util::{episode_art, file_name, file_stem, find_art, is_video, mtime_date, newest_episode, subdirs};
use crate::{Scan, ids};

static DATE_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d{4}-\d{2}-\d{2})\b").unwrap());
static ID_SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*\[[A-Za-z0-9_-]{6,}\]\s*$").unwrap());

struct Info {
    title: Option<String>,
    date: Option<NaiveDate>,
}

/// `<stem>.info.json` as written by yt-dlp (`--write-info-json`).
fn read_info(video: &Path) -> Option<Info> {
    let text = fs::read_to_string(video.with_extension("info.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let from_upload = v
        .get("upload_date")
        .and_then(|d| d.as_str())
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y%m%d").ok());
    let from_ts = v
        .get("timestamp")
        .and_then(|t| t.as_i64())
        .and_then(|t| DateTime::from_timestamp(t, 0))
        .map(|d| d.date_naive());
    Some(Info {
        title: v.get("title").and_then(|t| t.as_str()).map(str::to_string),
        date: from_upload.or(from_ts),
    })
}

pub fn scan(media_root: &Path) -> Scan {
    let mut scan = Scan::default();
    for channel_dir in subdirs(&media_root.join("youtube")) {
        let channel_id = ids::make("c", media_root, &channel_dir);
        // (date, id) -> episode, so same-day uploads get a stable order.
        let mut found: BTreeMap<(Option<NaiveDate>, String), Episode> = BTreeMap::new();
        let mut videos: Vec<_> = fs::read_dir(&channel_dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_file() && is_video(p))
            .collect();
        videos.sort();
        for video in videos {
            let stem = file_stem(&video);
            let info = read_info(&video);
            let (aired, source) = if let Some(d) = info.as_ref().and_then(|i| i.date) {
                (Some(d), DateSource::InfoJson)
            } else if let Some(d) = DATE_PREFIX
                .captures(&stem)
                .and_then(|c| parse_release(&c[1]))
            {
                (Some(d), DateSource::InfoJson)
            } else if let Some(d) = mtime_date(&video) {
                (Some(d), DateSource::Mtime)
            } else {
                (None, DateSource::None)
            };
            let title = info.and_then(|i| i.title).unwrap_or_else(|| {
                let t = DATE_PREFIX.replace(&stem, "");
                let t = ID_SUFFIX.replace(t.trim_start_matches(" -"), "");
                t.trim().to_string()
            });
            let id = ids::make("e", media_root, &video);
            found.insert(
                (aired, id.clone()),
                Episode {
                    id,
                    item_id: channel_id.clone(),
                    season: aired.map_or(0, |d| d.year()),
                    episode: 0,
                    title,
                    aired,
                    art: episode_art(&video),
                    date_source: source,
                    path: video,
                },
            );
        }
        if found.is_empty() {
            scan.warnings.push(format!(
                "channel folder without videos: {}",
                channel_dir.display()
            ));
            continue;
        }
        // Episode number = day-of-year * 100 + rank within the day: stable when older videos are added later.
        let mut rank: BTreeMap<Option<NaiveDate>, i32> = BTreeMap::new();
        let mut episodes: Vec<Episode> = found.into_values().collect();
        for e in &mut episodes {
            let r = rank.entry(e.aired).or_insert(0);
            e.episode = e.aired.map_or(0, |d| d.ordinal() as i32) * 100 + *r;
            *r += 1;
        }
        let newest = newest_episode(&episodes);
        scan.entries.push(MediaEntry {
            id: channel_id,
            kind: Kind::Channel,
            title: file_name(&channel_dir),
            date: newest.and_then(|e| e.aired),
            date_source: newest.map_or(DateSource::None, |e| e.date_source),
            path: channel_dir.clone(),
            system: None,
            art: find_art(&channel_dir),
        });
        scan.episodes.extend(episodes);
    }
    scan
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{create_dir_all, write};

    #[test]
    fn channel_is_a_show_of_downloaded_videos_dated_by_info_json() {
        let m = tempfile::tempdir().unwrap();
        let d = m.path().join("youtube/Jeff Geerling");
        create_dir_all(&d).unwrap();
        write(d.join("2024-06-02 - New Pi [abc123XYZ].mp4"), b"v").unwrap();
        write(
            d.join("2024-06-02 - New Pi [abc123XYZ].info.json"),
            r#"{"id":"abc123XYZ","title":"A New Pi!","upload_date":"20240602"}"#,
        )
        .unwrap();
        write(d.join("2023-12-15 - Old [def456UVW].mp4"), b"v").unwrap();
        let s = scan(m.path());
        assert_eq!(s.entries.len(), 1);
        let c = &s.entries[0];
        assert_eq!((c.kind, c.title.as_str()), (Kind::Channel, "Jeff Geerling"));
        assert_eq!(c.date, NaiveDate::from_ymd_opt(2024, 6, 2));
        assert_eq!(c.date_source, DateSource::InfoJson);
        assert_eq!(s.episodes.len(), 2);
        let new = s.episodes.iter().find(|e| e.title == "A New Pi!").unwrap();
        assert_eq!((new.season, new.date_source), (2024, DateSource::InfoJson));
        // No info.json: title from the file name minus the date and the [id].
        let old = s
            .episodes
            .iter()
            .find(|e| e.aired == NaiveDate::from_ymd_opt(2023, 12, 15))
            .unwrap();
        assert_eq!(old.title, "Old");
    }

    #[test]
    fn same_day_uploads_get_distinct_stable_episode_numbers() {
        let m = tempfile::tempdir().unwrap();
        let d = m.path().join("youtube/Chan");
        create_dir_all(&d).unwrap();
        for id in ["aaaaaaaa", "bbbbbbbb"] {
            write(d.join(format!("2024-03-05 - T [{id}].mp4")), b"v").unwrap();
        }
        let s = scan(m.path());
        let mut nums: Vec<i32> = s.episodes.iter().map(|e| e.episode).collect();
        nums.sort();
        assert_eq!(nums, [6500, 6501]); // March 5, 2024 is day 65 of a leap year
    }

    #[test]
    fn empty_channel_folders_are_reported() {
        let m = tempfile::tempdir().unwrap();
        create_dir_all(m.path().join("youtube/Nothing")).unwrap();
        let s = scan(m.path());
        assert!(s.entries.is_empty());
        assert_eq!(s.warnings.len(), 1);
    }
}
