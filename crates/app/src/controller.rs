//! The seam where `Effect`s become real work. `Controller` owns the state and
//! the views; `handle` runs reduce, then renders, then performs; and `perform`
//! turns each `Effect` into either an immediate main-thread action or a
//! background thread that eventually posts a `Msg` back through `mainqueue`.
//!
//! `Controller` is `Rc`-based and confined to the main thread by construction:
//! nothing here is `Send`, so a worker can only compute a `Msg` and hand it to
//! `mainqueue::post`. Do not give `Controller` a `Sync` impl or a `static`
//! slot to make it reachable from a worker directly -- see `mainqueue::HANDLER`
//! for why that shape was tried and reverted as unsound.
//!
//! Every effect that touches the version store is spawned through
//! `storelock::spawn`, which holds one process-wide lock for the operation's
//! whole duration; the two exceptions are marked at their arms in `perform`.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use objc2::MainThreadMarker;
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSString, NSURL};
use turnstile_core::age::{self, Age};
use turnstile_core::asset::HostArch;
use turnstile_core::dirs::Dirs;
use turnstile_core::download::{Progress, Status};
use turnstile_core::game::GameId;
use turnstile_core::gameconfig;
use turnstile_core::gamedata::{self, OriginalGame};
use turnstile_core::github::GitHub;
use turnstile_core::install;
use turnstile_core::release::Release;
use turnstile_core::store::{Mode, VersionStore};

use crate::mainqueue;
use crate::prefs::{Prefs, mode_for};
use crate::state::{Effect, Msg, ReleaseRow, UiState, reduce};
use crate::storelock;
use crate::views::{self, Views};

/// Where the update check looks. Named here rather than read at runtime: an
/// application that takes its update source from a file it ships is one whose
/// update source can be changed by editing that file.
const UPDATE_OWNER: &str = "northbymidwest";
const UPDATE_REPO: &str = "turnstile";

pub struct Controller {
    mtm: MainThreadMarker,
    state: RefCell<UiState>,
    views: Views,
    prefs: Prefs,
    /// The cancel flag for whichever install is currently in flight. One flag
    /// per install, never shared or reused: sharing one across installs was
    /// tried and reverted, because resetting it to `false` for a new install
    /// could silently un-cancel an old one still running under it.
    cancel: RefCell<Option<Arc<AtomicBool>>>,
}

/// The shortest gap between two *fraction* updates reaching the main queue.
///
/// Measured rather than guessed: a 117 MB release produced 7293 progress calls
/// in 2.3 seconds, and each one ran the whole of `views::apply` at ~434
/// microseconds. Posting three thousand blocks a second onto a main queue that
/// can retire two thousand means the queue never drains, so the run loop never
/// reaches the pass that would commit anything to the screen.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);

/// Decides which of an install's progress reports are worth a main-queue hop.
struct ProgressThrottle {
    last: Option<(Instant, Status)>,
}

impl ProgressThrottle {
    fn new() -> ProgressThrottle {
        ProgressThrottle { last: None }
    }

    /// Whether `p` must reach the UI. **Only an intermediate fraction is ever
    /// dropped.** The first report, any change of status, a switch to
    /// indeterminate and the report that completes a phase all say what the
    /// app is *doing* rather than how far along it is, and swallowing one
    /// would leave the interface stating something untrue.
    fn should_post(&mut self, p: Progress, now: Instant) -> bool {
        let is_intermediate_fraction = matches!(p.value, Some(fraction) if fraction < 1.0);
        let same_phase = self.last.is_some_and(|(_, status)| status == p.status);
        if is_intermediate_fraction
            && same_phase
            && self
                .last
                .is_some_and(|(at, _)| now.duration_since(at) < PROGRESS_INTERVAL)
        {
            return false;
        }
        self.last = Some((now, p.status));
        true
    }
}

/// Where a game's data is, if the game's own configuration says so and
/// something is actually there. The configuration is the authority rather than
/// the preference: somebody may have pointed the game somewhere by hand.
fn installed_game_data(game: OriginalGame) -> Option<PathBuf> {
    let dirs = Dirs::system()?;
    let configured = gameconfig::configured_path(game, &dirs).ok().flatten()?;
    gamedata::installed_at(game, &configured).then_some(configured)
}

/// `~/Library/Application Support/Turnstile/games`, falling back to the
/// current directory in the impossible case of no home so this stays total.
fn default_install_root() -> PathBuf {
    Dirs::system().map_or_else(
        || PathBuf::from("."),
        |dirs| dirs.app_support.join("Turnstile/games"),
    )
}

fn store_for(game: GameId) -> Option<VersionStore> {
    Some(VersionStore::new(Dirs::system()?, game))
}

fn err(message: &str) -> String {
    message.to_string()
}

pub fn start(mtm: MainThreadMarker) {
    let prefs = Prefs::new();
    let views = views::build(mtm);

    let selected = prefs.selected_game();
    let mut state = UiState::new(selected);
    state.show_develop = prefs.show_develop();
    state.check_for_updates = prefs.check_for_updates();
    state.multi_version = prefs.multi_version();
    // Every game's value, not just the selected one: `Msg::SelectGame` reloads
    // nothing from preferences and must not, since `reduce` is pure. Loading
    // all of them here leaves no window in which `ReleasesLoaded` could act on
    // another game's setting.
    for game in GameId::ALL {
        state.set_auto_update(game, prefs.auto_update(game));
    }

    let controller = Rc::new(Controller {
        mtm,
        state: RefCell::new(state),
        views,
        prefs,
        cancel: RefCell::new(None),
    });

    // Worker threads cannot hold the Rc, so they post a Msg and the main-queue
    // closure recovers the controller from here. This must run before anything
    // can post: a Msg posted before `register` is silently dropped, and
    // `Actions`' selectors are live the moment `views::build` returns.
    let weak = Rc::downgrade(&controller);
    mainqueue::register(Rc::new(move |msg| {
        if let Some(controller) = weak.upgrade() {
            controller.handle(msg);
        }
    }));

    // Repair whatever is on disk before showing anything, so the first render
    // reflects reality rather than a half-finished mode switch. Debris is swept
    // first and that order is load-bearing: `recover_debris` puts a build
    // caught between install's two renames back at its destination, and
    // `reconcile` then decides where `bin` points. The other way round,
    // `link_to_newest` would leave the restored build inactive.
    //
    // This is the one lock acquisition made on the main thread, and it cannot
    // block: no worker exists yet, since the first `handle` call is below.
    // Do not move it after that `handle`, which would make it contended.
    let mode = controller.mode();
    let mut repair_failures: Vec<String> = Vec::new();
    storelock::serialized(|| {
        for game in GameId::ALL {
            if let Some(store) = store_for(game) {
                install::recover_debris(&store, mode);
                // Reported rather than discarded: `reconcile_compatible` can
                // fail with an error whose stated remedy is the thing that
                // just failed, and swallowing it left the app running on an
                // unrepaired store with the user told nothing.
                if let Err(e) = store.reconcile(mode) {
                    repair_failures.push(format!("{}: {e}", game.display_name()));
                }
            }
        }
    });

    controller.handle(Msg::SelectGame(selected));

    // Straight into state rather than through a message: this is the stored
    // value being loaded, not somebody changing it, and `InstallRootChosen`
    // would save it back and rescan for no reason.
    controller.state.borrow_mut().install_root = controller.prefs.install_root();

    // After the first `SelectGame`, not before: that message bumps the
    // generation, and an error raised ahead of it would be cleared as stale by
    // the very bump that starts the app up. Non-modal on purpose, since a
    // dialog at this moment blocks the first render.
    if !repair_failures.is_empty() {
        controller.handle(Msg::StartupRepairFailed {
            message: repair_failures.join("\n"),
        });
    }

    // The detail pane shows this for every game rather than only the selected
    // one, so it is looked for once at startup rather than on each switch.
    controller.perform(Effect::ScanGameData);

    // Last, so the window is already populated before a request to GitHub is
    // in flight and a slow network cannot delay the first render.
    if controller.state.borrow().check_for_updates {
        controller.perform(Effect::CheckForUpdate);
    }

    // The controller owns `Views`, and nothing drops either for the life of the
    // process. `Views` declares its control fields after `actions`, and
    // controls hold `actions` unretained, so a real teardown path would have to
    // revisit the drop order.
    std::mem::forget(controller.clone());
}

impl Controller {
    fn mode(&self) -> Mode {
        mode_for(self.state.borrow().multi_version)
    }

    /// Where game data is unpacked: whatever Settings says, or the default
    /// beneath Turnstile's own directory. Read from the preference rather than
    /// `UiState`, which is not borrowed where effects are performed; the same
    /// message writes both, so they cannot disagree.
    fn install_root(&self) -> PathBuf {
        self.prefs
            .install_root()
            .unwrap_or_else(default_install_root)
    }

    /// Reduce, render, then perform, so the UI reflects the new state before
    /// any slow work starts.
    pub fn handle(self: &Rc<Self>, msg: Msg) {
        let effects = {
            let mut state = self.state.borrow_mut();
            reduce(&mut state, msg)
        };
        views::apply(&self.state.borrow(), &self.views);
        for effect in effects {
            self.perform(effect);
        }
    }

    pub(crate) fn perform(self: &Rc<Self>, effect: Effect) {
        let mode = self.mode();
        match effect {
            Effect::SavePref(pref) => {
                let game = self.state.borrow().selected_game;
                self.prefs.save(pref, game);
            }

            // Serialized with every mutation, not just with other reads: a
            // removal running alongside would have `installed()` and `active()`
            // describing two different moments, and the popup would then show a
            // store layout that never existed.
            Effect::LoadInstalled { generation, game } => {
                storelock::spawn(move || {
                    let result = (|| {
                        let store = store_for(game).ok_or_else(|| err("no home directory"))?;
                        let installed = store.installed(mode).map_err(|e| e.to_string())?;
                        let active = store.active(mode).map_err(|e| e.to_string())?;
                        Ok((installed, active))
                    })();
                    Msg::InstalledLoaded { generation, result }
                });
            }

            // Not under the store lock: an HTTPS request reads nothing on disk,
            // and a hanging GitHub request would otherwise hold every install,
            // switch and removal behind it.
            Effect::CheckForUpdate => {
                // Failure is silence: no banner is a better application than an
                // error dialog on launch about a check nobody asked for.
                mainqueue::spawn(move || {
                    let found = GitHub::new()
                        .latest_turnstile(UPDATE_OWNER, UPDATE_REPO, env!("CARGO_PKG_VERSION"))
                        .unwrap_or(None);
                    Msg::UpdateChecked(found)
                });
            }

            // Reads three directories and nothing else, so it is off the lock.
            Effect::ScanGameData => {
                mainqueue::spawn(move || {
                    let found = OriginalGame::ALL
                        .into_iter()
                        .map(|game| (game, installed_game_data(game)))
                        .collect();
                    Msg::GameDataScanned(found)
                });
            }

            Effect::PickInstaller(game) => {
                let chosen = views::picker::choose_installer(self.mtm);
                self.handle(Msg::InstallerChosen {
                    game,
                    installer: chosen,
                });
            }

            Effect::PickInstallRoot => {
                let chosen = views::picker::choose_directory(self.mtm);
                if let Some(root) = chosen {
                    self.handle(Msg::InstallRootChosen(Some(root)));
                }
            }

            Effect::InstallGameData {
                generation,
                game,
                installer,
            } => {
                let root = self.install_root();
                storelock::spawn(move || {
                    let result = (|| {
                        let dirs = Dirs::system().ok_or_else(|| err("no home directory"))?;
                        let destination = root.join(game.directory());

                        gamedata::install_game_data(
                            &installer,
                            game,
                            &destination,
                            &mut |progress| {
                                mainqueue::post(Msg::Progress {
                                    generation,
                                    status: progress.status,
                                    value: progress.value,
                                });
                            },
                            &AtomicBool::new(false),
                        )
                        .map_err(|e| e.to_string())?;

                        // Extracting without telling the game leaves somebody
                        // with the data installed and the game still saying it
                        // cannot find it.
                        gameconfig::point_at(game, &dirs, &destination)
                            .map_err(|e| e.to_string())?;

                        Ok(destination)
                    })();

                    Msg::GameDataInstalled {
                        generation,
                        game,
                        result,
                    }
                });
            }

            Effect::ShowSettings => {
                views::settings::show(&self.views.settings, self.mtm);
            }

            Effect::OpenUrl(url) => {
                let workspace = NSWorkspace::sharedWorkspace();
                if let Some(url) = NSURL::URLWithString(&NSString::from_str(&url)) {
                    workspace.openURL(&url);
                }
            }

            Effect::FetchReleases {
                generation,
                game,
                include_develop,
            } => {
                mainqueue::spawn(move || {
                    let result = GitHub::new()
                        .releases_for(game, include_develop, HostArch::current())
                        .map_err(|e| e.to_string())
                        .map(to_rows);
                    Msg::ReleasesLoaded { generation, result }
                });
            }

            Effect::Install {
                generation,
                game,
                tag,
            } => {
                // Starting an install cancels whatever was already running: in
                // compatible mode the two would otherwise race the same `bin`.
                // Taking the old flag out and setting it, rather than just
                // overwriting the slot, is what makes that true -- otherwise
                // the older worker keeps running and is no longer reachable by
                // any later `CancelInFlight`.
                let cancel = Arc::new(AtomicBool::new(false));
                if let Some(previous) = self.cancel.replace(Some(cancel.clone())) {
                    previous.store(true, Ordering::Relaxed);
                }
                let include_develop = self.state.borrow().show_develop;
                // The lock is held for the whole install, download included,
                // rather than only for the two renames at the end: splitting it
                // would leave the staging directory unprotected for the slow
                // part and put a gap in front of the renames. A store read
                // issued during a download queues behind it, but every route to
                // one first emits `Effect::CancelInFlight`, so the install it
                // is queued behind is already stopping.
                storelock::spawn(move || {
                    let mut throttle = ProgressThrottle::new();
                    let result = (|| {
                        let store = store_for(game).ok_or_else(|| err("no home directory"))?;
                        let releases = GitHub::new()
                            .releases_for(game, include_develop, HostArch::current())
                            .map_err(|e| e.to_string())?;
                        // Every listed release already carries a download this
                        // host can run, so there is no asset selection here.
                        let release = releases
                            .iter()
                            .find(|r| r.tag == tag)
                            .ok_or_else(|| err("that build is no longer listed"))?;

                        install::install(
                            &store,
                            mode,
                            &tag,
                            &release.download,
                            // Throttled here rather than by making `apply`
                            // incremental, which would mean diffing what is
                            // displayed against what is in state: a real
                            // correctness risk for a cosmetic win. Not
                            // throttled in `download.rs` either, so core keeps
                            // reporting everything it knows.
                            &mut |p| {
                                if throttle.should_post(p, Instant::now()) {
                                    mainqueue::post(Msg::Progress {
                                        generation,
                                        status: p.status,
                                        value: p.value,
                                    });
                                }
                            },
                            &cancel,
                        )
                        .map_err(|e| e.to_string())?;

                        if mode == Mode::MultiVersion {
                            // `install` reports no name of its own, and the
                            // directory it used need not equal `tag` verbatim
                            // (sanitization, or a collision walk). `activate`
                            // takes `Installed::name`, never a display tag, so
                            // the name is looked up freshly. Reloading lists
                            // newest-modified first, so the first match is
                            // correct even when an older entry shares the tag.
                            let installed = store.installed(mode).map_err(|e| e.to_string())?;
                            let name = installed
                                .iter()
                                .find(|i| i.tag == tag)
                                .map(|i| i.name.clone())
                                .ok_or_else(|| {
                                    // Not "vanished": the same lookup also
                                    // misses when the build is correct but its
                                    // `.version` file cannot be read, since
                                    // `describe` substitutes `UNKNOWN_TAG`.
                                    err("the build was written but could not be identified afterwards; its version file may be unreadable")
                                })?;
                            store.activate(mode, &name).map_err(|e| e.to_string())?;
                        }
                        Ok(())
                    })();
                    Msg::InstallFinished { generation, result }
                });
            }

            Effect::Activate {
                generation,
                game,
                name,
            } => {
                storelock::spawn(move || {
                    let result = store_for(game)
                        .ok_or_else(|| err("no home directory"))
                        .and_then(|s| s.activate(mode, &name).map_err(|e| e.to_string()));
                    Msg::Activated { generation, result }
                });
            }

            // The only effect that performs two store operations, and the only
            // place their ordering is decided. Both run on one worker, with the
            // delete reached only from the switch's success arm:
            // `VersionStore::remove` refuses to delete the active version, and
            // that refusal is what keeps `bin` from pointing at something that
            // no longer exists.
            Effect::SwitchThenRemove {
                generation,
                game,
                activate,
                remove,
            } => {
                // One acquisition around both phases, which is why
                // `storelock::spawn` takes the closure rather than exposing a
                // guard: a lock released between them would leave the gap open.
                storelock::spawn(move || match store_for(game) {
                    None => Msg::Activated {
                        generation,
                        result: Err(err("no home directory")),
                    },
                    Some(store) => switch_then_remove(&store, mode, &activate, &remove, generation),
                });
            }

            // Two of these are issued from one `Msg::ToggleMultiVersion` and
            // queue behind each other on the single lock. That is why there is
            // one lock rather than one per game: a per-game lock would not
            // serialize these against a per-game operation already running.
            Effect::SetMode {
                generation,
                game,
                multi_version,
            } => {
                storelock::spawn(move || {
                    let result = (|| {
                        let store = store_for(game).ok_or_else(|| err("no home directory"))?;
                        if multi_version {
                            store.enable_multi_version().map_err(|e| e.to_string())?;
                            Ok(None)
                        } else {
                            let report =
                                store.disable_multi_version().map_err(|e| e.to_string())?;
                            Ok(Some(report))
                        }
                    })();
                    Msg::ModeChanged { generation, result }
                });
            }

            // Not under the store lock: this mutates nothing, and the worst a
            // concurrent removal can do is make the exec fail, which is
            // reported as a launch error. Taking the lock would hold every
            // store operation behind the half-second liveness probe.
            Effect::Launch { game, dir } => {
                mainqueue::spawn(move || {
                    let result = install::launch(game, &dir).map_err(|e| e.to_string());
                    Msg::LaunchFinished { result }
                });
            }

            // The one effect with no worker: it flips the flag whichever install
            // is in flight is already watching. Takes the flag out of the slot,
            // since a set flag is never reused by a later install.
            //
            // Never fold this into a wildcard arm: silently doing nothing here
            // looks identical to working, and the race it prevents is not one
            // the tests can reach.
            Effect::CancelInFlight { generation: _ } => {
                if let Some(flag) = self.cancel.borrow_mut().take() {
                    flag.store(true, Ordering::Relaxed);
                }
            }

            // `name` is the on-disk identity, correct for the eventual
            // `Effect::SwitchThenRemove` but not what a user should be asked to
            // confirm: two entries can share a display tag, so showing `name`
            // could put a collision-suffixed directory name in front of them.
            //
            // The tag is looked up from the entry the *effect* names, never
            // from the current selection. `runModal` pumps a nested run loop,
            // so an `InstalledLoaded` in flight can reorder `state.installed`
            // while the dialog is open; re-reading the selection here would let
            // that put one build's name in the dialog while another is deleted.
            Effect::ConfirmRemove { name } => {
                let tag = {
                    let state = self.state.borrow();
                    state
                        .installed
                        .iter()
                        .find(|i| i.name == name)
                        .map(|i| i.tag.clone())
                        .unwrap_or_else(|| name.clone())
                };
                if views::alert::confirm_remove(self.mtm, &tag) {
                    self.handle(Msg::ConfirmRemove);
                }
            }

            Effect::ShowDisableReport(report) => {
                views::alert::show_disable_report(self.mtm, &report);
            }
        }
    }
}

/// Switches to `activate`, and deletes `remove` only if that switch really
/// happened. The whole of `Effect::SwitchThenRemove`'s ordering lives here, on
/// one thread, so there is nowhere for another message to get between the two
/// halves.
///
/// The failures report as different messages: a failed switch as `Activated`,
/// because nothing was deleted; a failed delete as `Removed`, which also
/// reloads, because by then `bin` has already moved.
///
/// Nothing here relaxes `StoreError::RemoveActive`: if the switch silently did
/// nothing, `remove` would still refuse, and the user would get an error
/// instead of a broken installation.
fn switch_then_remove(
    store: &VersionStore,
    mode: Mode,
    activate: &str,
    remove: &str,
    generation: u64,
) -> Msg {
    // `activate` writes its symlink without checking that the target exists, so
    // a name that stopped being installed between the click and here would
    // leave `bin` dangling. Not a re-reading of the selection: the name acted
    // on is still the caller's, this only declines when it refers to nothing.
    let switched = store
        .installed(mode)
        .map_err(|e| e.to_string())
        .and_then(|installed| {
            if installed.iter().any(|i| i.name == activate) {
                Ok(())
            } else {
                Err(err("the version to switch to is no longer installed"))
            }
        })
        .and_then(|()| store.activate(mode, activate).map_err(|e| e.to_string()));

    match switched {
        Err(message) => Msg::Activated {
            generation,
            result: Err(message),
        },
        Ok(()) => Msg::Removed {
            generation,
            result: store.remove(mode, remove).map_err(|e| e.to_string()),
        },
    }
}

/// Formats releases for the popup. Nothing is filtered: a release reaching
/// here already has a download this machine can run.
fn to_rows(releases: Vec<Release>) -> Vec<ReleaseRow> {
    // `age::now()`, not `time::OffsetDateTime::now_utc`: this crate does not
    // depend on `time`, so `now` is core's one exported door into it.
    let now = age::now();
    releases
        .into_iter()
        .map(|r| ReleaseRow {
            tag: r.tag,
            age: r.published.map(|at| Age::since(at, now)),
            channel: r.channel,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use turnstile_core::download::{Progress, Status};

    fn scratch(label: &str) -> Dirs {
        let root = std::env::temp_dir().join(format!(
            "turnstile-controller-{}-{label}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        Dirs::at(root)
    }

    /// Lays out `versions/<name>` for each name and makes the first one
    fn store_with(label: &str, names: &[&str]) -> VersionStore {
        let store = VersionStore::new(scratch(label), GameId::OpenRCT2);
        for name in names {
            let dir = store.versions_path().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(".version"), name).unwrap();
        }
        store.activate(Mode::MultiVersion, names[0]).unwrap();
        store
    }

    #[test]
    fn the_switch_happens_before_the_delete_and_both_land() {
        let store = store_with("happy", &["v1", "v2"]);
        let msg = switch_then_remove(&store, Mode::MultiVersion, "v2", "v1", 7);
        assert!(
            matches!(
                msg,
                Msg::Removed {
                    generation: 7,
                    result: Ok(())
                }
            ),
            "got {msg:?}"
        );
        assert!(
            !store.versions_path().join("v1").exists(),
            "the named build is gone"
        );
        assert!(
            store.versions_path().join("v2").is_dir(),
            "the switch target survives"
        );
        assert_eq!(
            store.active(Mode::MultiVersion).unwrap().as_deref(),
            Some("v2")
        );
    }

    #[test]
    fn a_failed_switch_deletes_nothing() {
        let store = store_with("failed-switch", &["v1", "v2"]);
        let msg = switch_then_remove(&store, Mode::MultiVersion, "ghost", "v1", 3);
        assert!(
            matches!(
                msg,
                Msg::Activated {
                    generation: 3,
                    result: Err(_)
                }
            ),
            "got {msg:?}"
        );
        assert!(
            store.versions_path().join("v1").is_dir(),
            "nothing may be deleted"
        );
        assert!(store.versions_path().join("v2").is_dir());
        assert_eq!(
            store.active(Mode::MultiVersion).unwrap().as_deref(),
            Some("v1"),
            "a failed switch must leave bin exactly where it was"
        );
    }

    #[test]
    fn removing_the_build_just_switched_to_is_still_refused() {
        let store = store_with("remove-active", &["v1", "v2"]);
        let msg = switch_then_remove(&store, Mode::MultiVersion, "v2", "v2", 1);
        assert!(
            matches!(
                msg,
                Msg::Removed {
                    generation: 1,
                    result: Err(_)
                }
            ),
            "got {msg:?}"
        );
        assert!(
            store.versions_path().join("v2").is_dir(),
            "the active build survives"
        );
    }

    fn at(base: Instant, millis: u64) -> Instant {
        base + Duration::from_millis(millis)
    }

    fn downloading(fraction: f64) -> Progress {
        Progress {
            status: Status::Downloading,
            value: Some(fraction),
        }
    }

    #[test]
    fn intermediate_fractions_are_limited_to_the_interval() {
        let base = Instant::now();
        let mut throttle = ProgressThrottle::new();
        assert!(
            throttle.should_post(downloading(0.0), base),
            "the first report always posts"
        );
        assert!(!throttle.should_post(downloading(0.01), at(base, 1)));
        assert!(!throttle.should_post(downloading(0.02), at(base, 49)));
        assert!(throttle.should_post(downloading(0.03), at(base, 50)));
        assert!(!throttle.should_post(downloading(0.04), at(base, 60)));
        assert!(throttle.should_post(downloading(0.05), at(base, 100)));
    }

    #[test]
    fn status_transitions_are_never_throttled() {
        let base = Instant::now();
        let mut throttle = ProgressThrottle::new();
        assert!(throttle.should_post(downloading(0.5), base));
        assert!(
            !throttle.should_post(downloading(0.51), at(base, 1)),
            "a fraction is dropped"
        );
        assert!(
            throttle.should_post(
                Progress {
                    status: Status::Extracting,
                    value: None
                },
                at(base, 2)
            ),
            "Downloading to Extracting must reach the UI"
        );
        assert!(
            throttle.should_post(
                Progress {
                    status: Status::Extracting,
                    value: Some(1.0)
                },
                at(base, 3)
            ),
            "the report that completes a phase must reach the UI"
        );
    }

    #[test]
    fn a_completed_phase_and_an_indeterminate_report_are_never_throttled() {
        let base = Instant::now();
        let mut throttle = ProgressThrottle::new();
        assert!(throttle.should_post(downloading(0.5), base));
        assert!(
            throttle.should_post(
                Progress {
                    status: Status::Downloading,
                    value: None
                },
                at(base, 1)
            ),
            "switching the bar to indeterminate is not a fraction update"
        );
        assert!(
            throttle.should_post(downloading(1.0), at(base, 2)),
            "1.0 ends the phase"
        );
    }
}
