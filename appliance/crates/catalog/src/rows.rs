use std::cmp::Reverse;

use chrono::NaiveDate;
use serde::Serialize;

use crate::item::{Item, Kind};
use crate::systems::{SYSTEMS, system_by_id};

/// The rows that always exist, in display order.
pub const FIXED_ROW_IDS: &[&str] = &["everything", "movies", "tvshows", "channels", "unwatched", "games"];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RowInfo {
    pub id: String,
    pub label: String,
    pub count: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum SortBy {
    Date,
    UnwatchedDate,
}

struct RowDef {
    id: String,
    label: String,
    sort: SortBy,
}

fn matches(row_id: &str, item: &Item) -> bool {
    match row_id {
        "everything" => true,
        "movies" => item.kind == Kind::Movie,
        "tvshows" => item.kind == Kind::Show,
        "channels" => item.kind == Kind::Channel,
        "unwatched" => {
            matches!(item.kind, Kind::Show | Kind::Channel) && item.watched > 0 && item.unwatched_date.is_some()
        }
        "games" => item.kind == Kind::Game,
        other => match other.strip_prefix("games:") {
            Some(system) => item.kind == Kind::Game && item.system.as_deref() == Some(system),
            None => false,
        },
    }
}

fn label_of(id: &str) -> &'static str {
    match id {
        "everything" => "Everything",
        "movies" => "Movies",
        "tvshows" => "TV shows",
        "channels" => "YouTube channels",
        "unwatched" => "Shows with unwatched episodes",
        "games" => "Games",
        _ => "",
    }
}

/// Fixed rows plus one games row per system that has games (by display name).
fn definitions(items: &[Item]) -> Vec<RowDef> {
    let mut rows: Vec<RowDef> = FIXED_ROW_IDS
        .iter()
        .map(|id| RowDef {
            id: (*id).to_string(),
            label: label_of(id).to_string(),
            sort: if *id == "unwatched" { SortBy::UnwatchedDate } else { SortBy::Date },
        })
        .collect();
    let mut systems: Vec<&str> = items
        .iter()
        .filter(|i| i.kind == Kind::Game)
        .filter_map(|i| i.system.as_deref())
        .collect();
    systems.sort_by_key(|s| system_by_id(s).map_or(*s, |sys| sys.name));
    systems.dedup();
    for sid in systems {
        let label = system_by_id(sid).map_or(sid, |s| s.name);
        rows.push(RowDef { id: format!("games:{sid}"), label: label.to_string(), sort: SortBy::Date });
    }
    rows
}

/// Newest first by `date` (or the newest unwatched date); undated last; ties by title.
pub fn sort_newest(items: &mut [Item], by_unwatched: bool) {
    let key = |i: &Item| -> Option<NaiveDate> { if by_unwatched { i.unwatched_date } else { i.date } };
    items.sort_by_cached_key(|i| (key(i).is_none(), Reverse(key(i)), i.title.to_lowercase()));
}

/// Rows with at least one item (Everything always), in display order.
pub fn list_rows(items: &[Item]) -> Vec<RowInfo> {
    definitions(items)
        .into_iter()
        .filter_map(|def| {
            let count = items.iter().filter(|i| matches(&def.id, i)).count();
            (count > 0 || def.id == "everything").then_some(RowInfo { id: def.id, label: def.label, count })
        })
        .collect()
}

/// The items of one row in display order. Unknown rows are `None`; a supported
/// game system without games is an empty row, not an error.
pub fn build_row(row_id: &str, items: &[Item]) -> Option<Vec<Item>> {
    for def in definitions(items) {
        if def.id == row_id {
            let mut out: Vec<Item> = items.iter().filter(|i| matches(row_id, i)).cloned().collect();
            sort_newest(&mut out, def.sort == SortBy::UnwatchedDate);
            return Some(out);
        }
    }
    match row_id.strip_prefix("games:") {
        Some(sid) if SYSTEMS.iter().any(|s| s.id == sid) => Some(Vec::new()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::DateSource;

    fn d(y: i32, m: u32, day: u32) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(y, m, day)
    }

    fn item(kind: Kind, title: &str, date: Option<NaiveDate>) -> Item {
        Item {
            id: title.to_string(),
            kind,
            title: title.to_string(),
            date,
            date_source: DateSource::None,
            label2: String::new(),
            system: None,
            art: None,
            watched: 0,
            unwatched_date: None,
        }
    }

    fn game(title: &str, system: &str, date: Option<NaiveDate>) -> Item {
        Item { system: Some(system.to_string()), ..item(Kind::Game, title, date) }
    }

    /// The same catalog the Python row tests use.
    fn catalog() -> Vec<Item> {
        vec![
            item(Kind::Movie, "Alien", d(1979, 5, 25)),
            item(Kind::Movie, "Heat", d(1995, 1, 1)),
            item(Kind::Movie, "Undated Movie", None),
            Item { watched: 1, unwatched_date: d(2013, 9, 29), ..item(Kind::Show, "Breaking Bad", d(2013, 9, 29)) },
            item(Kind::Channel, "Some Channel", d(2024, 6, 2)),
            Item { watched: 2, ..item(Kind::Show, "Finished Show", d(1999, 1, 1)) },
            item(Kind::Show, "Empty Show", None),
            game("Zelda", "nes", d(1987, 1, 1)),
            game("Sonic", "genesis", d(1991, 6, 1)),
            game("No Date", "nes", None),
        ]
    }

    fn titles(row: &str, items: &[Item]) -> Vec<String> {
        build_row(row, items).unwrap().into_iter().map(|i| i.title).collect()
    }

    #[test]
    fn everything_is_newest_first_across_kinds_undated_last() {
        let order = titles("everything", &catalog());
        assert_eq!(
            order,
            ["Some Channel", "Breaking Bad", "Finished Show", "Heat", "Sonic", "Zelda", "Alien", "Empty Show", "No Date", "Undated Movie"]
        );
    }

    #[test]
    fn type_rows_filter() {
        let c = catalog();
        assert_eq!(titles("movies", &c), ["Heat", "Alien", "Undated Movie"]);
        assert_eq!(titles("tvshows", &c), ["Breaking Bad", "Finished Show", "Empty Show"]);
        assert_eq!(titles("channels", &c), ["Some Channel"]);
        assert_eq!(titles("games", &c), ["Sonic", "Zelda", "No Date"]);
        assert_eq!(titles("games:nes", &c), ["Zelda", "No Date"]);
    }

    #[test]
    fn unwatched_needs_progress_and_an_unwatched_dated_episode() {
        // Breaking Bad: 1 watched and a dated unwatched episode. The channel has
        // no progress; Finished Show has nothing unwatched.
        assert_eq!(titles("unwatched", &catalog()), ["Breaking Bad"]);
    }

    #[test]
    fn unwatched_row_sorts_by_newest_unwatched_episode() {
        let mut c = catalog();
        c[5].unwatched_date = d(2020, 1, 1); // Finished Show gets a newer unwatched episode
        assert_eq!(titles("unwatched", &c), ["Finished Show", "Breaking Bad"]);
    }

    #[test]
    fn ties_break_by_title_case_insensitively() {
        let c = vec![
            item(Kind::Movie, "beta", d(2000, 1, 1)),
            item(Kind::Movie, "Alpha", d(2000, 1, 1)),
            item(Kind::Movie, "gamma", None),
            item(Kind::Movie, "Delta", None),
        ];
        assert_eq!(titles("movies", &c), ["Alpha", "beta", "Delta", "gamma"]);
    }

    #[test]
    fn one_games_row_per_system_present_ordered_by_name() {
        let ids: Vec<String> = list_rows(&catalog()).into_iter().map(|r| r.id).collect();
        assert_eq!(&ids[ids.len() - 3..], ["games", "games:genesis", "games:nes"]);
    }

    #[test]
    fn empty_rows_are_hidden_but_everything_stays() {
        let only_movies = vec![item(Kind::Movie, "Heat", d(1995, 1, 1))];
        let ids: Vec<String> = list_rows(&only_movies).into_iter().map(|r| r.id).collect();
        assert_eq!(ids, ["everything", "movies"]);
        let none: Vec<Item> = vec![];
        let ids: Vec<String> = list_rows(&none).into_iter().map(|r| r.id).collect();
        assert_eq!(ids, ["everything"]);
    }

    #[test]
    fn unknown_row_is_none_but_a_supported_empty_system_is_empty() {
        let c = catalog();
        assert!(build_row("nope", &c).is_none());
        assert!(build_row("games:atari", &c).is_none());
        assert_eq!(build_row("games:snes", &c), Some(vec![]));
    }

    #[test]
    fn counts_match_rows() {
        let rows = list_rows(&catalog());
        let count = |id: &str| rows.iter().find(|r| r.id == id).unwrap().count;
        assert_eq!(count("everything"), 10);
        assert_eq!(count("movies"), 3);
        assert_eq!(count("games"), 3);
        assert_eq!(count("unwatched"), 1);
    }
}
