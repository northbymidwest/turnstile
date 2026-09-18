//! Telling a reimplementation where the original game's data is.
//!
//! These are the games' own configuration files, not Turnstile's. Somebody has
//! their key bindings, their volume and their language in there, so setting one
//! value means editing one line and leaving the rest exactly as it was.
//!
//! Neither game needs a complete file to start from: given one key, each fills
//! in every default it has and rewrites the file on first run.
//!
//! None of this decodes the file. OpenLoco writes eight raw `0xff` bytes as
//! the name of the owner face nobody has chosen, so its configuration stops
//! being valid UTF-8 the moment the game has run once. Paths on this platform
//! are bytes too. Everything here works on bytes and copies through every line
//! it is not changing.

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
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
    let value = data.as_os_str().as_bytes();

    let existing = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(io(error)),
    };

    let updated = match setting.format {
        Format::Ini { section } => set_ini_key(&existing, section, setting.key, value),
        Format::Yaml => set_yaml_key(&existing, setting.key, value),
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

    let text = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io(error)),
    };

    let value = match setting.format {
        Format::Ini { section } => read_ini_key(&text, section, setting.key),
        Format::Yaml => read_yaml_key(&text, setting.key),
    };

    Ok(value.map(|bytes| PathBuf::from(OsString::from_vec(bytes))))
}

/// Replaces a file without ever leaving it half written. This is somebody's
/// settings: a crash between truncating and writing would lose the lot, so the
/// new text goes to a neighbouring file and is renamed over the old one.
fn write_replacing(path: &Path, text: &[u8]) -> Result<(), CoreError> {
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
fn line_ending(text: &[u8]) -> &'static [u8] {
    if text.windows(2).any(|pair| pair == b"\r\n") {
        b"\r\n"
    } else {
        b"\n"
    }
}

/// Splits into lines the way `str::lines` does, without requiring the bytes to
/// be text: on `\n`, dropping a `\r` before it, and without a trailing empty
/// line.
fn lines(text: &[u8]) -> Vec<&[u8]> {
    if text.is_empty() {
        return Vec::new();
    }

    let mut out: Vec<&[u8]> = text
        .split(|byte| *byte == b'\n')
        .map(|line| match line.split_last() {
            Some((b'\r', rest)) => rest,
            _ => line,
        })
        .collect();

    if out.last().is_some_and(|last| last.is_empty()) {
        out.pop();
    }

    out
}

fn trim(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(start, |at| at + 1);
    &bytes[start..end]
}

fn join(mut out: Vec<Vec<u8>>, ending: &[u8]) -> Vec<u8> {
    let mut result = Vec::new();
    for line in out.drain(..) {
        result.extend_from_slice(&line);
        result.extend_from_slice(ending);
    }
    result
}

/// The name in `[name]`, if the line is a section heading.
fn section_name(trimmed: &[u8]) -> Option<&[u8]> {
    let rest = trimmed.strip_prefix(b"[")?;
    let inner = rest.strip_suffix(b"]")?;
    Some(trim(inner))
}

/// Sets `key` in `section`, copying every other line through as it was.
fn set_ini_key(text: &[u8], section: &str, key: &str, value: &[u8]) -> Vec<u8> {
    let ending = line_ending(text);

    let mut line = Vec::new();
    line.extend_from_slice(key.as_bytes());
    line.extend_from_slice(b" = \"");
    line.extend_from_slice(&escape(value));
    line.push(b'"');

    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut current: Option<Vec<u8>> = None;
    let mut done = false;
    let mut end_of_section: Option<usize> = None;

    for raw in lines(text) {
        let trimmed = trim(raw);

        if let Some(name) = section_name(trimmed) {
            // Leaving the section we wanted without having found the key:
            // remember where it ended, so the key goes in at the bottom of it
            // rather than under somebody else's heading.
            if !done && current.as_deref() == Some(section.as_bytes()) && end_of_section.is_none() {
                end_of_section = Some(out.len());
            }
            current = Some(name.to_vec());
            out.push(raw.to_vec());
            continue;
        }

        if !done && current.as_deref() == Some(section.as_bytes()) && is_key_line(trimmed, key) {
            out.push(line.clone());
            done = true;
            continue;
        }

        out.push(raw.to_vec());
    }

    if !done {
        match end_of_section {
            Some(at) => out.insert(at, line),
            None if current.as_deref() == Some(section.as_bytes()) => out.push(line),
            None => {
                if !out.is_empty() {
                    out.push(Vec::new());
                }
                out.push(format!("[{section}]").into_bytes());
                out.push(line);
            }
        }
    }

    join(out, ending)
}

fn read_ini_key(text: &[u8], section: &str, key: &str) -> Option<Vec<u8>> {
    let mut current: Option<&[u8]> = None;

    for raw in lines(text) {
        let trimmed = trim(raw);

        if let Some(name) = section_name(trimmed) {
            current = Some(name);
            continue;
        }

        if current == Some(section.as_bytes()) && is_key_line(trimmed, key) {
            let at = trimmed.iter().position(|byte| *byte == b'=')?;
            return Some(unescape(trim(&trimmed[at + 1..])));
        }
    }

    None
}

/// Whether a line assigns `key`, allowing for the spacing people and
/// programs put around the equals sign.
fn is_key_line(trimmed: &[u8], key: &str) -> bool {
    trimmed
        .iter()
        .position(|byte| *byte == b'=')
        .is_some_and(|at| trim(&trimmed[..at]) == key.as_bytes())
}

/// Both files quote the same way, settled by giving OpenRCT2's own `set-rct2`
/// a directory whose name held each character and reading back what it wrote.
fn escape(value: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(value.len());
    for byte in value {
        if *byte == b'\\' || *byte == b'"' {
            out.push(b'\\');
        }
        out.push(*byte);
    }
    out
}

fn unescape(value: &[u8]) -> Vec<u8> {
    let value = match (value.first(), value.last()) {
        (Some(b'"'), Some(b'"')) if value.len() >= 2 => &value[1..value.len() - 1],
        _ => value,
    };

    let mut out = Vec::with_capacity(value.len());
    let mut escaped = false;
    for byte in value {
        if escaped {
            out.push(*byte);
            escaped = false;
        } else if *byte == b'\\' {
            escaped = true;
        } else {
            out.push(*byte);
        }
    }
    if escaped {
        out.push(b'\\');
    }
    out
}

/// Sets a top level `key`, copying every other line through as it was. Only a
/// key at the start of a line is replaced: the same name indented under
/// something else is a different setting.
fn set_yaml_key(text: &[u8], key: &str, value: &[u8]) -> Vec<u8> {
    let ending = line_ending(text);

    let mut line = Vec::new();
    line.extend_from_slice(key.as_bytes());
    line.extend_from_slice(b": \"");
    line.extend_from_slice(&escape(value));
    line.push(b'"');

    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut done = false;

    for raw in lines(text) {
        if !done && is_top_level_key(raw, key) {
            out.push(line.clone());
            done = true;
            continue;
        }
        out.push(raw.to_vec());
    }

    if !done {
        out.insert(0, line);
    }

    join(out, ending)
}

fn read_yaml_key(text: &[u8], key: &str) -> Option<Vec<u8>> {
    lines(text)
        .into_iter()
        .find(|raw| is_top_level_key(raw, key))
        .map(|raw| {
            let after = raw
                .iter()
                .position(|byte| *byte == b':')
                .map_or(&b""[..], |at| &raw[at + 1..]);
            unescape_yaml(trim(after))
        })
}

fn is_top_level_key(raw: &[u8], key: &str) -> bool {
    raw.strip_prefix(key.as_bytes())
        .is_some_and(|rest| trim(rest).first() == Some(&b':'))
}

fn unescape_yaml(value: &[u8]) -> Vec<u8> {
    if value.len() >= 2 && value.first() == Some(&b'\'') && value.last() == Some(&b'\'') {
        return value[1..value.len() - 1].to_vec();
    }
    unescape(value)
}

fn io(error: std::io::Error) -> CoreError {
    CoreError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{escape, read_ini_key, read_yaml_key, set_ini_key, set_yaml_key, unescape};

    const OPENRCT2: &[u8] = b"\
[general]
always_show_gridlines = false
rct1_path = \"/old/rct1\"
game_path = \"/old/rct2\"
language = en-US

[interface]
game_path = \"not this one\"
";

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    fn line_count(bytes: &[u8]) -> usize {
        bytes.iter().filter(|byte| **byte == b'\n').count()
    }

    #[test]
    fn a_key_is_replaced_where_it_stands() {
        let out = set_ini_key(OPENRCT2, "general", "game_path", b"/new/rct2");
        assert!(text(&out).contains("game_path = \"/new/rct2\""));
        assert_eq!(line_count(&out), line_count(OPENRCT2));
        assert_eq!(
            text(&out).lines().position(|l| l.starts_with("game_path")),
            text(OPENRCT2)
                .lines()
                .position(|l| l.starts_with("game_path")),
            "the line must not move"
        );
    }

    #[test]
    fn every_other_line_survives_untouched() {
        let out = text(&set_ini_key(OPENRCT2, "general", "game_path", b"/new/rct2"));
        for line in text(OPENRCT2)
            .lines()
            .filter(|l| !l.starts_with("game_path"))
        {
            assert!(out.contains(line), "lost: {line}");
        }
    }

    #[test]
    fn the_same_key_in_another_section_is_left_alone() {
        let out = text(&set_ini_key(OPENRCT2, "general", "game_path", b"/new/rct2"));
        assert!(
            out.contains("game_path = \"not this one\""),
            "[interface] was edited"
        );
    }

    #[test]
    fn a_missing_key_joins_its_section_rather_than_the_end_of_the_file() {
        let out = text(&set_ini_key(
            b"[general]\nlanguage = en-US\n\n[interface]\ntoolbar = true\n",
            "general",
            "game_path",
            b"/rct2",
        ));

        let game_path = out
            .lines()
            .position(|l| l.starts_with("game_path"))
            .unwrap();
        let interface = out.lines().position(|l| l == "[interface]").unwrap();
        assert!(game_path < interface, "landed in the wrong section:\n{out}");
    }

    #[test]
    fn a_missing_section_is_added() {
        let out = text(&set_ini_key(
            b"[interface]\ntoolbar = true\n",
            "general",
            "game_path",
            b"/rct2",
        ));
        assert!(out.contains("[general]"));
        assert!(out.contains("game_path = \"/rct2\""));
        assert!(out.contains("toolbar = true"));
    }

    #[test]
    fn an_empty_file_becomes_a_minimal_one() {
        let out = set_ini_key(b"", "general", "game_path", b"/rct2");
        assert_eq!(text(&out), "[general]\ngame_path = \"/rct2\"\n");
    }

    #[test]
    fn spacing_around_the_equals_sign_does_not_hide_the_key() {
        let out = text(&set_ini_key(
            b"[general]\n  game_path   =   \"/old\"\n",
            "general",
            "game_path",
            b"/new",
        ));
        assert!(out.contains("game_path = \"/new\""));
        assert!(!out.contains("/old"), "{out}");
    }

    #[test]
    fn a_quote_or_a_backslash_in_the_path_is_escaped_the_way_openrct2_writes_it() {
        assert_eq!(escape(br#"/a "b" c"#), br#"/a \"b\" c"#.to_vec());
        assert_eq!(escape(br"/a\b"), br"/a\\b".to_vec());
    }

    #[test]
    fn a_path_survives_being_written_and_read_back() {
        for path in [
            &b"/plain"[..],
            br#"/with "quotes""#,
            br"/with\backslash",
            b"/with spaces",
        ] {
            let out = set_ini_key(b"", "general", "game_path", path);
            assert_eq!(
                read_ini_key(&out, "general", "game_path").as_deref(),
                Some(path),
                "round trip failed for {}",
                text(path)
            );
        }
    }

    #[test]
    fn unescaping_a_value_that_was_never_escaped_leaves_it_alone() {
        assert_eq!(unescape(b"/plain/path"), b"/plain/path".to_vec());
    }

    #[test]
    fn a_file_with_windows_line_endings_keeps_them() {
        let out = set_ini_key(
            b"[general]\r\ngame_path = \"/old\"\r\n",
            "general",
            "game_path",
            b"/new",
        );
        assert!(!text(&out).contains("\n\n"), "a bare newline crept in");
        assert!(text(&out).contains("game_path = \"/new\"\r\n"));
    }

    const OPENLOCO: &[u8] = b"\
loco_install_path: \"/old/loco\"
display:
  mode: window
language: en-GB
";

    #[test]
    fn a_top_level_yaml_key_is_replaced_in_place() {
        let out = text(&set_yaml_key(OPENLOCO, "loco_install_path", b"/new/loco"));
        assert!(out.contains("loco_install_path: \"/new/loco\""));
        assert!(out.contains("  mode: window"));
        assert!(out.contains("language: en-GB"));
    }

    #[test]
    fn an_indented_key_of_the_same_name_is_not_the_one_we_mean() {
        let out = text(&set_yaml_key(
            b"display:\n  loco_install_path: nested\nlanguage: en-GB\n",
            "loco_install_path",
            b"/loco",
        ));

        assert!(out.contains("  loco_install_path: nested"), "{out}");
        assert!(out.starts_with("loco_install_path: \"/loco\""), "{out}");
    }

    #[test]
    fn a_missing_yaml_key_is_added_at_the_top() {
        let out = set_yaml_key(b"language: en-GB\n", "loco_install_path", b"/loco");
        assert_eq!(
            text(&out),
            "loco_install_path: \"/loco\"\nlanguage: en-GB\n"
        );
    }

    #[test]
    fn an_empty_yaml_file_becomes_a_single_line() {
        let out = set_yaml_key(b"", "loco_install_path", b"/loco");
        assert_eq!(text(&out), "loco_install_path: \"/loco\"\n");
    }

    #[test]
    fn a_yaml_path_survives_being_written_and_read_back() {
        for path in [
            &b"/plain"[..],
            br#"/with "quotes""#,
            b"/with: colon",
            b"/with #hash",
        ] {
            let out = set_yaml_key(b"", "loco_install_path", path);
            assert_eq!(
                read_yaml_key(&out, "loco_install_path").as_deref(),
                Some(path),
                "round trip failed for {}",
                text(path)
            );
        }
    }

    #[test]
    fn a_value_openloco_wrote_unquoted_is_read_back_whole() {
        let text_in = b"loco_install_path: /Users/someone/Game Data/locomotion\n";
        assert_eq!(
            read_yaml_key(text_in, "loco_install_path").as_deref(),
            Some(&b"/Users/someone/Game Data/locomotion"[..])
        );
    }

    /// The bytes OpenLoco writes for an owner face nobody has chosen. Its
    /// configuration stops being text the first time the game runs, so every
    /// install after the first one read it and failed with "stream did not
    /// contain valid UTF-8".
    const OPENLOCO_AFTER_RUNNING: &[u8] =
        b"loco_install_path: \"/old/loco\"\npreferredOwnerFace:\n  flags: 4294967295\n  name: \xff\xff\xff\xff\xff\xff\xff\xff\n  checksum: 4294967295\nlanguage: en-GB\n";

    #[test]
    fn a_configuration_that_is_not_text_is_still_edited() {
        let out = set_yaml_key(OPENLOCO_AFTER_RUNNING, "loco_install_path", b"/new/loco");

        assert_eq!(
            read_yaml_key(&out, "loco_install_path").as_deref(),
            Some(&b"/new/loco"[..])
        );
    }

    #[test]
    fn the_bytes_that_are_not_text_come_through_unchanged() {
        let out = set_yaml_key(OPENLOCO_AFTER_RUNNING, "loco_install_path", b"/new/loco");

        assert!(
            out.windows(8).any(|window| window == [0xff; 8]),
            "the raw bytes were mangled"
        );
        assert!(
            String::from_utf8(out.clone()).is_err(),
            "it should still not be text"
        );
        for line in [
            &b"  flags: 4294967295"[..],
            b"  checksum: 4294967295",
            b"language: en-GB",
        ] {
            assert!(
                out.windows(line.len()).any(|window| window == line),
                "lost: {}",
                text(line)
            );
        }
    }

    #[test]
    fn a_path_that_is_not_text_is_written_and_read_back_whole() {
        let path = b"/games/\xff\xfe/loco";
        let out = set_yaml_key(b"", "loco_install_path", path);
        assert_eq!(
            read_yaml_key(&out, "loco_install_path").as_deref(),
            Some(&path[..])
        );
    }
}
