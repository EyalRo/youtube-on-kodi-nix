//! Curated game lists: "NES canon" (a ranked set of titles) or "Castlevania series"
//! (every game whose title contains a key). Pure data; mediad loads the optional
//! user file and hands the result to the row engine.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;

use crate::item::Item;
use crate::systems::{strip_tags, title_key};

const DEFAULT_JSON: &str = include_str!("game_lists.json");

/// One title in a list: a name, or a name with alternates (regional titles,
/// renamed releases). Any of them matches.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum Entry {
    One(String),
    Any(Vec<String>),
}

#[derive(Debug, Clone, Deserialize)]
struct Raw {
    id: String,
    label: String,
    /// Restrict to one system id (`nes`, `snes`, ...); omit for every system.
    #[serde(default)]
    system: Option<String>,
    /// Show in list order (rank 1 first) instead of release order.
    #[serde(default)]
    ranked: bool,
    /// Exact titles (region/revision tags and articles ignored).
    #[serde(default)]
    titles: Vec<Entry>,
    /// Series rule: matches a game when its normalised title contains any of these
    /// (lower case, letters and digits only).
    #[serde(default)]
    contains: Vec<String>,
}

/// A game list ready to match items against.
#[derive(Debug, Clone)]
pub struct GameList {
    pub id: String,
    pub label: String,
    pub ranked: bool,
    system: Option<String>,
    /// Normalised title -> rank (position in `titles`).
    ranks: HashMap<String, usize>,
    contains: Vec<String>,
}

impl GameList {
    fn from_raw(raw: Raw) -> Self {
        let mut ranks = HashMap::new();
        for (rank, entry) in raw.titles.iter().enumerate() {
            let names: &[String] = match entry {
                Entry::One(n) => std::slice::from_ref(n),
                Entry::Any(v) => v,
            };
            for n in names {
                ranks.entry(match_key(n)).or_insert(rank);
            }
        }
        Self {
            id: raw.id,
            label: raw.label,
            ranked: raw.ranked,
            system: raw.system,
            ranks,
            contains: raw.contains.iter().map(|c| match_key(c)).collect(),
        }
    }

    /// The item's position in the list if it belongs (0 for series rules).
    pub fn rank_of(&self, item: &Item) -> Option<usize> {
        if let Some(sys) = &self.system
            && item.system.as_deref() != Some(sys.as_str())
        {
            return None;
        }
        let key = match_key(&item.title);
        if let Some(r) = self.ranks.get(&key) {
            return Some(*r);
        }
        self.contains
            .iter()
            .any(|c| !c.is_empty() && key.contains(c.as_str()))
            .then_some(0)
    }
}

static ARTICLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.+?), (The|A|An)( - |: |$)").unwrap());

/// Like [`title_key`], also treating "Legend of Zelda, The - X" as "The Legend of Zelda: X"
/// and ignoring a leading "the".
fn match_key(name: &str) -> String {
    let name = strip_tags(name);
    let name = ARTICLE.replace(&name, "$2 $1$3");
    let key = title_key(&name);
    key.strip_prefix("the").map_or(key.clone(), str::to_string)
}

/// The lists in display order.
#[derive(Debug, Clone, Default)]
pub struct GameLists {
    lists: Vec<GameList>,
}

impl GameLists {
    pub fn empty() -> Self {
        Self::default()
    }

    /// The lists shipped with the appliance.
    pub fn builtin() -> Self {
        Self::from_json(DEFAULT_JSON).expect("built-in game lists are valid")
    }

    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        let raws: Vec<Raw> = serde_json::from_str(text)?;
        Ok(Self {
            lists: raws.into_iter().map(GameList::from_raw).collect(),
        })
    }

    /// `other`'s lists replace same-id lists in place; new ones are appended.
    /// A list with no `titles` and no `contains` removes the built-in of that id.
    pub fn merged_with(mut self, other: Self) -> Self {
        for l in other.lists {
            let empty = l.ranks.is_empty() && l.contains.is_empty();
            match self.lists.iter().position(|x| x.id == l.id) {
                Some(i) if empty => {
                    self.lists.remove(i);
                }
                Some(i) => self.lists[i] = l,
                None if empty => {}
                None => self.lists.push(l),
            }
        }
        self
    }

    pub fn iter(&self) -> impl Iterator<Item = &GameList> {
        self.lists.iter()
    }

    pub fn get(&self, id: &str) -> Option<&GameList> {
        self.lists.iter().find(|l| l.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::{DateSource, Kind};

    fn game(title: &str, system: &str) -> Item {
        Item {
            id: title.to_string(),
            kind: Kind::Game,
            title: title.to_string(),
            date: None,
            date_source: DateSource::None,
            label2: String::new(),
            system: Some(system.to_string()),
            art: None,
            watched: 0,
            viewed: false,
            unwatched_date: None,
        }
    }

    #[test]
    fn builtin_lists_parse() {
        let l = GameLists::builtin();
        assert!(l.get("canon-nes").is_some() && l.get("castlevania").is_some());
        assert_eq!(l.iter().filter(|x| x.id.starts_with("canon-")).count(), 5);
    }

    #[test]
    fn canon_matches_rom_names_with_articles_tags_and_alternates() {
        let l = GameLists::builtin();
        let nes = l.get("canon-nes").unwrap();
        assert_eq!(nes.rank_of(&game("Legend of Zelda, The", "nes")), Some(1));
        assert_eq!(nes.rank_of(&game("Super Mario Bros. 3", "nes")), Some(2));
        assert_eq!(nes.rank_of(&game("Super Mario Bros.", "nes")), Some(0));
        assert_eq!(nes.rank_of(&game("DuckTales", "nes")), Some(13));
        assert_eq!(nes.rank_of(&game("Super Mario Bros. 2", "nes")), None);
        let snes = l.get("canon-snes").unwrap();
        assert_eq!(
            snes.rank_of(&game("Legend of Zelda, The - A Link to the Past", "snes")),
            Some(1)
        );
        assert_eq!(snes.rank_of(&game("Final Fantasy VI", "snes")), Some(4));
    }

    #[test]
    fn same_title_on_another_system_is_not_in_the_list() {
        let l = GameLists::builtin();
        let sms = l.get("canon-sms").unwrap();
        assert_eq!(sms.rank_of(&game("Sonic the Hedgehog", "sms")), Some(4));
        assert_eq!(sms.rank_of(&game("Sonic the Hedgehog", "genesis")), None);
        let gen_ = l.get("canon-genesis").unwrap();
        assert_eq!(
            gen_.rank_of(&game("Sonic the Hedgehog", "genesis")),
            Some(4)
        );
    }

    #[test]
    fn series_rule_matches_by_substring_across_systems() {
        let l = GameLists::builtin();
        let cv = l.get("castlevania").unwrap();
        assert!(
            cv.rank_of(&game("Castlevania II: Simon's Quest", "nes"))
                .is_some()
        );
        assert!(
            cv.rank_of(&game("Castlevania: Bloodlines", "genesis"))
                .is_some()
        );
        assert!(cv.rank_of(&game("Contra", "nes")).is_none());
    }

    #[test]
    fn user_lists_override_add_and_remove() {
        let user = GameLists::from_json(
            r#"[
              {"id":"zelda","label":"My Zelda","contains":["zelda"]},
              {"id":"mine","label":"Mine","titles":["Contra"]},
              {"id":"sonic","label":"gone"}
            ]"#,
        )
        .unwrap();
        let merged = GameLists::builtin().merged_with(user);
        assert_eq!(merged.get("zelda").unwrap().label, "My Zelda");
        assert!(merged.get("mine").is_some());
        assert!(merged.get("sonic").is_none());
    }
}
