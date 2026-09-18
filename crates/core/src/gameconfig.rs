//! Telling a reimplementation where the original game's data is.
//!
//! These are the games' own configuration files, not Turnstile's. Somebody
//! has their key bindings, their volume and their language in there, so
//! setting one value means editing one line and leaving the rest of the file
//! exactly as it was, not writing a file out from a template.
//!
//! Neither game needs a complete file to start from. Given one key, each
//! fills in every default it has and rewrites the file itself on first run,
//! which was checked by giving OpenLoco a one line `openloco.yml` and
//! watching it grow to a hundred and seventy.

use std::path::{Path, PathBuf};

use crate::dirs::Dirs;
use crate::error::CoreError;
use crate::gamedata::OriginalGame;

/// Where the setting lives and what it is called there.
struct Setting {
    file: &'static str,
    format: Format,
    key: &'static str,
}

enum Format {
    /// OpenRCT2's `config.ini`, whose values are quoted.
    Ini { section: &'static str },
    /// OpenLoco's `openloco.yml`.
    Yaml,
}

impl OriginalGame {
    /// The game whose configuration names this data, which is not the same
    /// question as which game the data belongs to: RollerCoaster Tycoon 1's
    /// data is read by OpenRCT2.
    #[must_use]
    pub const fn configured_in(self) -> crate::game::GameId {
        match self {
            Self::RollerCoasterTycoon1 | Self::RollerCoasterTycoon2 => {
                crate::game::GameId::OpenRCT2
            }
            Self::Locomotion => crate::game::GameId::OpenLoco,
        }
    }

    const fn setting(self) -> Setting {
        match self {
            Self::RollerCoasterTycoon1 => Setting {
                file: "config.ini",
                format: Format::Ini { section: "general" },
                key: "rct1_path",
            },
            Self::RollerCoasterTycoon2 => Setting {
                file: "config.ini",
                format: Format::Ini { section: "general" },
                key: "game_path",
            },
            Self::Locomotion => Setting {
                file: "openloco.yml",
                format: Format::Yaml,
                key: "loco_install_path",
            },
        }
    }
}

/// The configuration file that names this game's data.
#[must_use]
pub fn config_path(game: OriginalGame, dirs: &Dirs) -> PathBuf {
    game.configured_in()
        .game_path(dirs)
        .join(game.setting().file)
}

/// Points the reimplementation at the data installed in `data`.
///
/// # Errors
///
/// Returns [`CoreError::Io`] if the file cannot be read or replaced.
pub fn point_at(game: OriginalGame, dirs: &Dirs, data: &Path) -> Result<(), CoreError> {
    let setting = game.setting();
    let path = config_path(game, dirs);
    let value = data.to_string_lossy();

    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(io(error)),
    };

    let updated = match setting.format {
        Format::Ini { section } => set_ini_key(&existing, section, setting.key, &value),
        Format::Yaml => set_yaml_key(&existing, setting.key, &value),
    };

    if updated == existing {
        return Ok(());
    }

    write_replacing(&path, &updated)
}

/// Reads back what the configuration says this game's data path is.
///
/// # Errors
///
/// Returns [`CoreError::Io`] if the file exists and cannot be read.
pub fn configured_path(game: OriginalGame, dirs: &Dirs) -> Result<Option<PathBuf>, CoreError> {
    let setting = game.setting();
    let path = config_path(game, dirs);

    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io(error)),
    };

    let value = match setting.format {
        Format::Ini { section } => read_ini_key(&text, section, setting.key),
        Format::Yaml => read_yaml_key(&text, setting.key),
    };

    Ok(value.map(PathBuf::from))
}

/// Replaces a file without ever leaving it half written.
///
/// This is somebody's settings. A crash between truncating and writing would
/// lose the lot, so the new text goes to a neighbouring file and is renamed
/// over the old one, which is atomic.
fn write_replacing(path: &Path, text: &str) -> Result<(), CoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }

    let temporary = path.with_extension(format!(
        "{}.turnstile-new",
        path.extension().unwrap_or_default().to_string_lossy()
    ));

    std::fs::write(&temporary, text).map_err(io)?;

    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(io(error));
    }

    Ok(())
}

/// The line ending the file already uses, so an edit does not leave a file
/// with two kinds.
fn line_ending(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

/// Sets `key` in `section`, leaving every other line untouched.
fn set_ini_key(text: &str, section: &str, key: &str, value: &str) -> String {
    let ending = line_ending(text);
    let line = format!("{key} = \"{}\"", escape_ini(value));

    let mut out: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    let mut done = false;
    let mut end_of_section: Option<usize> = None;

    for raw in text.lines() {
        let trimmed = raw.trim();

        if let Some(name) = trimmed
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            // Leaving the section we wanted without having found the key:
            // remember where it ended so the key can go in at the bottom of
            // it rather than at the bottom of the file, under a heading that
            // would put it in somebody else's section.
            if !done && current.as_deref() == Some(section) && end_of_section.is_none() {
                end_of_section = Some(out.len());
            }
            current = Some(name.trim().to_string());
            out.push(raw.to_string());
            continue;
        }

        if !done && current.as_deref() == Some(section) && is_key_line(trimmed, key) {
            out.push(line.clone());
            done = true;
            continue;
        }

        out.push(raw.to_string());
    }

    if !done {
        match end_of_section {
            Some(at) => out.insert(at, line),
            None if current.as_deref() == Some(section) => out.push(line),
            None => {
                if !out.is_empty() {
                    out.push(String::new());
                }
                out.push(format!("[{section}]"));
                out.push(line);
            }
        }
    }

    let mut result = out.join(ending);
    result.push_str(ending);
    result
}

fn read_ini_key(text: &str, section: &str, key: &str) -> Option<String> {
    let mut current: Option<&str> = None;

    for raw in text.lines() {
        let trimmed = raw.trim();

        if let Some(name) = trimmed
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            current = Some(name.trim());
            continue;
        }

        if current == Some(section) && is_key_line(trimmed, key) {
            let value = trimmed.split_once('=')?.1.trim();
            return Some(unescape_ini(value));
        }
    }

    None
}

/// Whether a line assigns `key`, allowing for the spacing people and
/// programs put around the equals sign.
fn is_key_line(trimmed: &str, key: &str) -> bool {
    trimmed
        .split_once('=')
        .is_some_and(|(name, _)| name.trim() == key)
}

fn escape_ini(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn unescape_ini(value: &str) -> String {
    let value = value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(value);

    let mut out = String::with_capacity(value.len());
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character == '\\' {
            match characters.next() {
                Some(escaped) => out.push(escaped),
                None => out.push('\\'),
            }
        } else {
            out.push(character);
        }
    }
    out
}

/// Sets a top level `key`, leaving every other line untouched.
///
/// Only a key at the start of a line is replaced. The same name indented
/// under something else is a different setting, and rewriting it would move
/// a value from one place in the document to another.
fn set_yaml_key(text: &str, key: &str, value: &str) -> String {
    let ending = line_ending(text);
    let line = format!("{key}: \"{}\"", escape_yaml(value));

    let mut out: Vec<String> = Vec::new();
    let mut done = false;

    for raw in text.lines() {
        if !done && is_top_level_key(raw, key) {
            out.push(line.clone());
            done = true;
            continue;
        }
        out.push(raw.to_string());
    }

    if !done {
        out.insert(0, line);
    }

    let mut result = out.join(ending);
    result.push_str(ending);
    result
}

fn read_yaml_key(text: &str, key: &str) -> Option<String> {
    text.lines()
        .find(|raw| is_top_level_key(raw, key))
        .map(|raw| {
            let value = raw.split_once(':').map_or("", |(_, rest)| rest).trim();
            unescape_yaml(value)
        })
}

fn is_top_level_key(raw: &str, key: &str) -> bool {
    raw.strip_prefix(key)
        .is_some_and(|rest| rest.trim_start().starts_with(':'))
}

fn escape_yaml(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn unescape_yaml(value: &str) -> String {
    if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
        return unescape_ini(value);
    }
    if value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2 {
        return value[1..value.len() - 1].replace("''", "'");
    }
    value.to_string()
}

fn io(error: std::io::Error) -> CoreError {
    CoreError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{escape_ini, read_ini_key, read_yaml_key, set_ini_key, set_yaml_key, unescape_ini};

    const OPENRCT2: &str = "\
[general]
always_show_gridlines = false
rct1_path = \"/old/rct1\"
game_path = \"/old/rct2\"
language = en-US

[interface]
game_path = \"not this one\"
";

    #[test]
    fn a_key_is_replaced_where_it_stands() {
        let out = set_ini_key(OPENRCT2, "general", "game_path", "/new/rct2");
        assert!(out.contains("game_path = \"/new/rct2\""));
        assert_eq!(out.lines().count(), OPENRCT2.lines().count());
        assert_eq!(
            out.lines().position(|l| l.starts_with("game_path")),
            OPENRCT2.lines().position(|l| l.starts_with("game_path")),
            "the line must not move"
        );
    }

    #[test]
    fn every_other_line_survives_untouched() {
        // This file is somebody's key bindings and language. Rewriting it
        // from what we understand of it would drop everything we do not.
        let out = set_ini_key(OPENRCT2, "general", "game_path", "/new/rct2");
        for line in OPENRCT2.lines().filter(|l| !l.starts_with("game_path")) {
            assert!(out.contains(line), "lost: {line}");
        }
    }

    #[test]
    fn the_same_key_in_another_section_is_left_alone() {
        let out = set_ini_key(OPENRCT2, "general", "game_path", "/new/rct2");
        assert!(
            out.contains("game_path = \"not this one\""),
            "[interface] was edited"
        );
    }

    #[test]
    fn a_missing_key_joins_its_section_rather_than_the_end_of_the_file() {
        // Appended after [interface] it would be an interface setting, and
        // OpenRCT2 would never read it.
        let text = "[general]\nlanguage = en-US\n\n[interface]\ntoolbar = true\n";
        let out = set_ini_key(text, "general", "game_path", "/rct2");

        let game_path = out
            .lines()
            .position(|l| l.starts_with("game_path"))
            .unwrap();
        let interface = out.lines().position(|l| l == "[interface]").unwrap();
        assert!(game_path < interface, "landed in the wrong section:\n{out}");
    }

    #[test]
    fn a_missing_section_is_added() {
        let out = set_ini_key(
            "[interface]\ntoolbar = true\n",
            "general",
            "game_path",
            "/rct2",
        );
        assert!(out.contains("[general]"));
        assert!(out.contains("game_path = \"/rct2\""));
        assert!(out.contains("toolbar = true"));
    }

    #[test]
    fn an_empty_file_becomes_a_minimal_one() {
        // Both games fill in every other default themselves on first run.
        let out = set_ini_key("", "general", "game_path", "/rct2");
        assert_eq!(out, "[general]\ngame_path = \"/rct2\"\n");
    }

    #[test]
    fn spacing_around_the_equals_sign_does_not_hide_the_key() {
        let out = set_ini_key(
            "[general]\n  game_path   =   \"/old\"\n",
            "general",
            "game_path",
            "/new",
        );
        assert!(out.contains("game_path = \"/new\""));
        assert!(!out.contains("/old"), "{out}");
    }

    #[test]
    fn a_quote_or_a_backslash_in_the_path_is_escaped_the_way_openrct2_writes_it() {
        // Checked against OpenRCT2 itself, by giving its own `set-rct2` a
        // directory with each character in the name and reading back what it
        // wrote.
        assert_eq!(escape_ini(r#"/a "b" c"#), r#"/a \"b\" c"#);
        assert_eq!(escape_ini(r"/a\b"), r"/a\\b");
    }

    #[test]
    fn a_path_survives_being_written_and_read_back() {
        for path in [
            "/plain",
            r#"/with "quotes""#,
            r"/with\backslash",
            "/with spaces",
        ] {
            let out = set_ini_key("", "general", "game_path", path);
            assert_eq!(
                read_ini_key(&out, "general", "game_path").as_deref(),
                Some(path),
                "round trip failed for {path}"
            );
        }
    }

    #[test]
    fn unescaping_a_value_that_was_never_escaped_leaves_it_alone() {
        assert_eq!(unescape_ini("/plain/path"), "/plain/path");
    }

    #[test]
    fn a_file_with_windows_line_endings_keeps_them() {
        // Mixing the two would show up as stray characters in an editor and
        // as a needlessly enormous diff in anybody's backups.
        let text = "[general]\r\ngame_path = \"/old\"\r\n";
        let out = set_ini_key(text, "general", "game_path", "/new");
        assert!(!out.contains("\n\n"), "a bare newline crept in: {out:?}");
        assert!(out.contains("game_path = \"/new\"\r\n"));
    }

    const OPENLOCO: &str = "\
loco_install_path: \"/old/loco\"
display:
  mode: window
language: en-GB
";

    #[test]
    fn a_top_level_yaml_key_is_replaced_in_place() {
        let out = set_yaml_key(OPENLOCO, "loco_install_path", "/new/loco");
        assert!(out.contains("loco_install_path: \"/new/loco\""));
        assert!(out.contains("  mode: window"));
        assert!(out.contains("language: en-GB"));
        assert_eq!(out.lines().count(), OPENLOCO.lines().count());
    }

    #[test]
    fn an_indented_key_of_the_same_name_is_not_the_one_we_mean() {
        // Nested under something else it is a different setting, and moving
        // a value there would change the meaning of a document we do not
        // otherwise understand.
        let text = "display:\n  loco_install_path: nested\nlanguage: en-GB\n";
        let out = set_yaml_key(text, "loco_install_path", "/loco");

        assert!(out.contains("  loco_install_path: nested"), "{out}");
        assert!(out.starts_with("loco_install_path: \"/loco\""), "{out}");
    }

    #[test]
    fn a_missing_yaml_key_is_added_at_the_top() {
        let out = set_yaml_key("language: en-GB\n", "loco_install_path", "/loco");
        assert_eq!(out, "loco_install_path: \"/loco\"\nlanguage: en-GB\n");
    }

    #[test]
    fn an_empty_yaml_file_becomes_a_single_line() {
        // What OpenLoco was actually given in testing, and it filled in the
        // other hundred and seventy lines itself.
        let out = set_yaml_key("", "loco_install_path", "/loco");
        assert_eq!(out, "loco_install_path: \"/loco\"\n");
    }

    #[test]
    fn a_yaml_path_survives_being_written_and_read_back() {
        for path in ["/plain", r#"/with "quotes""#, "/with: colon", "/with #hash"] {
            let out = set_yaml_key("", "loco_install_path", path);
            assert_eq!(
                read_yaml_key(&out, "loco_install_path").as_deref(),
                Some(path),
                "round trip failed for {path}"
            );
        }
    }

    #[test]
    fn a_value_openloco_wrote_unquoted_is_read_back_whole() {
        // It writes the path bare when nothing in it needs quoting, so
        // reading has to cope with both.
        let text = "loco_install_path: /Users/someone/Game Data/locomotion\n";
        assert_eq!(
            read_yaml_key(text, "loco_install_path").as_deref(),
            Some("/Users/someone/Game Data/locomotion")
        );
    }
}
