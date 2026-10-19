use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use catalog::{parse_release, title_key};
use chrono::NaiveDate;
use regex::Regex;

static GAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?s)game\s*\(\s*name\s+"([^"]+)"(.*?)\n\)"#).unwrap());
static YEAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"releaseyear\s+(\d+)").unwrap());
static MONTH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"releasemonth\s+(\d+)").unwrap());

/// Release dates from libretro-database style `.dat` files, keyed by title with
/// region and revision tags removed. Month precision at best.
#[derive(Debug, Default)]
pub struct ReleaseDb {
    entries: HashMap<String, (i32, u32)>,
}

impl ReleaseDb {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Reads every `.dat` in `dir`; a missing directory gives an empty database.
    pub fn load_dir(dir: &Path) -> Self {
        let mut db = Self::default();
        let Ok(read) = fs::read_dir(dir) else {
            return db;
        };
        let mut files: Vec<_> = read.filter_map(Result::ok).map(|e| e.path()).collect();
        files.sort();
        for path in files
            .into_iter()
            .filter(|p| p.extension().is_some_and(|e| e == "dat"))
        {
            let Ok(bytes) = fs::read(&path) else { continue };
            db.add_dat(&String::from_utf8_lossy(&bytes));
        }
        db
    }

    pub fn add_dat(&mut self, text: &str) {
        for block in GAME.captures_iter(text) {
            let Some(year) = YEAR
                .captures(&block[2])
                .and_then(|c| c[1].parse::<i32>().ok())
            else {
                continue;
            };
            let month = MONTH
                .captures(&block[2])
                .and_then(|c| c[1].parse::<u32>().ok())
                .unwrap_or(1);
            self.entries.insert(title_key(&block[1]), (year, month));
        }
    }

    pub fn lookup(&self, file_stem: &str) -> Option<NaiveDate> {
        let (y, m) = self.entries.get(&title_key(file_stem))?;
        parse_release(&format!("{y}-{m}"))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAT: &str = "clrmamepro (\n\tname \"Sega - Mega Drive - Genesis\"\n)\n\ngame (\n\tname \"Sonic The Hedgehog (USA, Europe)\"\n\trom ( name \"x.md\" size 1 )\n\treleaseyear 1991\n\treleasemonth 6\n)\n\ngame (\n\tname \"Streets of Rage (USA)\"\n\treleaseyear 1991\n)\n";

    #[test]
    fn matches_across_naming_schemes_and_defaults_month() {
        let mut db = ReleaseDb::empty();
        db.add_dat(DAT);
        assert_eq!(db.len(), 2);
        assert_eq!(
            db.lookup("Sonic The Hedgehog (U) [!]"),
            NaiveDate::from_ymd_opt(1991, 6, 1)
        );
        assert_eq!(
            db.lookup("Streets of Rage"),
            NaiveDate::from_ymd_opt(1991, 1, 1)
        );
        assert_eq!(db.lookup("Unknown Game"), None);
    }

    #[test]
    fn a_missing_directory_is_just_empty() {
        assert!(ReleaseDb::load_dir(Path::new("/nonexistent/releases")).is_empty());
    }
}
