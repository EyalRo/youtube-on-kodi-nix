use std::path::Path;
use std::sync::LazyLock;

use catalog::{DateSource, Episode, Kind, MediaEntry, parse_release};
use chrono::NaiveDate;
use regex::Regex;
use walkdir::WalkDir;

use crate::util::{
    episode_art, file_name, file_stem, find_art, is_video, mtime_date, newest_episode, nfo_tag, subdirs,
};
use crate::{Scan, ids};

static SXXEYY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])s(\d{1,3})e(\d{1,4})").unwrap());
static DATED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{4})[-.](\d{2})[-.](\d{2})").unwrap());
/// `2x15`, `2X10`: season `x` episode.
static SEASON_X_EP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])(\d{1,2})x(\d{1,4})").unwrap());
/// `S2 - 11`: season marker then a dash then the episode.
static S_DASH_EP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])s(\d{1,2})\s*-\s*(\d{1,4})").unwrap());
/// `E05`: a single-season show's episode with no season marker. The episode must
/// not run into an alphanumeric (handled by checking `E(\d+)` is followed by a
/// non-alphanumeric or end of string, since `regex` has no look-around).
static E_ONLY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])e(\d{1,4})([^a-z0-9]|$)").unwrap());
/// `suits0112` = S01E12: a compact 2-digit season (`0\d`, 01-09) glued to a
/// 2-digit episode, directly after the show name. Requires the season to carry a
/// leading zero and a non-digit before it, and a non-alphanumeric after the
/// episode, so a 4-digit year like `2024` (or a digit-run inside a longer number)
/// never matches. (Rust `regex` has no look-around, so the surrounding non-matching
/// characters are matched, not looked behind/ahead.)
static COMPACT_SXX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|[^0-9])(0\d)(\d{2})([^a-z0-9]|$)").unwrap());

/// (season, episode, rest of the name after the marker)
fn parse_episode(stem: &str) -> Option<(i32, i32, String)> {
    // Scene/rename forms, most explicit first. `season_group` of 0 means the
    // pattern has no season group and it is a single-season show (E05 -> season 1).
    for (re, season_group, episode_group) in [
        (&*SXXEYY, 1, 2),      // S02E03
        (&*SEASON_X_EP, 1, 2), // 2x15
        (&*S_DASH_EP, 1, 2),   // S2 - 11
        (&*E_ONLY, 0, 1),      // E05
        (&*COMPACT_SXX, 1, 2), // suits0112
    ] {
        let Some(c) = re.captures(stem) else { continue };
        let season = if season_group == 0 {
            1
        } else {
            c.get(season_group)?.as_str().parse().ok()?
        };
        let episode = c.get(episode_group)?.as_str().parse().ok()?;
        let end = c.get(0)?.end();
        let rest = stem[end..]
            .trim_matches(|ch: char| " -._".contains(ch))
            .to_string();
        return Some((season, episode, rest));
    }
    None
}

fn episode_date(file: &Path, stem: &str) -> (Option<NaiveDate>, DateSource) {
    let nfo = file.with_extension("nfo");
    if let Some(d) =
        nfo_tag(&nfo, &["aired", "premiered", "releasedate"]).and_then(|v| parse_release(&v))
    {
        return (Some(d), DateSource::Nfo);
    }
    if let Some(c) = DATED.captures(stem)
        && let Some(d) = parse_release(&format!("{}-{}-{}", &c[1], &c[2], &c[3]))
    {
        return (Some(d), DateSource::Nfo);
    }
    match mtime_date(file) {
        Some(d) => (Some(d), DateSource::Mtime),
        None => (None, DateSource::None),
    }
}

pub fn scan(media_root: &Path) -> Scan {
    let mut scan = Scan::default();
    for show_dir in subdirs(&media_root.join("tv")) {
        let show_id = ids::make("s", media_root, &show_dir);
        let mut episodes: Vec<Episode> = Vec::new();
        let walker = WalkDir::new(&show_dir)
            .max_depth(3)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|e| {
                let n = e.file_name().to_string_lossy().to_lowercase();
                !(e.depth() > 0
                    && e.file_type().is_dir()
                    && matches!(n.as_str(), "extras" | "featurettes" | "sample" | "samples"))
            });
        for entry in walker.filter_map(Result::ok) {
            let path = entry.path();
            if !entry.file_type().is_file() || !is_video(path) {
                continue;
            }
            let stem = file_stem(path);
            if stem.to_lowercase().contains("sample") {
                continue;
            }
            let Some((season, episode, rest)) = parse_episode(&stem) else {
                scan.warnings.push(format!(
                    "no SxxExx in episode file name, skipped: {}",
                    path.display()
                ));
                continue;
            };
            let (aired, source) = episode_date(path, &stem);
            let title = if rest.is_empty() {
                format!("Episode {episode}")
            } else {
                rest.chars().take(120).collect()
            };
            episodes.push(Episode {
                id: ids::make("e", media_root, path),
                item_id: show_id.clone(),
                season,
                episode,
                title,
                aired,
                date_source: source,
                path: path.to_path_buf(),
                art: episode_art(path),
            });
        }
        if episodes.is_empty() {
            scan.warnings.push(format!(
                "show folder without episodes: {}",
                show_dir.display()
            ));
            continue;
        }
        episodes.sort_by_key(|e| (e.season, e.episode));
        let newest = newest_episode(&episodes);
        scan.entries.push(MediaEntry {
            id: show_id,
            kind: Kind::Show,
            title: file_name(&show_dir),
            date: newest.and_then(|e| e.aired),
            date_source: newest.map_or(DateSource::None, |e| e.date_source),
            path: show_dir.clone(),
            system: None,
            art: find_art(&show_dir),
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
    fn parses_non_standard_scene_names() {
        // 2xNN / 2X15
        assert_eq!(
            parse_episode("Servant.2x10.Josephine").map(|t| (t.0, t.1)),
            Some((2, 10))
        );
        assert_eq!(
            parse_episode("The Pitt 2X15").map(|t| (t.0, t.1)),
            Some((2, 15))
        );
        // S2 - 11
        assert_eq!(
            parse_episode("[SubsPlease] Goblin Slayer S2 - 11 (720p)").map(|t| (t.0, t.1)),
            Some((2, 11))
        );
        // E05 single season
        assert_eq!(
            parse_episode("Bubblegum.Crisis.E05.1080p.BluRay").map(|t| (t.0, t.1)),
            Some((1, 5))
        );
        // compact S01Exx directly after the show name
        assert_eq!(
            parse_episode("unveil-suits0112-1080p").map(|t| (t.0, t.1)),
            Some((1, 12))
        );
        assert_eq!(
            parse_episode("unveil-suits0107-1080p").map(|t| (t.0, t.1)),
            Some((1, 7))
        );
        // a 4-digit year must never match the compact form
        assert!(parse_episode("Something2012").is_none());
    }

    #[test]
    fn parses_sonarr_style_names() {
        let (s, e, rest) =
            parse_episode("Andor - S02E03 - Welcome to the Rebellion WEBDL-1080p").unwrap();
        assert_eq!((s, e), (2, 3));
        assert_eq!(rest, "Welcome to the Rebellion WEBDL-1080p");
        assert_eq!(
            parse_episode("show.s10e100.mkv").map(|t| (t.0, t.1)),
            Some((10, 100))
        );
        assert!(parse_episode("Movie Trailer 1080p").is_none());
        assert!(
            parse_episode("Classics01E02").is_none(),
            "no match inside a word"
        );
    }

    #[test]
    fn scans_shows_with_season_folders_and_hebrew_names() {
        let m = tempfile::tempdir().unwrap();
        let a = m.path().join("tv/Andor/Season 2");
        let h = m.path().join("tv/12 שעות");
        create_dir_all(&a).unwrap();
        create_dir_all(&h).unwrap();
        write(a.join("Andor - S02E01 - One.mkv"), b"v").unwrap();
        write(a.join("Andor - S02E02 - Two.mkv"), b"v").unwrap();
        write(
            a.join("Andor - S02E02 - Two.nfo"),
            "<episodedetails><aired>2025-04-29</aired></episodedetails>",
        )
        .unwrap();
        write(h.join("12 שעות S01E01.mp4"), b"v").unwrap();
        let s = scan(m.path());
        assert_eq!(s.entries.len(), 2);
        assert_eq!(s.episodes.len(), 3);
        let andor = s.entries.iter().find(|e| e.title == "Andor").unwrap();
        assert_eq!(
            andor.date,
            NaiveDate::from_ymd_opt(2025, 4, 29),
            "newest dated episode wins"
        );
        assert_eq!(andor.date_source, DateSource::Nfo);
        assert!(s.entries.iter().any(|e| e.title == "12 שעות"));
        let one = s.episodes.iter().find(|e| e.title == "One").unwrap();
        assert_eq!(
            one.date_source,
            DateSource::Mtime,
            "no NFO and no date in the name: arrival time"
        );
    }

    #[test]
    fn arrival_time_only_decides_when_no_episode_has_an_air_date() {
        let m = tempfile::tempdir().unwrap();
        let aired = m.path().join("tv/Aired");
        let arrived = m.path().join("tv/Arrived");
        create_dir_all(&aired).unwrap();
        create_dir_all(&arrived).unwrap();
        // One dated old episode plus one undated file that "arrived" today.
        write(aired.join("Aired S01E01.mkv"), b"v").unwrap();
        write(
            aired.join("Aired S01E01.nfo"),
            "<episodedetails><aired>2001-02-03</aired></episodedetails>",
        )
        .unwrap();
        write(aired.join("Aired S01E02.mkv"), b"v").unwrap();
        write(arrived.join("Arrived S01E01.mkv"), b"v").unwrap();
        let s = scan(m.path());
        let by = |t: &str| s.entries.iter().find(|e| e.title == t).unwrap();
        assert_eq!(
            (by("Aired").date, by("Aired").date_source),
            (NaiveDate::from_ymd_opt(2001, 2, 3), DateSource::Nfo)
        );
        assert_eq!(
            by("Arrived").date_source,
            DateSource::Mtime,
            "no real date anywhere: fall back to arrival"
        );
    }

    #[test]
    fn date_based_episode_names_use_the_date_in_the_name() {
        let (d, src) = episode_date(
            Path::new("/nonexistent/Show S01E01 2024-03-05 Title.mkv"),
            "Show S01E01 2024-03-05 Title",
        );
        assert_eq!(
            (d, src),
            (NaiveDate::from_ymd_opt(2024, 3, 5), DateSource::Nfo)
        );
    }

    #[test]
    fn skips_extras_samples_and_unnumbered_files_with_warnings() {
        let m = tempfile::tempdir().unwrap();
        let d = m.path().join("tv/Show");
        create_dir_all(d.join("Extras")).unwrap();
        write(d.join("Extras/S01E99 behind the scenes.mkv"), b"v").unwrap();
        write(d.join("Show S01E01.mkv"), b"v").unwrap();
        write(d.join("random video.mkv"), b"v").unwrap();
        write(d.join("Show S01E02 sample.mkv"), b"v").unwrap();
        let s = scan(m.path());
        assert_eq!(s.episodes.len(), 1);
        assert_eq!(s.warnings.len(), 1);
    }

    #[test]
    fn a_show_with_no_episodes_is_not_listed() {
        let m = tempfile::tempdir().unwrap();
        create_dir_all(m.path().join("tv/Empty")).unwrap();
        let s = scan(m.path());
        assert!(s.entries.is_empty());
        assert_eq!(s.warnings.len(), 1);
    }
}
