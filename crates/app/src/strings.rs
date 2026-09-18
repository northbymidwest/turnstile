//! Localized strings for the interface. Upstream's `.resx` files were
//! converted once by `scripts/import-resx.sh` into
//! `resources/<lang>.lproj/Localizable.strings`, which is checked in and
//! copied into the app bundle at build time. This module is the only thing
//! that reads them.
//!
//! `Age` and `Status` are enums in `turnstile-core` precisely so this layer
//! owns all grammar and wording. Do not add formatting logic to core.
//!
//! `{0}`-style placeholders are kept exactly as upstream wrote them, so future
//! translation updates drop straight into `scripts/import-resx.sh`'s output.

use objc2_foundation::{NSBundle, NSString};
use turnstile_core::age::Age;
use turnstile_core::download::Status;

/// English fallbacks compiled into the binary. `cargo run` and `cargo test`
/// have no app bundle around them, so `NSBundle::mainBundle()` finds no
/// `.lproj` and this table is what renders during development.
///
/// It is also the last line of defence in a packaged build: `NSBundle` returns
/// the *key itself* for any lookup that misses, which would put a raw
/// identifier like "StatusDownloading" in the UI.
pub const BUILTIN_EN: &[(&str, &str)] = &[
    ("InstalledLabel", "Installed:"),
    ("AvailableLabel", "Available:"),
    ("Unknown", "(Unknown)"),
    ("Play", "Play"),
    ("Download", "Download"),
    ("Remove", "Remove"),
    ("Cancel", "Cancel"),
    ("Update", "Update"),
    ("Dismiss", "Dismiss"),
    ("Settings", "Settings"),
    ("CheckForUpdates", "Check for updates on launch"),
    (
        "CheckForUpdatesDetail",
        "Asks GitHub whether a newer Turnstile has been released. Nothing is downloaded or installed.",
    ),
    ("UpdateAvailable", "Turnstile {0} is available."),
    ("GameData", "Game data"),
    ("GameDataNotInstalled", "Not installed"),
    ("GameDataOptional", "optional"),
    ("InstallGameData", "Install\u{2026}"),
    ("ChangeGameData", "Change\u{2026}"),
    ("ReinstallGameData", "Reinstall\u{2026}"),
    (
        "ChooseInstaller",
        "Choose the installer you downloaded from GOG.",
    ),
    (
        "ChooseInstallDirectory",
        "Choose where Turnstile unpacks game data.",
    ),
    ("Install", "Install"),
    ("Choose", "Choose"),
    ("FailedToInstallGameData", "Failed to install the game data"),
    (
        "UnrecognisedInstaller",
        "That installer does not hold RollerCoaster Tycoon 1 or 2, or Locomotion.",
    ),
    ("InstallDirectory", "Game data folder"),
    (
        "InstallDirectoryDetail",
        "Where the original games' data is unpacked. The games are pointed at it automatically.",
    ),
    ("UseDefault", "Use Default"),
    ("DefaultDirectory", "Default"),
    ("ShowDevelopmentVersions", "Show development versions"),
    ("AutoUpdateGame", "Automatically install updates"),
    ("KeepMultipleVersions", "Keep multiple versions installed"),
    (
        "KeepMultipleVersionsDetail",
        "Install several builds side by side and switch between them instantly. Changes how builds are stored on disk.",
    ),
    ("RemoveConfirmTitle", "Remove {0}?"),
    (
        "RemoveConfirmMessage",
        "This deletes the installed build from disk. Saved games, scenarios, and settings are not affected.",
    ),
    (
        "RemoveBusyMessage",
        "Other work was in progress, so nothing was removed. Try again once it has finished.",
    ),
    ("ActiveVersion", "{0} (active)"),
    ("MultiVersionDisabledTitle", "Now using {0}"),
    (
        "MultiVersionDisabledMessage",
        "{0} other versions are still on disk ({1}). Nothing was deleted. Turn this setting back on to use them again.",
    ),
    (
        "MultiVersionDisabledMessageOne",
        "1 other version is still on disk ({0}). Nothing was deleted. Turn this setting back on to use it again.",
    ),
    ("MultiVersionDisabledNoneTitle", "No version is in use"),
    ("BuildListing", "{0} (released {1})"),
    ("StatusDownloading", "Downloading\u{2026}"),
    ("StatusExtracting", "Extracting\u{2026}"),
    ("FailedToLaunchGame", "Failed to launch {0}"),
    ("FailedToSwitchVersion", "Failed to switch version"),
    ("FailedToRemoveVersion", "Failed to remove version"),
    (
        "FailedToReadInstalledVersions",
        "Failed to read installed versions",
    ),
    (
        "FailedToRepairInstallation",
        "Failed to repair the installation",
    ),
    ("FailedToObtainBuilds", "Failed to obtain builds"),
    (
        "FailedToChangeVersionMode",
        "Failed to change how versions are stored",
    ),
    ("DownloadBuildFailedTitle", "Failed to download build"),
    ("Minute", "a minute ago"),
    ("Minutes", "{0} minutes ago"),
    ("Hour", "an hour ago"),
    ("Hours", "{0} hours ago"),
    ("Day", "a day ago"),
    ("Days", "{0} days ago"),
    ("Month", "a month ago"),
    ("Months", "{0} months ago"),
    ("Year", "a year ago"),
    ("Years", "{0} years ago"),
];

/// Keys the interface actually asks for. The test below asserts every one has
/// a builtin, so a typo fails the build rather than shipping a raw identifier.
/// Read only by that test.
#[cfg_attr(not(test), allow(dead_code))]
pub const REQUIRED_KEYS: &[&str] = &[
    "Update",
    "Dismiss",
    "Settings",
    "CheckForUpdates",
    "CheckForUpdatesDetail",
    "UpdateAvailable",
    "GameData",
    "GameDataNotInstalled",
    "GameDataOptional",
    "InstallGameData",
    "ChangeGameData",
    "ReinstallGameData",
    "ChooseInstaller",
    "ChooseInstallDirectory",
    "Install",
    "Choose",
    "FailedToInstallGameData",
    "UnrecognisedInstaller",
    "InstallDirectory",
    "InstallDirectoryDetail",
    "UseDefault",
    "DefaultDirectory",
    "InstalledLabel",
    "AvailableLabel",
    "Play",
    "Download",
    "Remove",
    "Cancel",
    "ShowDevelopmentVersions",
    "AutoUpdateGame",
    "KeepMultipleVersions",
    "KeepMultipleVersionsDetail",
    "RemoveConfirmTitle",
    "RemoveConfirmMessage",
    "RemoveBusyMessage",
    "ActiveVersion",
    "MultiVersionDisabledTitle",
    "MultiVersionDisabledMessage",
    "MultiVersionDisabledMessageOne",
    "MultiVersionDisabledNoneTitle",
    "BuildListing",
    "StatusDownloading",
    "StatusExtracting",
    "FailedToLaunchGame",
    "FailedToSwitchVersion",
    "FailedToRemoveVersion",
    "FailedToReadInstalledVersions",
    "FailedToRepairInstallation",
    "FailedToObtainBuilds",
    "FailedToChangeVersionMode",
    "DownloadBuildFailedTitle",
    "Unknown",
    "Minute",
    "Minutes",
    "Hour",
    "Hours",
    "Day",
    "Days",
    "Month",
    "Months",
    "Year",
    "Years",
];

/// Looks a key up in the bundle, falling back to the compiled-in English.
pub fn get(key: &str) -> String {
    let ns_key = NSString::from_str(key);
    let bundle = NSBundle::mainBundle();
    // `localizedStringForKey:value:table:` returns the key itself when nothing
    // matches, which is how a miss is detected.
    let found = bundle.localizedStringForKey_value_table(&ns_key, None, None);
    let found = found.to_string();
    if found != key {
        return found;
    }
    BUILTIN_EN
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.to_string())
        .unwrap_or_else(|| key.to_string())
}

/// .NET's `{0}` placeholders, kept verbatim. An index with no corresponding
/// argument is left in the output rather than substituted with garbage.
pub fn substitute(template: &str, args: &[&str]) -> String {
    let mut out = template.to_string();
    for (i, arg) in args.iter().enumerate() {
        out = out.replace(&format!("{{{i}}}"), arg);
    }
    out
}

pub fn format1(key: &str, a: &str) -> String {
    substitute(&get(key), &[a])
}

pub fn format2(key: &str, a: &str, b: &str) -> String {
    substitute(&get(key), &[a, b])
}

pub fn age(age: Age) -> String {
    match age {
        Age::Minute => get("Minute"),
        Age::Minutes(n) => format1("Minutes", &n.to_string()),
        Age::Hour => get("Hour"),
        Age::Hours(n) => format1("Hours", &n.to_string()),
        Age::Day => get("Day"),
        Age::Days(n) => format1("Days", &n.to_string()),
        Age::Month => get("Month"),
        Age::Months(n) => format1("Months", &n.to_string()),
        Age::Year => get("Year"),
        Age::Years(n) => format1("Years", &n.to_string()),
    }
}

pub fn status(status: Status) -> String {
    match status {
        Status::Downloading => get("StatusDownloading"),
        Status::Extracting => get("StatusExtracting"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use turnstile_core::age::Age;

    #[test]
    fn a_missing_key_falls_back_to_the_key_itself_rather_than_panicking() {
        assert_eq!(get("NoSuchKeyAnywhere"), "NoSuchKeyAnywhere");
    }

    #[test]
    fn a_key_missing_from_the_bundle_falls_back_to_the_builtin_english_text() {
        assert_eq!(get("StatusDownloading"), "Downloading\u{2026}");
        assert_eq!(get("BuildListing"), "{0} (released {1})");
    }

    #[test]
    fn status_produces_the_builtin_english_wording_for_every_variant() {
        assert_eq!(status(Status::Downloading), "Downloading\u{2026}");
        assert_eq!(status(Status::Extracting), "Extracting\u{2026}");
    }

    #[test]
    fn the_builtin_english_table_covers_every_key_the_ui_uses() {
        for key in REQUIRED_KEYS {
            assert!(
                BUILTIN_EN.iter().any(|(k, _)| k == key),
                "no builtin English string for {key}"
            );
        }
    }

    /// Every key appearing in a `.strings` file, read straight off disk rather
    /// than through `NSBundle`, so this also catches a shipped `.lproj` that is
    /// missing, malformed, or never regenerated after `REQUIRED_KEYS` grew.
    fn keys_in_strings_file(path: &std::path::Path) -> std::collections::HashSet<String> {
        let content = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        content
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                let rest = line.strip_prefix('"')?;
                let end = rest.find('"')?;
                Some(rest[..end].to_string())
            })
            .collect()
    }

    #[test]
    fn every_shipped_language_carries_every_required_key() {
        let resources_dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../resources");
        let entries = std::fs::read_dir(&resources_dir)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", resources_dir.display()));

        let mut lproj_count = 0;
        for entry in entries {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("lproj") {
                continue;
            }
            lproj_count += 1;

            let strings_path = path.join("Localizable.strings");
            let keys = keys_in_strings_file(&strings_path);
            for key in REQUIRED_KEYS {
                assert!(
                    keys.contains(*key),
                    "{} is missing required key {key}",
                    strings_path.display()
                );
            }
        }
        assert_eq!(
            lproj_count,
            9,
            "expected all nine shipped .lproj directories in {}",
            resources_dir.display()
        );
    }

    /// Every key any module asks for, read out of this crate's own source. The
    /// scan matches `strings::get("...")` and its `format*` siblings, which is
    /// how every caller outside this module spells it.
    fn keys_used_in_source(dir: &std::path::Path) -> Vec<(String, String)> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", dir.display()))
        {
            let path = entry.unwrap().path();
            if path.is_dir() {
                found.extend(keys_used_in_source(&path));
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            if path.file_name().and_then(|f| f.to_str()) == Some("strings.rs") {
                continue;
            }
            let content = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
            for call in ["strings::get(", "strings::format1(", "strings::format2("] {
                for (offset, _) in content.match_indices(call) {
                    // rustfmt breaks a long call after the paren, so the key is not
                    // always the next character.
                    let rest = content[offset + call.len()..].trim_start();
                    let Some(rest) = rest.strip_prefix('"') else {
                        continue;
                    };
                    if let Some(end) = rest.find('"') {
                        found.push((rest[..end].to_string(), path.display().to_string()));
                    }
                }
            }
        }
        found
    }

    #[test]
    fn every_key_the_source_asks_for_is_one_we_ship() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let used = keys_used_in_source(&src);
        assert!(
            used.len() > 20,
            "the scan found almost nothing, so it is broken"
        );
        for (key, file) in used {
            assert!(
                REQUIRED_KEYS.contains(&key.as_str()),
                "{file} asks for {key}, which is not in REQUIRED_KEYS"
            );
        }
    }

    #[test]
    fn placeholders_are_substituted_positionally() {
        assert_eq!(substitute("{0} and {1}", &["a", "b"]), "a and b");
        assert_eq!(substitute("{0} twice {0}", &["x"]), "x twice x");
    }

    #[test]
    fn a_placeholder_with_no_argument_is_left_alone_rather_than_producing_garbage() {
        assert_eq!(substitute("{0} and {1}", &["only"]), "only and {1}");
    }

    #[test]
    fn a_template_with_no_placeholders_is_returned_unchanged() {
        assert_eq!(substitute("plain text", &["ignored"]), "plain text");
    }

    #[test]
    fn every_age_variant_produces_a_non_empty_string() {
        let all = [
            Age::Minute,
            Age::Minutes(5),
            Age::Hour,
            Age::Hours(5),
            Age::Day,
            Age::Days(5),
            Age::Month,
            Age::Months(5),
            Age::Year,
            Age::Years(5),
        ];
        for a in all {
            let s = age(a);
            assert!(!s.is_empty(), "{a:?} produced an empty string");
            assert!(
                !s.contains("{0}"),
                "{a:?} left an unsubstituted placeholder: {s}"
            );
        }
    }
}
