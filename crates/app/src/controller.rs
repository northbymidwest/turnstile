//! The seam where `Effect`s become real work. `Controller` owns the state
//! and the views; `handle` runs reduce, then renders, then performs; and
//! `perform` turns each `Effect` into either an immediate main-thread action
//! or a background thread that eventually posts a `Msg` back through
//! `mainqueue`.
//!
//! `Controller` is `Rc`-based and confined to the main thread by
//! construction: nothing here is `Send`, so a worker thread can only ever
//! compute a `Msg` (which is `Send`) and hand it to `mainqueue::post`. The
//! main-queue closure registered in `start` is what recovers the `Rc` from
//! `mainqueue`'s own `thread_local!` once that `Msg` is delivered back here.
//! Do not give `Controller` a `Sync` impl or a `static` slot to make it
//! reachable from a worker directly: see `mainqueue::HANDLER`'s doc comment
//! for why that shape was tried and reverted as unsound.
//!
//! Every effect that touches the version store is spawned through
//! `storelock::spawn` rather than `mainqueue::spawn`, which holds one
//! process-wide lock for the operation's whole duration. The two exceptions
//! are marked and justified at their arms in `perform`. That lock is what
//! keeps two workers out of the same `bin`; the reducer's `!is_busy()`
//! guards are what keep the *interface* from claiming two operations at
//! once, and neither substitutes for the other -- see `storelock`'s own doc
//! comment for how the two compose with the generation gate.

use std::cell::RefCell;
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
use turnstile_core::github::GitHub;
use turnstile_core::install;
use turnstile_core::release::Release;
use turnstile_core::store::{Mode, VersionStore};

use crate::mainqueue;
use crate::prefs::{Prefs, mode_for};
use crate::state::{Effect, Msg, ReleaseRow, UiState, reduce};
use crate::storelock;
use crate::views::{self, Views};

/// Where the update check looks. Named here rather than read from anywhere
/// at runtime: an application that takes its own update source from a file it
/// ships is an application whose update source can be changed by editing that
/// file.
const UPDATE_OWNER: &str = "northbymidwest";
const UPDATE_REPO: &str = "turnstile";

pub struct Controller {
    mtm: MainThreadMarker,
    state: RefCell<UiState>,
    views: Views,
    prefs: Prefs,
    /// The cancel flag for whichever install is currently in flight, if
    /// any. One flag per install, never shared or reused: `Effect::Install`
    /// mints a fresh one and cancels-and-replaces whatever was here before,
    /// and `Effect::CancelInFlight` takes this one out and sets it. Sharing
    /// a single flag across installs was tried and reverted -- see
    /// `perform`'s `Effect::Install` arm -- because resetting it to `false`
    /// for a new install could silently un-cancel an old one still running
    /// under the same flag.
    cancel: RefCell<Option<Arc<AtomicBool>>>,
}

/// The shortest gap between two *fraction* updates reaching the main queue:
/// twenty a second, comfortably more than the display can show and three
/// orders of magnitude below what `download.rs` produces.
///
/// The number it replaces was measured rather than guessed. `download.rs`
/// calls its progress callback once per read, and a real OpenRCT2 release
/// (117 MB) produced 7293 calls in 2.3 seconds -- 3175 a second, a median
/// of 132 microseconds apart. Each one ran the whole of `views::apply`,
/// which costs about 434 microseconds in a release build with the releases
/// popup full. Posting three thousand blocks a second onto a main queue
/// that can retire two thousand does not just lag: the queue never drains,
/// so the run loop never reaches the pass that would commit anything to the
/// screen, and every intermediate state is skipped. Twenty a second costs
/// under one percent of the main thread.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);

/// Decides which of an install's progress reports are worth a main-queue
/// hop. One per install, owned by the worker that produced the reports.
struct ProgressThrottle {
    /// When the last report was let through, and what status it carried.
    last: Option<(Instant, Status)>,
}

impl ProgressThrottle {
    fn new() -> ProgressThrottle {
        ProgressThrottle { last: None }
    }

    /// Whether `p` must reach the UI. **Only an intermediate fraction is
    /// ever dropped.** The first report, any change of status, a switch to
    /// indeterminate (`value: None`) and the report that completes a phase
    /// (`1.0`) all say what the app is *doing* rather than how far along it
    /// is, and swallowing one of those would leave the interface stating
    /// something untrue -- a worse failure than the cosmetic one this
    /// exists to fix. Downloading to Extracting is exactly such a
    /// transition, and it is the one the smoke test never saw.
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
    // Every game's auto-update value, not just the one being restored.
    // `Msg::SelectGame` clears the game-scoped state and reloads nothing
    // from preferences, and it must not: `reduce` is pure. Loading all of
    // them here is what makes a switch correct in the same turn it happens,
    // with no window in which `ReleasesLoaded` could act on another game's
    // setting. The other two above are deliberately global.
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

    // Worker threads cannot hold the Rc, so they post a Msg and the
    // main-queue closure recovers the controller from here. This must run
    // before anything else can post: a Msg posted before `register` runs is
    // silently dropped (see `mainqueue::post`), and `Actions`' selectors are
    // already live the moment `views::build` returns.
    let weak = Rc::downgrade(&controller);
    mainqueue::register(Rc::new(move |msg| {
        if let Some(controller) = weak.upgrade() {
            controller.handle(msg);
        }
    }));

    // Repair whatever is on disk before showing anything, so the first
    // render reflects reality rather than a half-finished mode switch.
    //
    // Crash debris from an interrupted install is swept first, and that
    // order is load-bearing rather than incidental: `recover_debris` puts a
    // build that was caught between install's two renames back at its
    // destination, and `reconcile` is what then decides where `bin` points.
    // The other way round, `link_to_newest` would choose among the versions
    // that happened to survive and leave the restored one inactive.
    //
    // Until this call existed, that build sat in `.trash-<name>` and the app
    // reported nothing installed until the user started another install --
    // which they had no reason to do, being told there was nothing there.
    //
    // Under the store lock, like every other store operation, and taken
    // once around both games rather than per game: these are store
    // mutations and nothing about them is special except when they run.
    //
    // This is the one acquisition made on the main thread, and it is the
    // one that cannot block. No worker exists yet: workers are spawned only
    // from `perform`, `perform` runs only from `handle`, and the first
    // `handle` call is below this. The repair itself already blocks the main
    // thread -- it is a synchronous walk of the store -- so an uncontended
    // lock adds nothing to that. Do not move this after the `handle` below,
    // which would make it contended and therefore able to block the UI.
    let mode = controller.mode();
    let mut repair_failures: Vec<String> = Vec::new();
    storelock::serialized(|| {
        for game in GameId::ALL {
            if let Some(store) = store_for(game) {
                install::recover_debris(&store, mode);
                // Reported rather than discarded. `reconcile_compatible` can
                // fail with "<path> exists from an interrupted operation; run
                // reconcile first", an error whose stated remedy is the thing
                // that just failed, and swallowing it left the app running on
                // an unrepaired store with the user told nothing.
                if let Err(e) = store.reconcile(mode) {
                    repair_failures.push(format!("{}: {e}", game.display_name()));
                }
            }
        }
    });

    controller.handle(Msg::SelectGame(selected));

    // After the first `SelectGame`, not before it: that message bumps the
    // generation, and an error raised ahead of it would be cleared as stale
    // by the very bump that starts the app up. Non-modal on purpose -- this
    // arrives while the window is still settling, and a modal at that moment
    // blocks the first render behind a dialog the user cannot yet place. The
    // banner is already the app's channel for an error about the store, and
    // `InstalledLoaded`'s success arm deliberately does not clear it, so it
    // survives the reload the switch above just started.
    if !repair_failures.is_empty() {
        controller.handle(Msg::StartupRepairFailed {
            message: repair_failures.join("\n"),
        });
    }

    // Last, and only if wanted. After the first `SelectGame` for the same
    // reason the repair banner is: that message bumps the generation. This
    // effect carries none, so it would survive the bump either way, but
    // issuing it after means the window is already populated before a request
    // to GitHub is in flight, and a slow or hanging network cannot delay the
    // first render.
    if controller.state.borrow().check_for_updates {
        controller.perform(Effect::CheckForUpdate);
    }

    // The controller owns `Views`, and nothing drops either for the life of
    // the process. Note `Views` declares its control fields after `actions`,
    // and controls hold `actions` as an *unretained* target, so if this ever
    // gains a real teardown path the drop order has to be revisited.
    std::mem::forget(controller.clone());
}

impl Controller {
    fn mode(&self) -> Mode {
        mode_for(self.state.borrow().multi_version)
    }

    /// Reduce, render, then perform. In this order, so the UI reflects the
    /// new state before any slow work starts.
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
            // `remove` or a mode change running alongside this would have
            // `installed()` and `active()` describing two different moments,
            // and the popup would then be repopulated from a store layout
            // that never existed.
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

            // Deliberately *not* under the store lock: this is an HTTPS
            // request and reads nothing on disk, so it has nothing to
            // corrupt or be corrupted by. Putting it there would be actively
            // harmful -- a slow or hanging GitHub request would hold every
            // install, switch and removal behind it for as long as it took.
            Effect::CheckForUpdate => {
                // Failure is silence. No network, a rate limit, a body that
                // will not parse: all reach `Ok(None)` and no banner appears.
                // The alternative is an error dialog on launch about a check
                // nobody asked for, which is a worse application.
                mainqueue::spawn(move || {
                    let found = GitHub::new()
                        .latest_turnstile(UPDATE_OWNER, UPDATE_REPO, env!("CARGO_PKG_VERSION"))
                        .unwrap_or(None);
                    Msg::UpdateChecked(found)
                });
            }

            Effect::ShowSettings => {
                views::settings::show(&self.views.settings, self.mtm);
            }

            Effect::OpenUrl(url) => {
                // Both safe in objc2-app-kit 0.3: checked against the
                // resolved crate rather than copied from an example written
                // against another version.
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
                // Starting an install cancels whatever was already running,
                // rather than merely not sharing a flag with it: the two
                // would otherwise race the same destination in compatible
                // mode, where every tag installs into `bin`. Taking the old
                // flag out and setting it -- not just overwriting the slot
                // -- is what makes that true. Replacing the slot without
                // setting the old flag would leave that older worker
                // running and no longer reachable by any later
                // `CancelInFlight`, trading "un-cancelled" for
                // "uncancellable": the same race, reached a different way.
                let cancel = Arc::new(AtomicBool::new(false));
                if let Some(previous) = self.cancel.replace(Some(cancel.clone())) {
                    previous.store(true, Ordering::Relaxed);
                }
                let include_develop = self.state.borrow().show_develop;
                // The lock is held for the whole install, download
                // included, rather than only for the two renames at the
                // end. Splitting it would mean the staging directory this
                // writes into is unprotected for the slow part, and the
                // renames are then a second acquisition with a gap in front
                // of them -- exactly the shape of interleaving this exists
                // to remove. The cost is that a store read issued during a
                // download queues behind it; every route to one first bumps
                // the generation, which emits `Effect::CancelInFlight`, and
                // `download.rs` checks that flag once per read, so the
                // install it is queued behind is already stopping.
                storelock::spawn(move || {
                    let mut throttle = ProgressThrottle::new();
                    let result = (|| {
                        let store = store_for(game).ok_or_else(|| err("no home directory"))?;
                        let releases = GitHub::new()
                            .releases_for(game, include_develop, HostArch::current())
                            .map_err(|e| e.to_string())?;
                        // Every listed release already carries a download this
                        // host can run, so there is no asset selection here and
                        // no "nothing for your platform" failure to handle.
                        let release = releases
                            .iter()
                            .find(|r| r.tag == tag)
                            .ok_or_else(|| err("that build is no longer listed"))?;

                        install::install(
                            &store,
                            mode,
                            &tag,
                            &release.download,
                            // Throttled here, at the point the message is
                            // posted, rather than by making `apply`
                            // cleverer: `apply` rebuilding both pop-ups on
                            // every render is wasteful, but making it
                            // incremental means diffing what is displayed
                            // against what is in state, and that is a real
                            // correctness risk traded for a cosmetic win.
                            // Not throttled in `download.rs` either, so
                            // core keeps reporting everything it knows and
                            // the UI decides what it can use.
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
                            // directory it actually used need not equal
                            // `tag` verbatim (sanitization, or a collision
                            // walk if a different tag sanitizes the same
                            // way). `activate` takes `Installed::name`, the
                            // on-disk identity, never a display `tag` --
                            // conflating the two is the bug the whole
                            // codebase guards against, so the name is looked
                            // up freshly here rather than assumed. Reloading
                            // lists newest-modified first, and the entry
                            // this install just wrote is the newest thing
                            // with this tag, so the first match is correct
                            // even when an older entry shares the same tag.
                            //
                            // A missing match here would mean the entry
                            // `install` just wrote is not found by its own
                            // tag, which install.rs's contract makes
                            // practically unreachable -- but a silent no-op
                            // is exactly the shape obligation #1 warns
                            // against, so this errors rather than skipping
                            // the activation quietly.
                            let installed = store.installed(mode).map_err(|e| e.to_string())?;
                            let name = installed
                                .iter()
                                .find(|i| i.tag == tag)
                                .map(|i| i.name.clone())
                                .ok_or_else(|| {
                                    // Not "vanished": the same lookup also
                                    // misses when the build is on disk and
                                    // correct but its `.version` file
                                    // cannot be read, since `describe`
                                    // substitutes `UNKNOWN_TAG` and no
                                    // entry then matches the tag asked for.
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

            // The only effect that performs two store operations, and the
            // only place their ordering is decided. Both run on the one
            // worker, in this order, with the delete reached only from the
            // switch's success arm: `VersionStore::remove` refuses to
            // delete the active version, and that refusal is what keeps
            // `bin` from being left pointing at something that no longer
            // exists. Nothing here weakens it -- the sequencing sits above
            // the store precisely so the store's own guard stays absolute.
            //
            // The two failures are reported as different messages on
            // purpose. A failed switch is an `Activated` error, because
            // that is what actually failed and nothing was deleted; a
            // failed delete is a `Removed` error, which also reloads,
            // because by then the switch has already changed `bin`.
            Effect::SwitchThenRemove {
                generation,
                game,
                activate,
                remove,
            } => {
                // One acquisition around both phases, which is the whole
                // point of `storelock::spawn` taking the closure rather than
                // exposing a guard: the switch and the delete are a sequence
                // whose ordering is the safety property, and a lock released
                // between them would leave the gap open.
                storelock::spawn(move || match store_for(game) {
                    None => Msg::Activated {
                        generation,
                        result: Err(err("no home directory")),
                    },
                    Some(store) => switch_then_remove(&store, mode, &activate, &remove, generation),
                });
            }

            // Two of these are issued from one `Msg::ToggleMultiVersion`,
            // one per game, and they queue behind each other on the single
            // lock. That is why there is one lock rather than one per game:
            // a per-game lock would not serialize either of these against a
            // per-game operation already running.
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

            // Deliberately *not* under the store lock. It mutates nothing:
            // it execs a binary out of a build directory and watches it for
            // half a second. The worst a concurrent removal can do to it is
            // make the exec fail, which is reported as a launch error, and
            // a build deleted from under an already-running game keeps
            // running from the unlinked inode -- the user's saved games live
            // in the game directory, not the build. Against that, taking the
            // lock would hold every real store operation behind the
            // half-second liveness probe below, and hold Play behind a
            // multi-second removal it has nothing to do with.
            Effect::Launch { game, dir } => {
                mainqueue::spawn(move || {
                    let result = install::launch(game, &dir).map_err(|e| e.to_string());
                    Msg::LaunchFinished { result }
                });
            }

            // The one effect with no worker: it only flips the flag
            // whichever install is currently in flight is already
            // watching, if any. Emitted on every generation bump, so a
            // channel toggle mid-install cannot leave a second install of
            // the same game running alongside the first. `install.rs` is
            // hardened against crash debris, not against a concurrent
            // second install of the same tag.
            //
            // Takes the flag out of the slot rather than leaving it there:
            // once set, a flag is never reused by a later install (see
            // `Effect::Install`'s fresh `Arc::new` above), so there is
            // nothing left for this slot to hold once it has been told to
            // stop.
            //
            // Never fold this into a wildcard arm: silently doing nothing
            // here looks identical to working, and the race it prevents is
            // not one the tests can reach.
            Effect::CancelInFlight { generation: _ } => {
                if let Some(flag) = self.cancel.borrow_mut().take() {
                    flag.store(true, Ordering::Relaxed);
                }
            }

            // Modals run on the main thread and feed their answer back in.
            //
            // `name` is the on-disk identity, correct for the eventual
            // `Effect::SwitchThenRemove` but not what a user should be
            // asked to confirm: two entries can share a display `tag` while
            // having different `name`s, so showing `name` verbatim could
            // put a confusing collision-suffixed directory name in front of
            // the user instead of the build version they actually picked.
            //
            // The tag is looked up from the entry the *effect* names, never
            // from the current selection, and that is load-bearing rather
            // than stylistic. `NSAlert::runModal` below pumps a nested run
            // loop, so an `InstalledLoaded` already in flight is delivered
            // while the dialog is on screen and can reorder
            // `state.installed` and move `selected_installed` underneath
            // it. Re-reading the selection here would let that reshuffle
            // put one build's name in the dialog while a different one is
            // deleted. The lookup below runs before the modal opens, and
            // the names the deletion uses were captured at click time (see
            // `PendingRemoval` in `state.rs`), so both sides come from the
            // same decision. Do not simplify either back into a read of
            // `selected_installed`.
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
/// happened. The whole of `Effect::SwitchThenRemove`'s ordering lives here,
/// in one function on one thread, so there is a single place to read it and
/// nowhere for another message to get between the two halves.
///
/// The two failures deliberately report as different messages. A failed
/// switch comes back as `Activated`, because the switch is what failed and
/// nothing was deleted; a failed delete comes back as `Removed`, which also
/// reloads, because by then `bin` has already moved.
///
/// Nothing here relaxes `StoreError::RemoveActive`. That refusal is the
/// last thing standing between a mistake in this sequence and a `bin`
/// symlink pointing at a directory that has been deleted, so the sequencing
/// sits above the store rather than inside it: if the switch below silently
/// did nothing, `remove` would still refuse, and the user would get an
/// error instead of a broken installation.
fn switch_then_remove(
    store: &VersionStore,
    mode: Mode,
    activate: &str,
    remove: &str,
    generation: u64,
) -> Msg {
    // `activate` writes its symlink without checking that the target
    // exists, so a name that stopped being installed between the click and
    // here would leave `bin` dangling -- the one outcome this whole
    // sequence exists to prevent. This is not a re-reading of the
    // selection: the name acted on is still the caller's own, this only
    // declines to act when that name no longer refers to anything.
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

/// Formats releases for the popup. No filtering happens here: a release that
/// reaches this point already has a download this machine can run, because
/// that was decided when the response was parsed.
fn to_rows(releases: Vec<Release>) -> Vec<ReleaseRow> {
    // `age::now()`, not `time::OffsetDateTime::now_utc` directly: this
    // crate's dependency list does not include `time` (see Global
    // Constraints), so `now` is core's one exported door into it.
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
    /// active, which is the shape every removal starts from.
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

    /// The property the whole effect exists for: if the switch does not
    /// happen, nothing is deleted. A name that is not installed stands in
    /// for the switch failing, because `activate` would otherwise happily
    /// write a symlink to it and leave `bin` dangling.
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

    /// `StoreError::RemoveActive` is untouched by any of this: asking to
    /// delete the build that was just switched *to* still fails, and fails
    /// as a removal rather than being quietly skipped.
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

    /// The one thing a throttle must never do. Every report that says what
    /// the app is *doing*, rather than how far along it is, goes through
    /// regardless of how recently the last one did.
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
