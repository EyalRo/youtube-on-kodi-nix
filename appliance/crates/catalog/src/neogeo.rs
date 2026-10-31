//! Real names for Neo Geo ROMs. A Neo Geo set is named by its MAME short name (`mslug.zip`);
//! `neogeo_names.tsv` (generated from the FinalBurn Neo Neo Geo dat: short name, description,
//! year) maps those to the games' titles. The libretro thumbnail repository names its pictures by
//! the same descriptions, so the full description is also what finds the cover.

use std::collections::HashMap;
use std::sync::LazyLock;

struct Entry {
    full: &'static str,
    year: Option<i32>,
}

static NAMES: LazyLock<HashMap<&'static str, Entry>> = LazyLock::new(|| {
    include_str!("neogeo_names.tsv")
        .lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            let (short, full) = (f.next()?, f.next()?);
            Some((short, Entry { full, year: f.next().and_then(|y| y.parse().ok()) }))
        })
        .collect()
});

/// Titles as shown on a tile -> the full description, built once.
static BY_TITLE: LazyLock<HashMap<String, &'static str>> =
    LazyLock::new(|| NAMES.values().map(|e| (shown(e.full), e.full)).collect());

/// The description without its catalogue-number tags and without the Japanese title that
/// follows " / ": `Art of Fighting / Ryuuko no Ken (NGM-044 ~ NGH-044)` -> `Art of Fighting`.
fn shown(full: &str) -> String {
    let western = full.split(" / ").next().unwrap_or(full);
    let mut out = String::new();
    let mut depth = 0u32;
    for c in western.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

/// The title of the game whose ROM set is `short_name` (`mslug`), if it is a known Neo Geo set.
pub fn neogeo_title(short_name: &str) -> Option<String> {
    NAMES.get(short_name.to_ascii_lowercase().as_str()).map(|e| shown(e.full))
}

/// The year the game came out.
pub fn neogeo_year(short_name: &str) -> Option<i32> {
    NAMES.get(short_name.to_ascii_lowercase().as_str()).and_then(|e| e.year)
}

/// The full description (the thumbnail repository's name for the game) behind a title that
/// `neogeo_title` produced.
pub fn neogeo_full_name(title: &str) -> Option<&'static str> {
    BY_TITLE.get(title).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_names_become_titles() {
        assert_eq!(neogeo_title("mslug").as_deref(), Some("Metal Slug - Super Vehicle-001"));
        assert_eq!(neogeo_title("aof").as_deref(), Some("Art of Fighting"));
        assert_eq!(neogeo_title("KOF98").as_deref(), Some("The King of Fighters '98 - The Slugfest"));
        assert_eq!(neogeo_title("not-a-game"), None);
        assert_eq!(neogeo_year("mslug"), Some(1996));
    }

    #[test]
    fn a_title_leads_back_to_the_thumbnail_name() {
        let full = neogeo_full_name("Art of Fighting").unwrap();
        assert!(full.starts_with("Art of Fighting / "), "{full}");
        assert_eq!(neogeo_full_name("Nothing"), None);
    }
}
