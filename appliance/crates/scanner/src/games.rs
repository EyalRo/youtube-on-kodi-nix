use std::path::Path;

use catalog::{
    DateSource, Kind, MediaEntry, display_title, tosec, parse_release, system_for, system_for_folder,
};
use chrono::NaiveDate;
use walkdir::WalkDir;

use crate::release_db::ReleaseDb;
use crate::util::{ext_lower, file_name, file_stem, nfo_tag};
use crate::{Scan, ids};

/// Files next to ROMs that are not games.
const NOT_ROMS: &[&str] = &[
    "json", "nfo", "txt", "png", "jpg", "jpeg", "srm", "sav", "state", "cue", "m3u", "xml", "db",
];

/// Release date from a sidecar: `Game.json` {"released": "1991-06-23"} or `Game.nfo` <releasedate>.
fn sidecar_date(rom: &Path) -> Option<NaiveDate> {
    if let Ok(text) = std::fs::read_to_string(rom.with_extension("json"))
        && let Some(d) = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| {
                v.get("released")
                    .and_then(|r| r.as_str().map(str::to_string))
            })
            .and_then(|r| parse_release(&r))
    {
        return Some(d);
    }
    nfo_tag(&rom.with_extension("nfo"), &["releasedate"]).and_then(|v| parse_release(&v))
}

pub fn scan(media_root: &Path, db: &ReleaseDb) -> Scan {
    let mut scan = Scan::default();
    let roms = media_root.join("roms");
    let mut undated = 0usize;
    for entry in WalkDir::new(&roms)
        .min_depth(2)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| !file_name(e.path()).starts_with('.'))
        .filter_map(Result::ok)
    {
        let path = entry.path();
        let ext = ext_lower(path);
        if !entry.file_type().is_file() || NOT_ROMS.contains(&ext.as_str()) {
            continue;
        }
        // The first folder under roms/ names the system; deeper folders are just organisation.
        let Ok(rel) = path.strip_prefix(&roms) else {
            continue;
        };
        let folder = rel
            .components()
            .next()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .unwrap_or_default();
        let named = system_for_folder(&folder);
        let Some(system) = system_for(&folder, &ext) else {
            tracing::debug!("{}: unknown system, skipped", path.display());
            continue;
        };
        if !system.accepts(&ext, named.is_some_and(|n| n.id == system.id)) {
            tracing::debug!(
                "{}: .{ext} is not a {} ROM, skipped",
                path.display(),
                system.name
            );
            continue;
        }
        // The Neo Geo BIOS set lives beside the games but is not one.
        if system.id == "neogeo" && file_name(path).eq_ignore_ascii_case("neogeo.zip") {
            continue;
        }
        let stem = file_stem(path);
        let (date, source) = if let Some(d) = sidecar_date(path) {
            (Some(d), DateSource::Sidecar)
        } else if let Some(d) = db.lookup(&stem) {
            (Some(d), DateSource::Database)
        } else if let Some((_, Some(d))) = tosec(&file_name(path)) {
            (Some(d), DateSource::Filename)
        } else {
            undated += 1;
            (None, DateSource::None)
        };
        scan.entries.push(MediaEntry {
            id: ids::make("g", media_root, path),
            kind: Kind::Game,
            title: display_title(&file_name(path)),
            date,
            date_source: source,
            path: path.to_path_buf(),
            system: Some(system.id.to_string()),
            art: None,
        });
    }
    if undated > 0 {
        scan.warnings.push(format!(
            "{undated} game(s) without a release date (add Game.json or a release database)"
        ));
    }
    scan
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{create_dir_all, write};

    fn tree() -> tempfile::TempDir {
        let m = tempfile::tempdir().unwrap();
        let r = m.path().join("roms");
        create_dir_all(r.join("megadrive/A - F")).unwrap();
        create_dir_all(r.join("snes")).unwrap();
        create_dir_all(r.join("n64")).unwrap();
        write(r.join("megadrive/A - F/Aladdin (U) [!].bin"), b"r").unwrap();
        write(r.join("megadrive/A - F/Comix Zone (4) [!].smd"), b"r").unwrap();
        write(r.join("snes/Super Mario World.smc"), b"r").unwrap();
        write(
            r.join("snes/Super Mario World.json"),
            r#"{"released":"1990-11-21"}"#,
        )
        .unwrap();
        write(r.join("snes/notes.txt"), b"x").unwrap();
        write(r.join("n64/007 - GoldenEye.v64"), b"r").unwrap();
        write(r.join("n64/A Bug's Life (U) [!].z64"), b"r").unwrap();
        m
    }

    #[test]
    fn finds_roms_in_subfolders_by_system_including_bin_in_a_named_folder() {
        let m = tree();
        let s = scan(m.path(), &ReleaseDb::empty());
        let by_system = |sys: &str| {
            s.entries
                .iter()
                .filter(|e| e.system.as_deref() == Some(sys))
                .count()
        };
        assert_eq!(by_system("genesis"), 2, "the .bin and the .smd both count");
        assert_eq!(by_system("snes"), 1);
        assert_eq!(by_system("n64"), 2);
        assert!(s.entries.iter().all(|e| e.kind == Kind::Game));
        assert!(s.entries.iter().any(|e| e.title == "Aladdin"));
    }

    #[test]
    fn a_stray_bin_outside_a_named_folder_is_not_guessed() {
        let m = tempfile::tempdir().unwrap();
        create_dir_all(m.path().join("roms/misc")).unwrap();
        write(m.path().join("roms/misc/mystery.bin"), b"r").unwrap();
        assert!(scan(m.path(), &ReleaseDb::empty()).entries.is_empty());
    }

    #[test]
    fn dates_come_from_sidecar_then_database_else_none_with_one_summary_warning() {
        let m = tree();
        let mut db = ReleaseDb::empty();
        db.add_dat("game (\n\tname \"Aladdin (USA)\"\n\treleaseyear 1993\n\treleasemonth 11\n)\n");
        let s = scan(m.path(), &db);
        let by = |t: &str| s.entries.iter().find(|e| e.title == t).unwrap();
        assert_eq!(
            (
                by("Super Mario World").date,
                by("Super Mario World").date_source
            ),
            (NaiveDate::from_ymd_opt(1990, 11, 21), DateSource::Sidecar)
        );
        assert_eq!(
            (by("Aladdin").date, by("Aladdin").date_source),
            (NaiveDate::from_ymd_opt(1993, 11, 1), DateSource::Database)
        );
        assert_eq!(by("Comix Zone").date_source, DateSource::None);
        assert_eq!(s.warnings.len(), 1, "one summary, not one warning per game");
        assert!(s.warnings[0].starts_with("3 game(s)"));
    }
}
