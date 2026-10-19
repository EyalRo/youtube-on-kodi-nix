use std::sync::LazyLock;

use regex::Regex;

/// A game system: how to recognise its ROM folder and files.
pub struct System {
    pub id: &'static str,
    pub name: &'static str,
    /// File extensions (lower case, no dot) its cores load.
    pub exts: &'static [&'static str],
    /// Extra extensions accepted only when the *folder* names the system
    /// (`.bin` is ambiguous on its own, but fine inside `megadrive/`).
    pub folder_exts: &'static [&'static str],
    /// Folder names (letters and digits only, lower case) that mean this system.
    pub aliases: &'static [&'static str],
}

pub const SYSTEMS: &[System] = &[
    System {
        id: "nes",
        name: "NES",
        exts: &["nes", "unf", "unif"],
        folder_exts: &[],
        aliases: &["nes", "famicom", "nintendo", "nintendoentertainmentsystem"],
    },
    System {
        id: "snes",
        name: "SNES",
        exts: &["sfc", "smc"],
        folder_exts: &["swc", "fig"],
        aliases: &["snes", "supernintendo", "superfamicom", "supernes"],
    },
    System {
        id: "genesis",
        name: "Genesis",
        exts: &["md", "gen", "smd"],
        folder_exts: &["bin"],
        aliases: &["genesis", "megadrive", "segagenesis", "segamegadrive"],
    },
    System {
        id: "sms",
        name: "Master System",
        exts: &["sms"],
        folder_exts: &[],
        aliases: &["sms", "mastersystem", "segamastersystem"],
    },
    System {
        id: "gamegear",
        name: "Game Gear",
        exts: &["gg"],
        folder_exts: &[],
        aliases: &["gg", "gamegear", "segagamegear"],
    },
    System {
        id: "gb",
        name: "Game Boy",
        exts: &["gb"],
        folder_exts: &[],
        aliases: &["gb", "gameboy"],
    },
    System {
        id: "gbc",
        name: "Game Boy Color",
        exts: &["gbc"],
        folder_exts: &[],
        aliases: &["gbc", "gameboycolor"],
    },
    System {
        id: "gba",
        name: "Game Boy Advance",
        exts: &["gba"],
        folder_exts: &[],
        aliases: &["gba", "gameboyadvance"],
    },
    System {
        id: "n64",
        name: "Nintendo 64",
        exts: &["z64", "n64", "v64"],
        folder_exts: &[],
        aliases: &["n64", "nintendo64"],
    },
];

pub fn system_by_id(id: &str) -> Option<&'static System> {
    SYSTEMS.iter().find(|s| s.id == id)
}

fn squash(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The system a folder name stands for, if it is a known alias.
pub fn system_for_folder(folder: &str) -> Option<&'static System> {
    let key = squash(folder);
    SYSTEMS.iter().find(|s| s.aliases.contains(&key.as_str()))
}

impl System {
    /// Whether a file with this extension is a ROM of this system. `in_named_folder`
    /// is true when the folder names the system, which widens the accepted extensions.
    pub fn accepts(&self, ext: &str, in_named_folder: bool) -> bool {
        self.exts.contains(&ext) || (in_named_folder && self.folder_exts.contains(&ext))
    }
}

/// The system for a ROM in `folder` with extension `ext`: the folder name wins
/// when it is a known alias, otherwise the extension decides.
pub fn system_for(folder: &str, ext: &str) -> Option<&'static System> {
    system_for_folder(folder).or_else(|| SYSTEMS.iter().find(|s| s.exts.contains(&ext)))
}

/// Region/revision tags: `(USA, Europe)`, `[!]`.
static TAGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*[\(\[][^\)\]]*[\)\]]").unwrap());

/// File name without extension or tags: `Sonic The Hedgehog (USA, Europe) [!].md` -> `Sonic The Hedgehog`.
pub fn display_title(filename: &str) -> String {
    let stem = match filename.rfind('.') {
        Some(i) if i > 0 => &filename[..i],
        _ => filename,
    };
    let cleaned = TAGS.replace_all(stem, "");
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        filename.to_string()
    } else {
        cleaned.to_string()
    }
}

/// Lookup key for release databases: tags stripped, letters and digits only.
pub fn title_key(name: &str) -> String {
    squash(&TAGS.replace_all(name, ""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_alias_beats_extension() {
        // `.bin` is ambiguous; the folder decides.
        assert_eq!(system_for("megadrive", "bin").unwrap().id, "genesis");
        assert_eq!(system_for("Mega Drive", "bin").unwrap().id, "genesis");
        assert_eq!(system_for("SNES", "smc").unwrap().id, "snes");
    }

    #[test]
    fn bin_roms_are_accepted_only_inside_a_named_folder() {
        let genesis = system_by_id("genesis").unwrap();
        assert!(genesis.accepts("bin", true));
        assert!(!genesis.accepts("bin", false), "a loose .bin is ambiguous");
        assert!(genesis.accepts("smd", false));
        assert!(!genesis.accepts("nes", true));
    }

    #[test]
    fn extension_is_the_fallback() {
        assert_eq!(system_for("misc", "nes").unwrap().id, "nes");
        assert!(system_for("misc", "xyz").is_none());
    }

    #[test]
    fn n64_extensions_include_byte_swapped_dumps() {
        for ext in ["z64", "v64", "n64"] {
            assert_eq!(system_for("whatever", ext).unwrap().id, "n64");
        }
    }

    #[test]
    fn titles_lose_region_and_goodtools_tags() {
        assert_eq!(
            display_title("Sonic The Hedgehog (USA, Europe) [!].md"),
            "Sonic The Hedgehog"
        );
        assert_eq!(display_title("Aladdin (U) [!].bin"), "Aladdin");
        assert_eq!(display_title("Super Mario World.smc"), "Super Mario World");
        assert_eq!(display_title("(only tags).nes"), "(only tags).nes");
    }

    #[test]
    fn title_keys_match_across_naming_schemes() {
        assert_eq!(title_key("Aladdin (U) [!]"), title_key("Aladdin (USA)"));
        assert_eq!(title_key("Kirby's Dream Land 3"), "kirbysdreamland3");
    }
}
