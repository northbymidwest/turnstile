use std::fmt;
use std::path::{Path, PathBuf};

use crate::dirs::Dirs;
use crate::error::TagError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryName {
    pub owner: String,
    pub name: String,
}

impl RepositoryName {
    fn new(owner: &str, name: &str) -> Self {
        RepositoryName {
            owner: owner.to_string(),
            name: name.to_string(),
        }
    }
}

impl fmt::Display for RepositoryName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

/// The games this launcher knows about. Everything else about a game
/// derives from this, so there is no way to name one that does not exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GameId {
    OpenRCT2,
    OpenLoco,
}

impl GameId {
    pub const ALL: [GameId; 2] = [GameId::OpenRCT2, GameId::OpenLoco];

    /// Also the on-disk directory name and the `.app` bundle name. The
    /// compatibility contract depends on this exact spelling.
    pub fn display_name(self) -> &'static str {
        match self {
            GameId::OpenRCT2 => "OpenRCT2",
            GameId::OpenLoco => "OpenLoco",
        }
    }

    /// Stable identifier for preferences. Persisting this instead of an
    /// index means adding or reordering games cannot silently change a
    /// user's saved selection.
    pub fn key(self) -> &'static str {
        self.display_name()
    }

    pub fn from_key(key: &str) -> Option<GameId> {
        GameId::ALL.into_iter().find(|g| g.key() == key)
    }

    /// This game's position in `ALL`, so a caller can hold one value per
    /// game in a plain array. A `match` rather than a search through `ALL`:
    /// adding a game then fails to compile here instead of silently
    /// returning the wrong slot, and the result needs no bounds check.
    ///
    /// Never persisted. `key` exists for that, precisely because an index
    /// changes meaning when the list is reordered.
    pub fn index(self) -> usize {
        match self {
            GameId::OpenRCT2 => 0,
            GameId::OpenLoco => 1,
        }
    }

    pub fn release_repo(self) -> RepositoryName {
        match self {
            GameId::OpenRCT2 => RepositoryName::new("OpenRCT2", "OpenRCT2"),
            GameId::OpenLoco => RepositoryName::new("OpenLoco", "OpenLoco"),
        }
    }

    pub fn develop_repo(self) -> Option<RepositoryName> {
        match self {
            GameId::OpenRCT2 => Some(RepositoryName::new("OpenRCT2", "OpenRCT2-binaries")),
            GameId::OpenLoco => None,
        }
    }

    pub fn game_path(self, dirs: &Dirs) -> PathBuf {
        dirs.app_support.join(self.display_name())
    }

    pub fn bin_path(self, dirs: &Dirs) -> PathBuf {
        self.game_path(dirs).join("bin")
    }

    pub fn versions_path(self, dirs: &Dirs) -> PathBuf {
        self.game_path(dirs).join("versions")
    }

    /// `tag` must already have passed `sanitize_tag`.
    pub fn version_path(self, dirs: &Dirs, tag: &str) -> PathBuf {
        self.versions_path(dirs).join(tag)
    }

    pub fn executable_in(self, dir: &Path) -> PathBuf {
        let name = self.display_name();
        dir.join(format!("{name}.app"))
            .join("Contents/MacOS")
            .join(name)
    }
}

pub fn version_file_in(dir: &Path) -> PathBuf {
    dir.join(".version")
}

/// Release tags become directory names, and a tag is remote input, so it has
/// no business escaping `versions/`.
pub fn sanitize_tag(tag: &str) -> Result<String, TagError> {
    if tag.is_empty() {
        return Err(TagError::Empty);
    }
    let mut cleaned: String = tag
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if cleaned == "." || cleaned == ".." {
        return Err(TagError::Reserved);
    }
    // A leading dot is this crate's reserved namespace for staging, trash
    // and marker files (`.staging-`, `.trash-`, ...); a sanitized tag that
    // began with one would also fall outside `installed_multi`'s directory
    // listing, which skips dot-prefixed names. Replace rather than reject:
    // only the shape is hostile here, not the tag's identity.
    if cleaned.starts_with('.') {
        cleaned.replace_range(0..1, "-");
    }
    Ok(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dirs::Dirs;

    fn dirs() -> Dirs {
        Dirs::at("/tmp/turnstile-test")
    }

    #[test]
    fn every_game_is_reachable_through_all() {
        assert_eq!(GameId::ALL.len(), 2);
        assert!(GameId::ALL.contains(&GameId::OpenRCT2));
        assert!(GameId::ALL.contains(&GameId::OpenLoco));
    }

    #[test]
    fn openrct2_has_a_develop_repository() {
        let g = GameId::OpenRCT2;
        assert_eq!(g.display_name(), "OpenRCT2");
        assert_eq!(g.release_repo().to_string(), "OpenRCT2/OpenRCT2");
        assert_eq!(
            g.develop_repo().map(|r| r.to_string()),
            Some("OpenRCT2/OpenRCT2-binaries".to_string())
        );
    }

    #[test]
    fn openloco_has_no_develop_repository() {
        let g = GameId::OpenLoco;
        assert_eq!(g.display_name(), "OpenLoco");
        assert_eq!(g.release_repo().to_string(), "OpenLoco/OpenLoco");
        assert!(g.develop_repo().is_none());
    }

    #[test]
    fn keys_round_trip() {
        for g in GameId::ALL {
            assert_eq!(GameId::from_key(g.key()), Some(g));
        }
    }

    /// What every per-game array indexed by `index` relies on: the slot a
    /// game names is the slot `ALL` holds it in, for every game and with no
    /// slot shared.
    #[test]
    fn index_addresses_the_games_own_slot_in_all() {
        for g in GameId::ALL {
            assert_eq!(GameId::ALL[g.index()], g);
            assert!(g.index() < GameId::ALL.len());
        }
    }

    #[test]
    fn an_unknown_key_is_rejected_rather_than_guessed() {
        // This is what protects a saved selection when the game list changes.
        assert_eq!(GameId::from_key("OpenTTD"), None);
        assert_eq!(GameId::from_key(""), None);
        assert_eq!(GameId::from_key("0"), None);
    }

    #[test]
    fn paths_are_rooted_at_the_injected_dirs() {
        let g = GameId::OpenRCT2;
        let d = dirs();
        assert_eq!(g.game_path(&d), Path::new("/tmp/turnstile-test/OpenRCT2"));
        assert_eq!(
            g.bin_path(&d),
            Path::new("/tmp/turnstile-test/OpenRCT2/bin")
        );
        assert_eq!(
            g.versions_path(&d),
            Path::new("/tmp/turnstile-test/OpenRCT2/versions")
        );
        assert_eq!(
            g.version_path(&d, "v0.4.29"),
            Path::new("/tmp/turnstile-test/OpenRCT2/versions/v0.4.29")
        );
    }

    #[test]
    fn the_executable_lives_inside_a_capitalized_app_bundle() {
        assert_eq!(
            GameId::OpenRCT2.executable_in(Path::new("/x")),
            Path::new("/x/OpenRCT2.app/Contents/MacOS/OpenRCT2")
        );
        assert_eq!(
            GameId::OpenLoco.executable_in(Path::new("/x")),
            Path::new("/x/OpenLoco.app/Contents/MacOS/OpenLoco")
        );
    }

    #[test]
    fn the_version_file_is_a_dotfile_beside_the_bundle() {
        assert_eq!(version_file_in(Path::new("/x")), Path::new("/x/.version"));
    }

    #[test]
    fn ordinary_tags_survive_sanitization_unchanged() {
        assert_eq!(sanitize_tag("v0.4.29").unwrap(), "v0.4.29");
        assert_eq!(sanitize_tag("v26.08").unwrap(), "v26.08");
        assert_eq!(
            sanitize_tag("v0.5.5-9-g394e588fc7").unwrap(),
            "v0.5.5-9-g394e588fc7"
        );
    }

    #[test]
    fn unsafe_characters_are_replaced() {
        assert_eq!(sanitize_tag("release/1.0").unwrap(), "release-1.0");
        assert_eq!(sanitize_tag("a b").unwrap(), "a-b");
        assert_eq!(sanitize_tag("x\u{0000}y").unwrap(), "x-y");
    }

    #[test]
    fn reserved_and_empty_names_are_rejected() {
        assert!(matches!(sanitize_tag("."), Err(TagError::Reserved)));
        assert!(matches!(sanitize_tag(".."), Err(TagError::Reserved)));
        assert!(matches!(sanitize_tag(""), Err(TagError::Empty)));
    }

    #[test]
    fn a_hostile_tag_cannot_escape_the_versions_directory() {
        let g = GameId::OpenRCT2;
        let d = dirs();
        let tag = sanitize_tag("../../etc/passwd").unwrap();
        let p = g.version_path(&d, &tag);
        assert_eq!(p.parent().unwrap(), g.versions_path(&d));
        assert_eq!(
            p.components().count(),
            g.versions_path(&d).components().count() + 1
        );
    }

    #[test]
    fn a_sanitized_tag_never_begins_with_a_dot() {
        // A leading dot would land in this crate's reserved namespace for
        // staging/trash/marker files, and would make the directory invisible
        // to `installed_multi`'s dot-prefix filter.
        assert_eq!(sanitize_tag(".foo").unwrap(), "-foo");
        assert!(!sanitize_tag(".foo").unwrap().starts_with('.'));
        assert!(!sanitize_tag("../../etc/passwd").unwrap().starts_with('.'));
    }
}
