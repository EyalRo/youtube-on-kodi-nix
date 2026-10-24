use std::path::PathBuf;

use chrono::NaiveDate;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Movie,
    /// A TV show.
    Show,
    /// A YouTube channel: a show whose episodes are downloaded videos.
    Channel,
    Game,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Movie => "movie",
            Kind::Show => "show",
            Kind::Channel => "channel",
            Kind::Game => "game",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "movie" => Kind::Movie,
            "show" => Kind::Show,
            "channel" => Kind::Channel,
            "game" => Kind::Game,
            _ => return None,
        })
    }
}

/// Where a date came from, so a row's order can always be explained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DateSource {
    /// `<premiered>`, `<aired>` or `<releasedate>` in an NFO file.
    Nfo,
    /// `upload_date` in a yt-dlp `.info.json`.
    InfoJson,
    /// Only the year was known (folder name).
    Year,
    /// A game sidecar file (`Game.json`).
    Sidecar,
    /// A release database (libretro-database `.dat`).
    Database,
    /// File modification time: when it arrived, not when it aired.
    Mtime,
    None,
}

impl DateSource {
    pub fn as_str(self) -> &'static str {
        match self {
            DateSource::Nfo => "nfo",
            DateSource::InfoJson => "info_json",
            DateSource::Year => "year",
            DateSource::Sidecar => "sidecar",
            DateSource::Database => "database",
            DateSource::Mtime => "mtime",
            DateSource::None => "none",
        }
    }

    pub fn parse(s: &str) -> DateSource {
        match s {
            "nfo" => DateSource::Nfo,
            "info_json" => DateSource::InfoJson,
            "year" => DateSource::Year,
            "sidecar" => DateSource::Sidecar,
            "database" => DateSource::Database,
            "mtime" => DateSource::Mtime,
            _ => DateSource::None,
        }
    }
}

/// What a scan finds for one movie, show, channel or game.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaEntry {
    pub id: String,
    pub kind: Kind,
    pub title: String,
    pub date: Option<NaiveDate>,
    pub date_source: DateSource,
    /// The file to play (movie, game) or the folder (show, channel).
    pub path: PathBuf,
    pub system: Option<String>,
    pub art: Option<PathBuf>,
}

/// One episode of a show or channel.
#[derive(Debug, Clone, PartialEq)]
pub struct Episode {
    pub id: String,
    pub item_id: String,
    pub season: i32,
    pub episode: i32,
    pub title: String,
    pub aired: Option<NaiveDate>,
    pub date_source: DateSource,
    pub path: PathBuf,
    /// A thumbnail next to the video (or fetched into the art cache).
    pub art: Option<PathBuf>,
}

/// An entry as the row engine sees it, with watch state folded in.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Item {
    pub id: String,
    pub kind: Kind,
    pub title: String,
    /// The one sort date: release date, or newest aired episode.
    pub date: Option<NaiveDate>,
    pub date_source: DateSource,
    /// Display line under the title: "Movie", "TV show", "YouTube" or the game system.
    pub label2: String,
    pub system: Option<String>,
    pub art: Option<String>,
    /// Episodes watched (shows and channels).
    pub watched: u32,
    /// Newest aired episode that is still unwatched.
    pub unwatched_date: Option<NaiveDate>,
}
