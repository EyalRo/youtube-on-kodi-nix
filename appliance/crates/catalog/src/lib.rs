//! Domain model and row engine for the media appliance. No I/O.
//!
//! Every item has one sort date, newest first, undated last:
//! a movie's or game's release date, a show's or channel's newest aired episode.

mod date;
mod item;
mod rows;
mod systems;

pub use date::parse_release;
pub use item::{DateSource, Episode, Item, Kind, MediaEntry};
pub use rows::{FIXED_ROW_IDS, RowInfo, build_row, list_rows, sort_newest};
pub use systems::{
    SYSTEMS, System, display_title, system_by_id, system_for, system_for_folder, title_key,
};
