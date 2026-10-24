//! Import watched flags and resume points from Kodi's `MyVideos*.db`.
//!
//! The Kodi database is opened read-only. Kodi stores where a file was played from
//! (`nfs://nas/volume1/media/movies/...`), so the caller maps those prefixes to the local
//! library root. Existing appliance state is never overwritten, so the import is safe to
//! repeat.

use std::path::Path;

use chrono::NaiveDateTime;
use rusqlite::{Connection, OpenFlags};
use store::{Store, StoreError};

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("cannot read the Kodi database: {0}")]
    Kodi(#[from] rusqlite::Error),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// A path prefix Kodi used, and where the same files live now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping {
    pub from: String,
    pub to: String,
}

impl Mapping {
    /// Parse `FROM=TO`.
    pub fn parse(s: &str) -> Option<Mapping> {
        let (from, to) = s.split_once('=')?;
        (!from.is_empty() && !to.is_empty()).then(|| Mapping {
            from: from.into(),
            to: to.into(),
        })
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Files Kodi had state for.
    pub seen: usize,
    pub imported: usize,
    /// The appliance already had state for these.
    pub already_known: usize,
    /// Not in the library (plugin URLs, other storage, files since removed).
    pub unmatched: usize,
    /// A few unmatched paths, for the log.
    pub unmatched_sample: Vec<String>,
}

fn map_path(path: &str, maps: &[Mapping]) -> String {
    maps.iter()
        .find_map(|m| {
            path.strip_prefix(&m.from)
                .map(|rest| format!("{}{}", m.to, rest))
        })
        .unwrap_or_else(|| path.to_string())
}

fn to_epoch(s: &str) -> Option<i64> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|d| d.and_utc().timestamp())
}

pub fn import(kodi_db: &Path, store: &mut Store, maps: &[Mapping]) -> Result<Report, ImportError> {
    let conn = Connection::open_with_flags(kodi_db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    // type 1 is a resume bookmark (2 is an episode bookmark; 0 a standard one we do not use).
    let mut stmt = conn.prepare(
        "SELECT p.strPath || f.strFilename, COALESCE(f.playCount, 0), f.lastPlayed,
                b.timeInSeconds, b.totalTimeInSeconds
         FROM files f
         JOIN path p ON p.idPath = f.idPath
         LEFT JOIN bookmark b ON b.idFile = f.idFile AND b.type = 1
         WHERE COALESCE(f.playCount, 0) > 0 OR b.idFile IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<f64>>(3)?,
            r.get::<_, Option<f64>>(4)?,
        ))
    })?;
    let mut report = Report::default();
    for row in rows {
        let (path, play_count, last_played, position, total) = row?;
        report.seen += 1;
        let local = map_path(&path, maps);
        let Some(id) = store.id_by_path(&local)? else {
            report.unmatched += 1;
            if report.unmatched_sample.len() < 5 {
                report.unmatched_sample.push(path);
            }
            continue;
        };
        let watched = play_count > 0;
        let written = store.import_state(
            &id,
            position.unwrap_or(0.0),
            total.unwrap_or(0.0),
            watched,
            last_played.as_deref().and_then(to_epoch),
        )?;
        if written {
            report.imported += 1;
        } else {
            report.already_known += 1;
        }
    }
    tracing::info!(
        seen = report.seen,
        imported = report.imported,
        already_known = report.already_known,
        unmatched = report.unmatched,
        "kodi import complete"
    );
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use catalog::{DateSource, Episode, Kind, MediaEntry};
    use scanner::Scan;
    use std::path::PathBuf;

    fn kodi_db(dir: &Path) -> PathBuf {
        let path = dir.join("MyVideos131.db");
        let c = Connection::open(&path).unwrap();
        c.execute_batch(
            "CREATE TABLE path (idPath INTEGER PRIMARY KEY, strPath TEXT);
             CREATE TABLE files (idFile INTEGER PRIMARY KEY, idPath INTEGER, strFilename TEXT, playCount INTEGER, lastPlayed TEXT);
             CREATE TABLE bookmark (idBookmark INTEGER PRIMARY KEY, idFile INTEGER, timeInSeconds REAL, totalTimeInSeconds REAL, type INTEGER);
             INSERT INTO path VALUES (1, 'nfs://nas/media/movies/Alien (1979)/'), (2, 'nfs://nas/media/tv/Show/'), (3, 'plugin://plugin.video.myyoutubers/');
             INSERT INTO files VALUES
               (1, 1, 'Alien (1979).mkv', 1, '2024-05-01 20:00:00'),
               (2, 1, 'Other.mkv', NULL, NULL),
               (3, 2, 'Show S01E01.mkv', NULL, NULL),
               (4, 2, 'Show S01E02.mkv', 2, NULL),
               (5, 3, '?mode=play&id=abc', 1, NULL),
               (6, 2, 'Gone S09E09.mkv', 1, NULL);
             INSERT INTO bookmark VALUES (1, 3, 600, 1200, 1), (2, 2, 50, 100, 1), (3, 4, 5, 100, 0);",
        )
        .unwrap();
        path
    }

    fn library() -> Store {
        let entry = |id: &str, kind, path: &str| MediaEntry {
            id: id.into(),
            kind,
            title: id.into(),
            date: None,
            date_source: DateSource::None,
            path: path.into(),
            system: None,
            art: None,
        };
        let ep = |id: &str, n, path: &str| Episode {
            id: id.into(),
            item_id: "s-1".into(),
            season: 1,
            episode: n,
            title: id.into(),
            aired: None,
            date_source: DateSource::None,
            path: path.into(),
            art: None,
        };
        let mut s = Store::open_in_memory().unwrap();
        s.apply_scan(&Scan {
            entries: vec![
                entry(
                    "m-1",
                    Kind::Movie,
                    "/media/movies/Alien (1979)/Alien (1979).mkv",
                ),
                entry("s-1", Kind::Show, "/media/tv/Show"),
            ],
            episodes: vec![
                ep("e-1", 1, "/media/tv/Show/Show S01E01.mkv"),
                ep("e-2", 2, "/media/tv/Show/Show S01E02.mkv"),
            ],
            warnings: vec![],
        })
        .unwrap();
        s
    }

    fn maps() -> Vec<Mapping> {
        vec![Mapping::parse("nfs://nas/media/=/media/").unwrap()]
    }

    #[test]
    fn imports_watched_and_resume_and_counts_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let db = kodi_db(dir.path());
        let mut s = library();
        let r = import(&db, &mut s, &maps()).unwrap();
        assert_eq!(
            (r.seen, r.imported, r.already_known, r.unmatched),
            (6, 3, 0, 3)
        );
        assert!(
            r.unmatched_sample
                .iter()
                .any(|p| p.starts_with("plugin://"))
        );
        // Watched movie: no resume. Show: episode 1 has a resume point, episode 2 is watched.
        assert_eq!(s.play_info("m-1").unwrap().unwrap().resume_position, 0.0);
        let p = s.play_info("s-1").unwrap().unwrap();
        assert_eq!((p.id.as_str(), p.resume_position), ("e-1", 600.0));
        assert_eq!(s.item("s-1").unwrap().unwrap().watched, 1);
    }

    #[test]
    fn rerunning_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let db = kodi_db(dir.path());
        let mut s = library();
        import(&db, &mut s, &maps()).unwrap();
        s.save_progress("e-1", 900.0, 1200.0).unwrap();
        let r = import(&db, &mut s, &maps()).unwrap();
        assert_eq!((r.imported, r.already_known), (0, 3));
        assert_eq!(s.play_info("e-1").unwrap().unwrap().resume_position, 900.0);
    }

    #[test]
    fn mapping_parse() {
        assert_eq!(
            Mapping::parse("a=b"),
            Some(Mapping {
                from: "a".into(),
                to: "b".into()
            })
        );
        assert_eq!(Mapping::parse("nope"), None);
        assert_eq!(Mapping::parse("=x"), None);
    }

    #[test]
    fn missing_database_is_an_error() {
        let mut s = library();
        assert!(import(Path::new("/nonexistent/MyVideos.db"), &mut s, &[]).is_err());
    }
}
