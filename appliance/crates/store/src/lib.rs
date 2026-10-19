//! The appliance's one source of truth: catalog entries, episodes and watch state.
//!
//! Watch state is keyed by the id of a movie, game or episode and is deliberately
//! **not** removed when a rescan stops seeing the file: an unmounted NFS share must
//! never wipe history.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use catalog::{DateSource, Item, Kind, system_by_id};
use chrono::NaiveDate;
use rusqlite::{Connection, OptionalExtension, params};
use scanner::Scan;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS entries (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    title TEXT NOT NULL,
    sort_date TEXT,
    date_source TEXT NOT NULL,
    path TEXT NOT NULL,
    system TEXT,
    art TEXT
);
CREATE TABLE IF NOT EXISTS episodes (
    id TEXT PRIMARY KEY,
    item_id TEXT NOT NULL,
    season INTEGER NOT NULL,
    episode INTEGER NOT NULL,
    title TEXT NOT NULL,
    aired TEXT,
    date_source TEXT NOT NULL,
    path TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS episodes_item ON episodes(item_id);
CREATE TABLE IF NOT EXISTS play_state (
    id TEXT PRIMARY KEY,
    position REAL NOT NULL DEFAULT 0,
    duration REAL NOT NULL DEFAULT 0,
    watched INTEGER NOT NULL DEFAULT 0,
    last_played INTEGER
);
";

/// A position counts as 'watched' at this fraction of the duration.
const WATCHED_FRACTION: f64 = 0.9;
/// Resume only past the first 30 seconds and before the last 30.
const RESUME_MARGIN: f64 = 30.0;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error(
        "the scan found nothing but the library has {existing} entries; refusing to wipe it (is /media mounted?)"
    )]
    EmptyScan { existing: usize },
    #[error("no such item or episode: {0}")]
    NotFound(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanStats {
    pub added: usize,
    pub removed: usize,
    pub entries: usize,
    pub episodes: usize,
}

/// One episode with its watch state, for a show or channel page.
#[derive(Debug, Clone, PartialEq)]
pub struct EpisodeView {
    pub id: String,
    pub season: i32,
    pub episode: i32,
    pub title: String,
    pub aired: Option<NaiveDate>,
    pub date_source: DateSource,
    pub watched: bool,
    pub position: f64,
    pub duration: f64,
}

/// What the player needs to start something.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayInfo {
    /// The id of the movie, game or episode that will play.
    pub id: String,
    pub kind: &'static str,
    pub title: String,
    pub path: PathBuf,
    pub resume_position: f64,
    pub system: Option<String>,
}

pub struct Store {
    conn: Connection,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn date(s: Option<String>) -> Option<NaiveDate> {
    s.and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok())
}

fn resume_of(position: f64, duration: f64, watched: bool) -> f64 {
    let enough =
        position >= RESUME_MARGIN && (duration <= 0.0 || position <= duration - RESUME_MARGIN);
    if watched || !enough { 0.0 } else { position }
}

impl Store {
    pub fn open(path: &Path) -> Result<Store, StoreError> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Store, StoreError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Store, StoreError> {
        conn.execute_batch(SCHEMA)?;
        Ok(Store { conn })
    }

    /// Replace the catalog with a scan's result. Watch state is untouched.
    pub fn apply_scan(&mut self, scan: &Scan) -> Result<ScanStats, StoreError> {
        let tx = self.conn.transaction()?;
        let before: HashSet<String> = {
            let mut stmt = tx.prepare("SELECT id FROM entries")?;
            stmt.query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<_, _>>()?
        };
        if scan.entries.is_empty() && !before.is_empty() {
            return Err(StoreError::EmptyScan {
                existing: before.len(),
            });
        }
        let now_ids: HashSet<&str> = scan.entries.iter().map(|e| e.id.as_str()).collect();
        let mut removed = 0;
        for id in before.iter().filter(|id| !now_ids.contains(id.as_str())) {
            tx.execute("DELETE FROM entries WHERE id = ?1", params![id])?;
            removed += 1;
        }
        let mut added = 0;
        for e in &scan.entries {
            if !before.contains(&e.id) {
                added += 1;
            }
            tx.execute(
                "INSERT INTO entries (id, kind, title, sort_date, date_source, path, system, art)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(id) DO UPDATE SET kind=?2, title=?3, sort_date=?4, date_source=?5, path=?6, system=?7, art=?8",
                params![
                    e.id,
                    e.kind.as_str(),
                    e.title,
                    e.date.map(|d| d.to_string()),
                    e.date_source.as_str(),
                    e.path.to_string_lossy(),
                    e.system,
                    e.art.as_ref().map(|a| a.to_string_lossy().into_owned()),
                ],
            )?;
        }
        tx.execute("DELETE FROM episodes", [])?;
        for ep in &scan.episodes {
            tx.execute(
                "INSERT INTO episodes (id, item_id, season, episode, title, aired, date_source, path)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    ep.id,
                    ep.item_id,
                    ep.season,
                    ep.episode,
                    ep.title,
                    ep.aired.map(|d| d.to_string()),
                    ep.date_source.as_str(),
                    ep.path.to_string_lossy(),
                ],
            )?;
        }
        tx.commit()?;
        Ok(ScanStats {
            added,
            removed,
            entries: scan.entries.len(),
            episodes: scan.episodes.len(),
        })
    }

    /// Every entry as the row engine sees it, with watch state folded in.
    pub fn items(&self) -> Result<Vec<Item>, StoreError> {
        // Per show/channel: episodes watched, and the newest still-unwatched air date.
        let mut agg: HashMap<String, (u32, Option<NaiveDate>)> = HashMap::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT e.item_id,
                        COALESCE(SUM(ps.watched), 0),
                        MAX(CASE WHEN COALESCE(ps.watched, 0) = 0 THEN e.aired END)
                 FROM episodes e LEFT JOIN play_state ps ON ps.id = e.id
                 GROUP BY e.item_id",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, u32>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })?;
            for row in rows {
                let (id, watched, unwatched) = row?;
                agg.insert(id, (watched, date(unwatched)));
            }
        }
        let mut stmt = self
            .conn
            .prepare("SELECT id, kind, title, sort_date, date_source, system, art FROM entries")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })?;
        let mut items = Vec::new();
        for row in rows {
            let (id, kind, title, sort_date, source, system, art) = row?;
            let Some(kind) = Kind::parse(&kind) else {
                continue;
            };
            let (watched, unwatched_date) = agg.get(&id).copied().unwrap_or((0, None));
            let label2 = match kind {
                Kind::Movie => "Movie".to_string(),
                Kind::Show => "TV show".to_string(),
                Kind::Channel => "YouTube".to_string(),
                Kind::Game => system
                    .as_deref()
                    .and_then(system_by_id)
                    .map_or_else(|| "Game".to_string(), |s| s.name.to_string()),
            };
            items.push(Item {
                id,
                kind,
                title,
                date: date(sort_date),
                date_source: DateSource::parse(&source),
                label2,
                system,
                art,
                watched,
                unwatched_date,
            });
        }
        Ok(items)
    }

    pub fn item(&self, id: &str) -> Result<Option<Item>, StoreError> {
        Ok(self.items()?.into_iter().find(|i| i.id == id))
    }

    /// Episodes of a show (season, episode order) or channel (newest first), with watch state.
    pub fn episodes_of(&self, item_id: &str) -> Result<Vec<EpisodeView>, StoreError> {
        let channel = self
            .conn
            .query_row(
                "SELECT kind FROM entries WHERE id = ?1",
                params![item_id],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .is_some_and(|k| k == "channel");
        let order = if channel {
            "e.aired IS NULL, e.aired DESC, e.episode DESC"
        } else {
            "e.season, e.episode"
        };
        let mut stmt = self.conn.prepare(&format!(
            "SELECT e.id, e.season, e.episode, e.title, e.aired, e.date_source,
                    COALESCE(ps.watched, 0), COALESCE(ps.position, 0), COALESCE(ps.duration, 0)
             FROM episodes e LEFT JOIN play_state ps ON ps.id = e.id
             WHERE e.item_id = ?1 ORDER BY {order}"
        ))?;
        let rows = stmt.query_map(params![item_id], |r| {
            Ok(EpisodeView {
                id: r.get(0)?,
                season: r.get(1)?,
                episode: r.get(2)?,
                title: r.get(3)?,
                aired: date(r.get(4)?),
                date_source: DateSource::parse(&r.get::<_, String>(5)?),
                watched: r.get::<_, i64>(6)? != 0,
                position: r.get(7)?,
                duration: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    fn state(&self, id: &str) -> Result<(f64, f64, bool), StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT position, duration, watched FROM play_state WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)),
            )
            .optional()?
            .unwrap_or((0.0, 0.0, false)))
    }

    /// What to play for an id: a movie, game or episode directly; for a show or
    /// channel, the episode to continue with.
    pub fn play_info(&self, id: &str) -> Result<Option<PlayInfo>, StoreError> {
        let entry = self
            .conn
            .query_row(
                "SELECT kind, title, path, system FROM entries WHERE id = ?1",
                params![id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()?;
        if let Some((kind, title, path, system)) = entry {
            return match Kind::parse(&kind) {
                Some(Kind::Movie) | Some(Kind::Game) => {
                    let (pos, dur, watched) = self.state(id)?;
                    let k = if kind == "game" { "game" } else { "movie" };
                    Ok(Some(PlayInfo {
                        id: id.to_string(),
                        kind: k,
                        title,
                        path: PathBuf::from(path),
                        resume_position: resume_of(pos, dur, watched),
                        system,
                    }))
                }
                Some(Kind::Show) | Some(Kind::Channel) => self.next_episode(id),
                None => Ok(None),
            };
        }
        let ep = self
            .conn
            .query_row(
                "SELECT title, path FROM episodes WHERE id = ?1",
                params![id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;
        Ok(ep.map(|(title, path)| {
            let (pos, dur, watched) = self.state(id).unwrap_or((0.0, 0.0, false));
            PlayInfo {
                id: id.to_string(),
                kind: "episode",
                title,
                path: PathBuf::from(path),
                resume_position: resume_of(pos, dur, watched),
                system: None,
            }
        }))
    }

    /// An in-progress episode (most recently played) first; otherwise the first unwatched in
    /// `episodes_of` order (a show: the earliest; a channel: the newest); otherwise the first,
    /// to replay.
    fn next_episode(&self, item_id: &str) -> Result<Option<PlayInfo>, StoreError> {
        let episodes = self.episodes_of(item_id)?;
        let last_played = |id: &str| -> i64 {
            self.conn
                .query_row(
                    "SELECT COALESCE(last_played, 0) FROM play_state WHERE id = ?1",
                    params![id],
                    |r| r.get(0),
                )
                .unwrap_or(0)
        };
        let in_progress = episodes
            .iter()
            .filter(|e| !e.watched && resume_of(e.position, e.duration, false) > 0.0)
            .max_by_key(|e| last_played(&e.id));
        let pick = in_progress
            .or_else(|| episodes.iter().find(|e| !e.watched))
            .or(episodes.first());
        match pick {
            Some(e) => self.play_info(&e.id),
            None => Ok(None),
        }
    }

    /// Record how far playback got. At 90% the item counts as watched and resume resets.
    pub fn save_progress(
        &mut self,
        id: &str,
        position: f64,
        duration: f64,
    ) -> Result<(), StoreError> {
        let known: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM entries WHERE id = ?1 AND kind IN ('movie','game')) OR EXISTS(SELECT 1 FROM episodes WHERE id = ?1)",
            params![id],
            |r| r.get(0),
        )?;
        if !known {
            return Err(StoreError::NotFound(id.to_string()));
        }
        let watched = duration > 0.0 && position / duration >= WATCHED_FRACTION;
        let position = if watched { 0.0 } else { position.max(0.0) };
        self.conn.execute(
            "INSERT INTO play_state (id, position, duration, watched, last_played) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET position=?2, duration=?3, watched=MAX(watched, ?4), last_played=?5",
            params![id, position, duration, watched as i64, now()],
        )?;
        Ok(())
    }

    /// Note that a game (or anything without a position) was started.
    pub fn mark_played(&mut self, id: &str) -> Result<(), StoreError> {
        self.save_progress(id, 0.0, 0.0)
    }

    pub fn count(&self) -> Result<usize, StoreError> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM entries", [], |r| r.get::<_, i64>(0))?
            as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use catalog::Episode;
    use catalog::MediaEntry;

    fn d(s: &str) -> Option<NaiveDate> {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
    }

    fn entry(id: &str, kind: Kind, title: &str, date: Option<&str>) -> MediaEntry {
        MediaEntry {
            id: id.into(),
            kind,
            title: title.into(),
            date: date.and_then(d),
            date_source: DateSource::Nfo,
            path: PathBuf::from(format!("/media/{id}")),
            system: None,
            art: None,
        }
    }

    fn ep(id: &str, item: &str, season: i32, episode: i32, aired: &str) -> Episode {
        Episode {
            id: id.into(),
            item_id: item.into(),
            season,
            episode,
            title: id.into(),
            aired: d(aired),
            date_source: DateSource::Nfo,
            path: PathBuf::from(format!("/media/{id}.mkv")),
        }
    }

    fn scan() -> Scan {
        Scan {
            entries: vec![
                entry("m-1", Kind::Movie, "Alien", Some("1979-05-25")),
                entry("s-1", Kind::Show, "Show", Some("2024-03-02")),
                entry("c-1", Kind::Channel, "Chan", Some("2024-05-01")),
            ],
            episodes: vec![
                ep("e-1", "s-1", 1, 1, "2024-01-01"),
                ep("e-2", "s-1", 1, 2, "2024-02-01"),
                ep("e-3", "s-1", 1, 3, "2024-03-02"),
                ep("e-c1", "c-1", 2024, 101, "2024-04-01"),
                ep("e-c2", "c-1", 2024, 201, "2024-05-01"),
            ],
            warnings: vec![],
        }
    }

    fn store() -> Store {
        let mut s = Store::open_in_memory().unwrap();
        s.apply_scan(&scan()).unwrap();
        s
    }

    #[test]
    fn apply_scan_counts_and_items() {
        let mut s = Store::open_in_memory().unwrap();
        let stats = s.apply_scan(&scan()).unwrap();
        assert_eq!(
            stats,
            ScanStats {
                added: 3,
                removed: 0,
                entries: 3,
                episodes: 5
            }
        );
        assert_eq!(s.count().unwrap(), 3);
        let show = s.item("s-1").unwrap().unwrap();
        assert_eq!(show.label2, "TV show");
        assert_eq!(show.unwatched_date, d("2024-03-02"));
        assert_eq!(s.item("m-1").unwrap().unwrap().label2, "Movie");
    }

    #[test]
    fn empty_scan_is_refused_when_store_is_populated() {
        let mut s = store();
        let err = s
            .apply_scan(&Scan {
                entries: vec![],
                episodes: vec![],
                warnings: vec![],
            })
            .unwrap_err();
        assert!(matches!(err, StoreError::EmptyScan { existing: 3 }));
        assert_eq!(s.count().unwrap(), 3);
        // An empty scan into an empty store is fine.
        let mut fresh = Store::open_in_memory().unwrap();
        fresh
            .apply_scan(&Scan {
                entries: vec![],
                episodes: vec![],
                warnings: vec![],
            })
            .unwrap();
    }

    #[test]
    fn rescan_removes_vanished_entries_but_keeps_watch_state() {
        let mut s = store();
        s.save_progress("m-1", 100.0, 1000.0).unwrap();
        let mut next = scan();
        next.entries.retain(|e| e.id != "m-1");
        let stats = s.apply_scan(&next).unwrap();
        assert_eq!(stats.removed, 1);
        assert!(s.item("m-1").unwrap().is_none());
        // The file comes back: the resume point is still there.
        s.apply_scan(&scan()).unwrap();
        assert_eq!(s.play_info("m-1").unwrap().unwrap().resume_position, 100.0);
    }

    #[test]
    fn progress_thresholds() {
        let mut s = store();
        s.save_progress("m-1", 10.0, 1000.0).unwrap();
        assert_eq!(
            s.play_info("m-1").unwrap().unwrap().resume_position,
            0.0,
            "under 30s does not resume"
        );
        s.save_progress("m-1", 500.0, 1000.0).unwrap();
        assert_eq!(s.play_info("m-1").unwrap().unwrap().resume_position, 500.0);
        s.save_progress("m-1", 980.0, 1000.0).unwrap();
        assert_eq!(s.play_info("m-1").unwrap().unwrap().resume_position, 0.0);
        s.save_progress("m-1", 950.0, 1000.0).unwrap();
        assert_eq!(
            s.play_info("m-1").unwrap().unwrap().resume_position,
            0.0,
            "watched stays watched"
        );
    }

    #[test]
    fn unknown_ids() {
        let mut s = store();
        assert!(matches!(
            s.save_progress("nope", 1.0, 2.0),
            Err(StoreError::NotFound(_))
        ));
        assert!(
            matches!(
                s.save_progress("s-1", 1.0, 2.0),
                Err(StoreError::NotFound(_))
            ),
            "shows have no position"
        );
        assert!(s.play_info("nope").unwrap().is_none());
    }

    #[test]
    fn show_plays_earliest_unwatched_channel_plays_newest() {
        let mut s = store();
        assert_eq!(s.play_info("s-1").unwrap().unwrap().id, "e-1");
        assert_eq!(s.play_info("c-1").unwrap().unwrap().id, "e-c2");
        s.save_progress("e-1", 1000.0, 1000.0).unwrap();
        s.save_progress("e-c2", 1000.0, 1000.0).unwrap();
        assert_eq!(s.play_info("s-1").unwrap().unwrap().id, "e-2");
        assert_eq!(s.play_info("c-1").unwrap().unwrap().id, "e-c1");
    }

    #[test]
    fn in_progress_episode_wins_and_resumes() {
        let mut s = store();
        s.save_progress("e-3", 600.0, 1200.0).unwrap();
        let p = s.play_info("s-1").unwrap().unwrap();
        assert_eq!(
            (p.id.as_str(), p.resume_position, p.kind),
            ("e-3", 600.0, "episode")
        );
    }

    #[test]
    fn fully_watched_show_replays_from_the_start() {
        let mut s = store();
        for id in ["e-1", "e-2", "e-3"] {
            s.save_progress(id, 1000.0, 1000.0).unwrap();
        }
        assert_eq!(s.play_info("s-1").unwrap().unwrap().id, "e-1");
        let show = s.item("s-1").unwrap().unwrap();
        assert_eq!(show.watched, 3);
        assert_eq!(show.unwatched_date, None);
    }

    #[test]
    fn episodes_of_orders_by_kind() {
        let s = store();
        let show: Vec<_> = s
            .episodes_of("s-1")
            .unwrap()
            .into_iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(show, ["e-1", "e-2", "e-3"]);
        let chan: Vec<_> = s
            .episodes_of("c-1")
            .unwrap()
            .into_iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(chan, ["e-c2", "e-c1"]);
    }

    #[test]
    fn mark_played_on_a_game() {
        let mut s = Store::open_in_memory().unwrap();
        let mut g = entry("g-1", Kind::Game, "Sonic", Some("1991-06-23"));
        g.system = Some("genesis".into());
        s.apply_scan(&Scan {
            entries: vec![g],
            episodes: vec![],
            warnings: vec![],
        })
        .unwrap();
        s.mark_played("g-1").unwrap();
        let p = s.play_info("g-1").unwrap().unwrap();
        assert_eq!((p.kind, p.system.as_deref()), ("game", Some("genesis")));
        assert_eq!(s.item("g-1").unwrap().unwrap().label2, "Genesis");
    }

    #[test]
    fn persists_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        {
            let mut s = Store::open(&path).unwrap();
            s.apply_scan(&scan()).unwrap();
            s.save_progress("m-1", 200.0, 1000.0).unwrap();
        }
        let s = Store::open(&path).unwrap();
        assert_eq!(s.count().unwrap(), 3);
        assert_eq!(s.play_info("m-1").unwrap().unwrap().resume_position, 200.0);
    }
}
