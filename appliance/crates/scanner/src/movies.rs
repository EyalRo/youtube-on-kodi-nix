use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use catalog::{DateSource, Kind, MediaEntry, parse_release};
use chrono::NaiveDate;
use regex::Regex;

use crate::util::{file_name, file_stem, find_art, is_video, nfo_tag, subdirs};
use crate::{Scan, ids};

static FOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?P<title>.+?)\s*\((?P<year>\d{4})\)\s*$").unwrap());

/// `Alien (1979)` -> ("Alien", Some(1979)); anything else keeps its whole name.
fn split_title(name: &str) -> (String, Option<i32>) {
    match FOLDER.captures(name) {
        Some(c) => (c["title"].to_string(), c["year"].parse().ok()),
        None => (name.to_string(), None),
    }
}

/// The main feature: the largest video file that is not a sample or trailer.
fn main_video(dir: &Path) -> Option<std::path::PathBuf> {
    let mut best: Option<(u64, std::path::PathBuf)> = None;
    for entry in fs::read_dir(dir).ok()?.filter_map(Result::ok) {
        let path = entry.path();
        let name = file_name(&path).to_lowercase();
        if !path.is_file()
            || !is_video(&path)
            || name.contains("sample")
            || name.contains("trailer")
        {
            continue;
        }
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        if best.as_ref().is_none_or(|(s, _)| size > *s) {
            best = Some((size, path));
        }
    }
    best.map(|(_, p)| p)
}

fn nfo_date(dir: &Path) -> Option<NaiveDate> {
    let mut nfos: Vec<_> = fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    nfos.sort();
    nfos.iter()
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("nfo")))
        .find_map(|p| {
            nfo_tag(p, &["premiered", "releasedate", "year"]).and_then(|v| parse_release(&v))
        })
}

pub fn scan(media_root: &Path) -> Scan {
    let mut scan = Scan::default();
    let root = media_root.join("movies");
    for dir in subdirs(&root) {
        let name = file_name(&dir);
        let Some(video) = main_video(&dir) else {
            scan.warnings.push(format!(
                "movie folder without a video file: {}",
                dir.display()
            ));
            continue;
        };
        let (title, year) = split_title(&name);
        let (date, source) = if let Some(d) = nfo_date(&dir) {
            (Some(d), DateSource::Nfo)
        } else if let Some(d) = year.and_then(|y| NaiveDate::from_ymd_opt(y, 1, 1)) {
            (Some(d), DateSource::Year)
        } else {
            scan.warnings
                .push(format!("movie without a date: {}", dir.display()));
            (None, DateSource::None)
        };
        scan.entries.push(MediaEntry {
            id: ids::make("m", media_root, &dir),
            kind: Kind::Movie,
            title,
            date,
            date_source: source,
            path: video,
            system: None,
            art: find_art(&dir),
        });
    }
    // Loose files directly in movies/ are movies too.
    if let Ok(read) = fs::read_dir(&root) {
        let mut files: Vec<_> = read
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_file() && is_video(p))
            .collect();
        files.sort();
        for file in files {
            let (title, year) = split_title(&file_stem(&file));
            let date = year.and_then(|y| NaiveDate::from_ymd_opt(y, 1, 1));
            scan.entries.push(MediaEntry {
                id: ids::make("m", media_root, &file),
                kind: Kind::Movie,
                title,
                date,
                date_source: if date.is_some() {
                    DateSource::Year
                } else {
                    DateSource::None
                },
                path: file,
                system: None,
                art: None,
            });
        }
    }
    scan
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{create_dir_all, write};

    fn media() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn folder_name_gives_title_and_year() {
        assert_eq!(
            split_title("Alien (1979)"),
            ("Alien".to_string(), Some(1979))
        );
        assert_eq!(
            split_title("Alien³ (1992)"),
            ("Alien³".to_string(), Some(1992))
        );
        assert_eq!(split_title("No Year"), ("No Year".to_string(), None));
    }

    #[test]
    fn scans_movie_folders_with_year_dates_and_picks_the_main_file() {
        let m = media();
        let d = m.path().join("movies/Alien (1979)");
        create_dir_all(&d).unwrap();
        write(d.join("Alien.1979.mkv"), vec![0u8; 5000]).unwrap();
        write(d.join("Alien.sample.mkv"), vec![0u8; 100]).unwrap();
        write(d.join("poster.jpg"), b"x").unwrap();
        let scan = scan(m.path());
        assert_eq!(scan.entries.len(), 1);
        let e = &scan.entries[0];
        assert_eq!((e.title.as_str(), e.kind), ("Alien", Kind::Movie));
        assert_eq!(e.date, NaiveDate::from_ymd_opt(1979, 1, 1));
        assert_eq!(e.date_source, DateSource::Year);
        assert!(e.path.ends_with("Alien.1979.mkv"));
        assert!(e.art.as_ref().unwrap().ends_with("poster.jpg"));
    }

    #[test]
    fn an_nfo_date_beats_the_folder_year() {
        let m = media();
        let d = m.path().join("movies/Heat (1995)");
        create_dir_all(&d).unwrap();
        write(d.join("Heat.mkv"), b"v").unwrap();
        write(
            d.join("movie.nfo"),
            "<movie><premiered>1995-12-15</premiered></movie>",
        )
        .unwrap();
        let e = scan(m.path()).entries.remove(0);
        assert_eq!(e.date, NaiveDate::from_ymd_opt(1995, 12, 15));
        assert_eq!(e.date_source, DateSource::Nfo);
    }

    #[test]
    fn folders_without_video_are_reported_not_listed() {
        let m = media();
        create_dir_all(m.path().join("movies/Empty (2000)")).unwrap();
        let s = scan(m.path());
        assert!(s.entries.is_empty());
        assert_eq!(s.warnings.len(), 1);
    }

    #[test]
    fn ids_are_stable_and_path_based() {
        let m = media();
        let d = m.path().join("movies/Alien (1979)");
        create_dir_all(&d).unwrap();
        write(d.join("a.mkv"), b"v").unwrap();
        assert_eq!(scan(m.path()).entries[0].id, scan(m.path()).entries[0].id);
    }
}
