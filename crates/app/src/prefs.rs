//! Settings persisted over `NSUserDefaults` on an **explicit suite**, not
//! the standard bundle-keyed defaults. Two reasons, both load-bearing: it
//! gives a clean `~/Library/Preferences/Turnstile.plist`, and the standard
//! defaults key on the bundle identifier, which is absent under
//! `cargo run`, so bundle-keyed storage would silently put development and
//! bundled runs in different domains.

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSString, NSUserDefaults};
use turnstile_core::game::GameId;
use turnstile_core::store::Mode;

use crate::state::Pref;

pub const KEY_SHOW_DEVELOP: &str = "showDevelopmentVersions";
pub const KEY_MULTI_VERSION: &str = "keepMultipleVersions";
pub const KEY_SELECTED_GAME: &str = "selectedGame";

/// `game_key` is always `GameId::key()`, never a display string: the key is
/// derived from the enum at every call site, not assembled from a name and
/// resolved by reflection the way upstream does.
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
    /// `NSUserDefaults` never hands this back, and `remove_all` (test-only)
    /// needs it to remove the whole domain rather than key-by-key. Unread
    /// outside that test-only path, so it is dead weight in a production
    /// build.
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

    // Generic round-trip coverage only: no preference is actually stored as
    // a plain integer (`selected_game` uses `GameId::key()` via
    // get_string/set_string precisely to avoid that), so nothing outside
    // this module's own tests calls these.
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

    pub fn set_string(&self, key: &str, value: &str) {
        let obj: Retained<NSString> = NSString::from_str(value);
        let obj: &AnyObject = &obj;
        // SAFETY: the value is an `NSString`, exactly the type
        // `-setObject:forKey:`'s "must be of the correct type" contract
        // requires for a key read back with `-stringForKey:`.
        unsafe {
            self.defaults
                .setObject_forKey(Some(obj), &NSString::from_str(key))
        };
    }

    /// Removes the whole domain `self` was opened with, not just the keys
    /// this module knows about. Test-only: production code has no reason to
    /// wipe a user's settings wholesale.
    ///
    /// This clears the domain's *contents*, which is what the tests need
    /// for isolation from each other and from a previous run, but an empty
    /// `~/Library/Preferences/<suite>.plist` may still be left on disk
    /// afterward: `cfprefsd` re-flushes a domain it has touched sometime
    /// after the owning process exits, regardless of whether that domain is
    /// now empty, and `NSUserDefaults` has no API that removes the file
    /// itself, only its contents.
    ///
    /// A `std::fs::remove_file` at the suite's path was tried here and
    /// withdrawn: it appeared to work when checked immediately, but the
    /// file reliably came back within about fifteen seconds of the test
    /// process exiting, confirmed by direct measurement. Deleting a file a
    /// daemon is about to recreate is not cleanup, it is a race this
    /// process cannot win, and carrying that code brought a real hazard for
    /// no working benefit: `#[cfg(test)]` is the only thing stopping it
    /// from deleting `~/Library/Preferences/Turnstile.plist`, the
    /// developer's real settings, the day someone drops that bound to
    /// reuse this helper elsewhere. This is cosmetic: the leftover files
    /// are empty and a few hundred bytes each.
    #[cfg(test)]
    pub fn remove_all(&self) {
        self.defaults
            .removePersistentDomainForName(&NSString::from_str(&self.suite));
        self.defaults.synchronize();
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

    /// Falls back to the first game when the stored key is absent or names
    /// a game this build does not have. Storing `GameId::key()` rather than
    /// an index is what makes that fallback safe: an index would silently
    /// point at a different game the moment the list gains an entry or
    /// changes order, while an unrecognised key is simply rejected by
    /// `GameId::from_key`.
    pub fn selected_game(&self) -> GameId {
        self.get_string(KEY_SELECTED_GAME)
            .and_then(|s| GameId::from_key(&s))
            .unwrap_or(GameId::ALL[0])
    }

    pub fn save(&self, pref: Pref, game: GameId) {
        match pref {
            Pref::ShowDevelop(v) => self.set_bool(KEY_SHOW_DEVELOP, v),
            Pref::MultiVersion(v) => self.set_bool(KEY_MULTI_VERSION, v),
            Pref::SelectedGame(v) => self.set_string(KEY_SELECTED_GAME, v.key()),
            Pref::AutoUpdate(v) => self.set_bool(&auto_update_key(game.key()), v),
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
        // `auto_update_key` is a pure formatter: it does not require the
        // input to name a game this build knows about, so a hypothetical
        // future game cannot collide with an existing one.
        assert_eq!(auto_update_key("Something"), "autoInstallUpdates.Something");
    }

    #[test]
    fn the_multi_version_preference_maps_to_the_store_mode() {
        assert_eq!(mode_for(false), Mode::Compatible);
        assert_eq!(mode_for(true), Mode::MultiVersion);
    }

    #[test]
    fn a_round_trip_through_the_real_defaults_preserves_values() {
        // Uses a throwaway suite so the developer's own settings are untouched.
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

        // The other direction too, which is what `UiState`'s per-game array
        // mirrors: turning one game's setting on or off must leave the
        // other's exactly where it was.
        p.save(Pref::AutoUpdate(true), GameId::OpenLoco);
        assert!(p.auto_update(GameId::OpenRCT2));
        assert!(p.auto_update(GameId::OpenLoco));
        p.save(Pref::AutoUpdate(false), GameId::OpenRCT2);
        assert!(!p.auto_update(GameId::OpenRCT2));
        assert!(p.auto_update(GameId::OpenLoco));
        p.remove_all();
    }
}
