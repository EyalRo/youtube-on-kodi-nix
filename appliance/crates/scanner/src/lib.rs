//! Scans the media library into catalog entries and episodes.
//!
//! Layout under the media root (all optional):
//!
//! ```text
//! movies/<Title (Year)>/<file>.mkv        (+ *.nfo, poster.jpg)
//! tv/<Show>/[Season N/]<... S01E02 ...>.mkv   (+ <file>.nfo)
//! youtube/<Channel>/<date - title [id]>.mp4   (+ <file>.info.json)
//! roms/<system>/[subfolders/]<game>.<ext>     (+ <game>.json or .nfo)
//! ```
//!
//! Every date records where it came from ([`catalog::DateSource`]).

mod games;
mod ids;
mod movies;
mod release_db;
mod tv;
mod util;
mod youtube;

use std::path::Path;

use catalog::{Episode, MediaEntry};

pub use release_db::ReleaseDb;

/// Everything one scan found.
#[derive(Debug, Default)]
pub struct Scan {
    pub entries: Vec<MediaEntry>,
    pub episodes: Vec<Episode>,
    /// Things skipped or guessed, for the log.
    pub warnings: Vec<String>,
}

impl Scan {
    fn extend(&mut self, other: Scan) {
        self.entries.extend(other.entries);
        self.episodes.extend(other.episodes);
        self.warnings.extend(other.warnings);
    }
}

pub fn scan_all(media_root: &Path, release_db: &ReleaseDb) -> Scan {
    let mut scan = Scan::default();
    scan.extend(movies::scan(media_root));
    scan.extend(tv::scan(media_root));
    scan.extend(youtube::scan(media_root));
    scan.extend(games::scan(media_root, release_db));
    for w in &scan.warnings {
        tracing::warn!("{w}");
    }
    tracing::info!(
        entries = scan.entries.len(),
        episodes = scan.episodes.len(),
        warnings = scan.warnings.len(),
        "scan finished"
    );
    scan
}

pub use games::scan as scan_games;
pub use movies::scan as scan_movies;
pub use tv::scan as scan_tv;
pub use youtube::scan as scan_youtube;
