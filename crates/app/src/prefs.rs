//! Settings persisted over `NSUserDefaults` on an **explicit suite**, not the
//! standard bundle-keyed defaults: it gives a clean
//! `~/Library/Preferences/Turnstile.plist`, and the standard defaults key on
//! the bundle identifier, which is absent under `cargo run`, so bundle-keyed
//! storage would silently split development and bundled runs into two domains.

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSString, NSUserDefaults};
use turnstile_core::game::GameId;
use turnstile_core::store::Mode;

use crate::state::Pref;

pub const KEY_SHOW_DEVELOP: &str = "showDevelopmentVersions";
pub const KEY_CHECK_FOR_UPDATES: &str = "checkForUpdates";
pub const KEY_MULTI_VERSION: &str = "keepMultipleVersions";
pub const KEY_SELECTED_GAME: &str = "selectedGame";
pub const KEY_INSTALL_ROOT: &str = "gameDataDirectory";

/// `game_key` is always `GameId::key()`, never a display string.
pub fn auto_update_key(game_key: &str) -> String {
    format!("autoInstallUpdates.{game_key}")
}

pub fn mode_for(multi_version: bool) -> Mode {
    if multi_version {
        Mode::MultiVersion
    } else {
        Mode::Compatible
    }
}

pub struct Prefs {
    defaults: Retained<NSUserDefaults>,
    /// `NSUserDefaults` never hands this back, but `remove_all` (test-only)
    /// needs it to remove the whole domain rather than key-by-key.
    #[cfg_attr(not(test), allow(dead_code))]
    suite: String,
}

impl Prefs {
    pub fn new() -> Prefs {
        Prefs::with_suite("Turnstile")
    }

    pub fn with_suite(suite: &str) -> Prefs {
        let name = NSString::from_str(suite);
        let defaults = NSUserDefaults::initWithSuiteName(NSUserDefaults::alloc(), Some(&name))
            .expect("NSUserDefaults suite could not be opened");
        Prefs {
            defaults,
            suite: suite.to_string(),
        }
    }

    pub fn get_bool(&self, key: &str) -> bool {
        self.defaults.boolForKey(&NSString::from_str(key))
    }

    pub fn set_bool(&self, key: &str, value: bool) {
        self.defaults
            .setBool_forKey(value, &NSString::from_str(key));
    }

    // Generic round-trip coverage only: no preference is stored as a plain
    // integer, so nothing outside this module's own tests calls these.
    #[cfg(test)]
    fn get_int(&self, key: &str) -> i64 {
        self.defaults.integerForKey(&NSString::from_str(key)) as i64
    }

    #[cfg(test)]
    fn set_int(&self, key: &str, value: i64) {
        self.defaults
            .setInteger_forKey(value as isize, &NSString::from_str(key));
    }

    pub fn get_string(&self, key: &str) -> Option<String> {
        self.defaults
            .stringForKey(&NSString::from_str(key))
            .map(|s| s.to_string())
    }

    pub fn remove(&self, key: &str) {
        self.defaults.removeObjectForKey(&NSString::from_str(key));
    }

    pub fn set_string(&self, key: &str, value: &str) {
        let obj: Retained<NSString> = NSString::from_str(value);
        let obj: &AnyObject = &obj;
        // SAFETY: the value is an `NSString`, exactly the type
        // `-setObject:forKey:` requires for a key read back with
        // `-stringForKey:`.
        unsafe {
            self.defaults
                .setObject_forKey(Some(obj), &NSString::from_str(key))
        };
    }

    /// Removes the whole domain `self` was opened with. Test-only: production
    /// code has no reason to wipe a user's settings wholesale.
    ///
    /// This clears the domain's *contents*, but an empty
    /// `~/Library/Preferences/<suite>.plist` may still be left on disk:
    /// `cfprefsd` re-flushes a domain it has touched after the owning process
    /// exits, and `NSUserDefaults` has no API that removes the file itself.
    ///
    /// A `std::fs::remove_file` was tried here and withdrawn: the file
    /// reliably came back within about fifteen seconds. It also brought a real
    /// hazard, since `#[cfg(test)]` was the only thing stopping it deleting
    /// the developer's real settings.
    #[cfg(test)]
    pub fn remove_all(&self) {
        self.defaults
            .removePersistentDomainForName(&NSString::from_str(&self.suite));
        self.defaults.synchronize();
    }

    /// Defaults to true, which is why this cannot use `get_bool`: an absent
    /// key reads as false there, leaving the check off for everyone who has
    /// never opened Settings.
    pub fn check_for_updates(&self) -> bool {
        match self
            .defaults
            .objectForKey(&NSString::from_str(KEY_CHECK_FOR_UPDATES))
        {
            Some(_) => self.get_bool(KEY_CHECK_FOR_UPDATES),
            None => true,
        }
    }

    pub fn show_develop(&self) -> bool {
        self.get_bool(KEY_SHOW_DEVELOP)
    }

    pub fn multi_version(&self) -> bool {
        self.get_bool(KEY_MULTI_VERSION)
    }

    pub fn auto_update(&self, game: GameId) -> bool {
        self.get_bool(&auto_update_key(game.key()))
    }

    /// Falls back to the first game when the stored key is absent or names a
    /// game this build does not have. Storing `GameId::key()` rather than an
    /// index is what makes that safe: an index would silently point at a
    /// different game the moment the list changes order.
    pub fn selected_game(&self) -> GameId {
        self.get_string(KEY_SELECTED_GAME)
            .and_then(|s| GameId::from_key(&s))
            .unwrap_or(GameId::ALL[0])
    }

    /// Where game data is unpacked, or `None` for the default. Absent and empty
    /// both mean the default: an empty string is what a hand-edited preference
    /// tends to end up as, and it is not a usable path.
    pub fn install_root(&self) -> Option<std::path::PathBuf> {
        self.get_string(KEY_INSTALL_ROOT)
            .filter(|path| !path.trim().is_empty())
            .map(std::path::PathBuf::from)
    }

    pub fn save(&self, pref: Pref, game: GameId) {
        match pref {
            Pref::CheckForUpdates(v) => self.set_bool(KEY_CHECK_FOR_UPDATES, v),
            Pref::ShowDevelop(v) => self.set_bool(KEY_SHOW_DEVELOP, v),
            Pref::MultiVersion(v) => self.set_bool(KEY_MULTI_VERSION, v),
            Pref::SelectedGame(v) => self.set_string(KEY_SELECTED_GAME, v.key()),
            Pref::AutoUpdate(v) => self.set_bool(&auto_update_key(game.key()), v),
            Pref::InstallRoot(Some(path)) => self.set_string(KEY_INSTALL_ROOT, &path),
            // Removed rather than stored empty, so the default is the absence
            // of an answer and not a second spelling of one.
            Pref::InstallRoot(None) => self.remove(KEY_INSTALL_ROOT),
        }
    }
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_game_keys_are_built_from_the_games_stable_key() {
        assert_eq!(
            auto_update_key(GameId::OpenRCT2.key()),
            "autoInstallUpdates.OpenRCT2"
        );
        assert_eq!(
            auto_update_key(GameId::OpenLoco.key()),
            "autoInstallUpdates.OpenLoco"
        );
    }

    #[test]
    fn an_unrecognised_key_still_gets_its_own_distinct_preference_key() {
        assert_eq!(auto_update_key("Something"), "autoInstallUpdates.Something");
    }

    #[test]
    fn the_multi_version_preference_maps_to_the_store_mode() {
        assert_eq!(mode_for(false), Mode::Compatible);
        assert_eq!(mode_for(true), Mode::MultiVersion);
    }

    #[test]
    fn a_round_trip_through_the_real_defaults_preserves_values() {
        let p = Prefs::with_suite("TurnstileTestRoundTrip");
        p.remove_all();
        p.set_bool(KEY_MULTI_VERSION, true);
        assert!(p.get_bool(KEY_MULTI_VERSION));
        p.set_bool(KEY_MULTI_VERSION, false);
        assert!(!p.get_bool(KEY_MULTI_VERSION));
        p.set_int(KEY_SELECTED_GAME, 1);
        assert_eq!(p.get_int(KEY_SELECTED_GAME), 1);
        p.remove_all();
    }

    #[test]
    fn absent_keys_return_false_and_zero_so_a_first_run_needs_no_special_case() {
        let p = Prefs::with_suite("TurnstileTestEmpty");
        p.remove_all();
        assert!(!p.get_bool(KEY_MULTI_VERSION));
        assert!(!p.get_bool(KEY_SHOW_DEVELOP));
        assert_eq!(p.get_int(KEY_SELECTED_GAME), 0);
    }

    #[test]
    fn selected_game_falls_back_to_the_first_game_when_the_key_is_absent() {
        let p = Prefs::with_suite("TurnstileTestSelectedGameAbsent");
        p.remove_all();
        assert_eq!(p.selected_game(), GameId::ALL[0]);
    }

    #[test]
    fn selected_game_falls_back_to_the_first_game_for_an_unrecognised_stored_key() {
        let p = Prefs::with_suite("TurnstileTestSelectedGameUnknown");
        p.remove_all();
        p.set_string(KEY_SELECTED_GAME, "NotAGame");
        assert_eq!(p.selected_game(), GameId::ALL[0]);
        p.remove_all();
    }

    #[test]
    fn save_and_selected_game_round_trip_through_the_stable_key_not_an_index() {
        let p = Prefs::with_suite("TurnstileTestSelectedGameRoundTrip");
        p.remove_all();
        p.save(Pref::SelectedGame(GameId::OpenLoco), GameId::OpenLoco);
        assert_eq!(p.selected_game(), GameId::OpenLoco);
        p.remove_all();
    }

    #[test]
    fn auto_update_is_saved_per_game_not_globally() {
        let p = Prefs::with_suite("TurnstileTestAutoUpdate");
        p.remove_all();
        p.save(Pref::AutoUpdate(true), GameId::OpenRCT2);
        assert!(p.auto_update(GameId::OpenRCT2));
        assert!(!p.auto_update(GameId::OpenLoco));

        p.save(Pref::AutoUpdate(true), GameId::OpenLoco);
        assert!(p.auto_update(GameId::OpenRCT2));
        assert!(p.auto_update(GameId::OpenLoco));
        p.save(Pref::AutoUpdate(false), GameId::OpenRCT2);
        assert!(!p.auto_update(GameId::OpenRCT2));
        assert!(p.auto_update(GameId::OpenLoco));
        p.remove_all();
    }
}
