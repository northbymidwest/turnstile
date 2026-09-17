//! Everything interesting about the interface's behavior, with no AppKit
//! anywhere near it. `UiState` holds what is on screen; `reduce` is the only
//! thing allowed to change it, and it never touches the filesystem or the
//! network itself: it returns `Effect`s for something else to carry out and
//! report back through another `Msg`.
//!
//! Control enablement (`play_enabled` and friends) is always derived from
//! state, never a separately-assigned flag: that is what makes it
//! impossible for the play button and the busy spinner to disagree.
//!
//! Every effect that resolves a directory is tagged with the `generation`
//! current when it was issued, and any reply carrying a stale generation is
//! discarded. That is what stops switching games mid-fetch from populating
//! the list with the other game's releases.

use std::collections::HashMap;
use std::path::PathBuf;

use turnstile_core::age::Age;
use turnstile_core::download::Status;
use turnstile_core::game::GameId;
use turnstile_core::release::Channel;
use turnstile_core::store::{DisableReport, Installed};

/// One row of the available-releases popup, already formatted for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseRow {
    pub tag: String,
    pub age: Option<Age>,
    pub channel: Channel,
}

/// Something is running. Present from the moment an operation is
/// *initiated* until the last reply for it lands -- not from its first
/// progress report, which is what it used to mean and which left every
/// `!is_busy()` guard open for the whole pre-progress window (and open
/// forever for `Activate`, `SwitchThenRemove` and `SetMode`, which report
/// no progress at all). See `UiState::outstanding`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Busy {
    /// What the worker last said it was doing, once it has said anything.
    /// `None` before the first `Msg::Progress`, and for the whole of an
    /// operation that never reports progress: the bar spins and the
    /// download button keeps its ordinary (disabled) title rather than
    /// claiming a phase that is not running.
    pub status: Option<Status>,
    pub value: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub title: String,
    pub message: String,
    /// `true` when `message` names a localization key rather than holding
    /// text to display verbatim.
    ///
    /// Almost every error here is some worker's failure, and its message is
    /// whatever the store, the network or the operating system said, which
    /// is not localizable and is shown as-is. The exception is an error the
    /// reducer raises itself: it has no reported text, so it names a key and
    /// `views::apply` looks it up, the same way it already looks up `title`.
    /// `state.rs` must stay free of `strings.rs` -- that is what lets the
    /// reducer be tested with no AppKit and no bundle around it -- so the
    /// choice is made here and the lookup happens there.
    pub message_is_key: bool,
    /// The generation this error belongs to, for errors raised by
    /// generation-gated work (`ReleasesLoaded`, `InstalledLoaded`,
    /// `InstallFinished`, `Activated`, `Removed`, `ModeChanged`): a
    /// generation bump makes these stale the same way it makes any other
    /// reply from that work stale, so `bump_generation` clears them.
    /// `None` for errors raised by work with no generation of its own
    /// (`LaunchFinished` is the only one), which a generation bump must
    /// leave alone, since toggling a channel or switching games has
    /// nothing to do with whether the previous launch succeeded.
    pub generation: Option<u64>,
    /// The argument for `title`'s `{0}`, when it has one. This is data, not
    /// formatted text: `state.rs` must stay free of `strings.rs`, which is
    /// what lets the reducer be tested with no AppKit and no bundle around
    /// it. `apply` is what actually substitutes it, via `strings::format1`.
    /// `None` for every title but `FailedToLaunchGame`, the only one of the
    /// seven error titles with a placeholder in it.
    pub title_arg: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pref {
    ShowDevelop(bool),
    AutoUpdate(bool),
    MultiVersion(bool),
    SelectedGame(GameId),
}

#[derive(Debug)]
pub struct UiState {
    pub selected_game: GameId,
    pub installed: Vec<Installed>,
    pub selected_installed: usize,
    /// Whatever `VersionStore::active` reported. Deliberately vague: it does
    /// not report the same *kind* of value in both modes -- `Installed::name`
    /// (the on-disk identity) in multi-version mode, but `Installed::tag` (a
    /// display label) in compatible mode, where the sole entry's `name` is
    /// always the literal string `"bin"` and useless as an identifier.
    /// Nothing outside `active_entry` may compare this field against
    /// `Installed::name` or `Installed::tag` directly -- go through
    /// `active_entry`, the one place that knows which of the two this is.
    pub active: Option<String>,
    pub releases: Vec<ReleaseRow>,
    /// Releases already fetched, so switching games does not ask GitHub
    /// again. Keyed on both things that decide what a fetch returns: the
    /// game, and whether development builds were included, since that second
    /// one changes which repositories are queried rather than merely
    /// filtering the result.
    ///
    /// Never invalidated. The unauthenticated API allows sixty requests an
    /// hour and bouncing between two games spent one each time; a launcher is
    /// open for a minute, so a release appearing mid-session is not worth a
    /// request per switch. Quitting and reopening refetches.
    release_cache: HashMap<(GameId, bool), Vec<ReleaseRow>>,
    pub selected_release: usize,
    pub show_develop: bool,
    /// Auto-update is a **per-game** preference (`autoInstallUpdates.
    /// <gameKey>`, see `prefs.rs`), so the in-memory mirror is per game
    /// too, one slot per `GameId::index`. It was a single game-less `bool`
    /// and that is the shape of this project's worst class of defect:
    /// `Msg::SelectGame` clears every game-scoped field and nothing ever
    /// reloaded this one, so after a sidebar click `ReleasesLoaded` issued
    /// an `Effect::Install` for the newly selected game on the strength of
    /// the *previous* game's setting -- in compatible mode, over a build
    /// the user deliberately kept.
    ///
    /// Private, and read only through `auto_update()`, which resolves it
    /// against `selected_game`. That is what stops a future call site
    /// picking the wrong game's slot; `show_develop` and `multi_version`
    /// above are deliberately global and stay plain `bool`s.
    auto_update: [bool; GameId::ALL.len()],
    pub multi_version: bool,
    pub busy: Option<Busy>,
    /// How many operations have been initiated in the current generation
    /// and not yet reported back. `busy` is derived from this, and it is a
    /// count rather than a flag because `Msg::ToggleMultiVersion` issues
    /// one `SetMode` per game: clearing `busy` on the first `ModeChanged`
    /// would re-open every guard while the second store is still
    /// mid-rename.
    ///
    /// Every operation that increments this posts exactly one reply that
    /// decrements it (`SwitchThenRemove` posts `Activated` *or* `Removed`,
    /// never both), and `bump_generation` zeroes it unconditionally, which
    /// is what keeps the UI from stranding in a permanently busy state if
    /// a reply is ever lost. Do not weaken that: it has already happened
    /// once on this branch and left the app inert with every control
    /// disabled.
    outstanding: u32,
    pub error: Option<Alert>,
    pub generation: u64,
    pending_removal: Option<PendingRemoval>,
}

/// What `ClickRemove` decided, held until the modal answers. Both names are
/// captured at click time and neither is ever re-derived afterwards:
/// `NSAlert::runModal` pumps a nested run loop, so an `InstalledLoaded`
/// already in flight can reshuffle `state.installed` while the dialog is
/// open, and re-reading the selection on the way out would let that
/// reshuffle redirect the operation at a different build than the one the
/// dialog named.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingRemoval {
    /// The build to switch to first. See `switch_target`.
    activate: String,
    /// The build to delete, once -- and only once -- the switch succeeded.
    remove: String,
}

impl UiState {
    pub fn new(game: GameId) -> UiState {
        UiState {
            selected_game: game,
            installed: Vec::new(),
            selected_installed: 0,
            active: None,
            releases: Vec::new(),
            release_cache: HashMap::new(),
            selected_release: 0,
            show_develop: false,
            auto_update: [false; GameId::ALL.len()],
            multi_version: false,
            busy: None,
            outstanding: 0,
            error: None,
            generation: 0,
            pending_removal: None,
        }
    }

    pub fn is_busy(&self) -> bool {
        self.busy.is_some()
    }

    /// The auto-update preference for the game currently selected, which is
    /// the only one any decision here may act on. See the field.
    pub fn auto_update(&self) -> bool {
        self.auto_update[self.selected_game.index()]
    }

    /// Sets one game's stored auto-update value. `controller::start` calls
    /// this once per game at launch, from preferences, so a later game
    /// switch needs no preference read and `reduce` stays pure;
    /// `Msg::ToggleAutoUpdate` is the only other writer and writes the
    /// selected game's slot.
    pub fn set_auto_update(&mut self, game: GameId, on: bool) {
        self.auto_update[game.index()] = on;
    }

    /// The on-disk identity of the current selection: what `Activate`,
    /// `SwitchThenRemove`, and the active-version comparison must use
    /// instead of the display tag.
    fn selected_installed_name(&self) -> Option<&str> {
        self.installed
            .get(self.selected_installed)
            .map(|i| i.name.as_str())
    }

    fn selected_installed_dir(&self) -> Option<&std::path::Path> {
        self.installed
            .get(self.selected_installed)
            .map(|i| i.dir.as_path())
    }

    /// The installed entry `self.active` refers to, resolved the correct way
    /// for whichever mode is current -- see `active`'s own doc comment for
    /// why that distinction exists. This is the only place allowed to
    /// compare `self.active` against `Installed::name` or `Installed::tag`
    /// directly; everything else goes through this.
    pub(crate) fn active_entry(&self) -> Option<&Installed> {
        let active = self.active.as_deref()?;
        self.installed
            .iter()
            .find(|i| is_active(i, active, self.multi_version))
    }

    pub fn selected_release_version(&self) -> Option<&str> {
        self.releases
            .get(self.selected_release)
            .map(|b| b.tag.as_str())
    }

    pub fn play_enabled(&self) -> bool {
        !self.is_busy()
            && self
                .installed
                .get(self.selected_installed)
                .is_some_and(|i| i.can_launch)
    }

    pub fn download_enabled(&self) -> bool {
        !self.is_busy() && !self.releases.is_empty() && !self.auto_update()
    }

    /// Remove deletes whatever the popup is showing, which is always the
    /// active version: `SelectInstalled` activates whatever was just
    /// picked, and `InstalledLoaded` puts the selection back on the active
    /// entry on every reload. So there is deliberately no "selected differs
    /// from active" condition here -- one was tried, and because of those
    /// two rules it was never satisfiable, which left the button greyed out
    /// forever. The active version is instead made removable by switching
    /// away from it first (see `switch_target` and
    /// `Effect::SwitchThenRemove`), which is why two installed versions are
    /// the real requirement: with one there is nothing to switch to.
    pub fn remove_enabled(&self) -> bool {
        !self.is_busy() && self.multi_version && self.installed.len() > 1
    }

    pub fn popup_enabled(&self) -> bool {
        !self.is_busy() && !self.auto_update()
    }
}

#[derive(Debug)]
pub enum Msg {
    SelectGame(GameId),
    SelectInstalled(usize),
    SelectRelease(usize),
    ToggleDevelop(bool),
    ToggleAutoUpdate(bool),
    ToggleMultiVersion(bool),
    ClickPlay,
    ClickDownload,
    ClickRemove,
    ConfirmRemove,
    InstalledLoaded {
        generation: u64,
        result: Result<(Vec<Installed>, Option<String>), String>,
    },
    ReleasesLoaded {
        generation: u64,
        result: Result<Vec<ReleaseRow>, String>,
    },
    Progress {
        generation: u64,
        status: Status,
        value: Option<f64>,
    },
    InstallFinished {
        generation: u64,
        result: Result<(), String>,
    },
    Activated {
        generation: u64,
        result: Result<(), String>,
    },
    Removed {
        generation: u64,
        result: Result<(), String>,
    },
    ModeChanged {
        generation: u64,
        result: Result<Option<DisableReport>, String>,
    },
    LaunchFinished {
        result: Result<(), String>,
    },
    /// `VersionStore::reconcile` failed for at least one game at launch.
    /// Posted once, after the first `SelectGame`, by `controller::start`:
    /// the repair runs before any generation exists, so it cannot report
    /// itself as the reply to anything, and `reduce` is still the only
    /// thing allowed to write `UiState`.
    StartupRepairFailed {
        message: String,
    },
}

#[derive(Debug)]
pub enum Effect {
    LoadInstalled {
        generation: u64,
        game: GameId,
    },
    FetchReleases {
        generation: u64,
        game: GameId,
        include_develop: bool,
    },
    Install {
        generation: u64,
        game: GameId,
        tag: String,
    },
    /// `name` is `Installed::name`, the on-disk identity, never the
    /// display tag: `VersionStore::activate` takes exactly this and
    /// nothing else, by contract.
    Activate {
        generation: u64,
        game: GameId,
        name: String,
    },
    /// Switch to `activate`, then -- only if that switched for real --
    /// delete `remove`. One effect rather than two because the ordering
    /// between them is the whole point: `VersionStore::remove` refuses to
    /// delete the active version (`StoreError::RemoveActive`), which is the
    /// last thing standing between a mistake here and a `bin` symlink
    /// pointing at a directory that no longer exists. Splitting this into
    /// an `Activate` whose reply triggers a `Remove` would put a main-loop
    /// turn between them, during which any other message -- another
    /// `Activated` from the popup, most obviously -- could pair the wrong
    /// two halves together.
    ///
    /// Both names carry the same identity contract as `Activate`:
    /// `Installed::name`, never the display `tag`.
    SwitchThenRemove {
        generation: u64,
        game: GameId,
        activate: String,
        remove: String,
    },
    SetMode {
        generation: u64,
        game: GameId,
        multi_version: bool,
    },
    Launch {
        game: GameId,
        dir: PathBuf,
    },
    /// Same identity contract as `Activate`.
    ConfirmRemove {
        name: String,
    },
    ShowDisableReport(DisableReport),
    SavePref(Pref),
    /// A generation bump means whatever was running for the old generation
    /// is now irrelevant. `generation` is the generation being abandoned
    /// (the one current before the bump), not the new one.
    ///
    /// The controller's executor holds one cancel flag per in-flight
    /// install, never shared across installs, and sets whichever one is
    /// currently stored -- a no-op if nothing is running. Because each
    /// install's flag is its own, this cannot un-cancel a different install
    /// the way a single shared flag could. It does not branch on
    /// `generation` to do that (one flag slot is enough), so this field is
    /// read only by this module's own tests, which check that the abandoned
    /// generation -- not the new one -- is what gets reported.
    CancelInFlight {
        #[allow(dead_code)]
        generation: u64,
    },
}

/// All the interesting behavior, with no AppKit anywhere near it.
pub fn reduce(state: &mut UiState, msg: Msg) -> Vec<Effect> {
    match msg {
        Msg::SelectGame(game) => {
            let cancelled = bump_generation(state);
            state.selected_game = game;
            state.installed.clear();
            state.releases.clear();
            state.active = None;
            state.selected_installed = 0;
            state.selected_release = 0;
            let mut effects = vec![
                Effect::CancelInFlight {
                    generation: cancelled,
                },
                Effect::SavePref(Pref::SelectedGame(game)),
                Effect::LoadInstalled {
                    generation: state.generation,
                    game,
                },
            ];
            // Switching back to a game already looked at asks GitHub nothing.
            // Through `adopt_releases` rather than by assigning, so a cache
            // hit still evaluates auto-update.
            match cached(state, game, state.show_develop) {
                Some(rows) => effects.extend(adopt_releases(state, rows)),
                None => effects.push(Effect::FetchReleases {
                    generation: state.generation,
                    game,
                    include_develop: state.show_develop,
                }),
            }
            effects
        }

        Msg::SelectInstalled(index) => {
            // Refused outright while something is in flight, rather than
            // moving the selection and declining only the activation: the
            // popup always shows the active version, so a selection that
            // moved without activating would be a lie about what is
            // running. `views::apply` runs after every reduce and puts the
            // popup back on `selected_installed`, so the refusal is not
            // visible as a stuck control.
            if state.is_busy() {
                return Vec::new();
            }
            state.selected_installed = index;
            let Some(name) = state.selected_installed_name().map(str::to_string) else {
                return Vec::new();
            };
            // Selecting *is* switching, but only where switching exists.
            //
            // The comparison routes through `active_entry` rather than reading
            // `state.active` directly. It would be correct either way here --
            // the `multi_version` guard is what makes `active` a name in this
            // branch -- but that is a fact a reader has to re-derive, and
            // re-deriving it per site is precisely how this went wrong twice.
            if state.multi_version
                && state.active_entry().map(|e| e.name.as_str()) != Some(name.as_str())
            {
                begin(state, 1);
                vec![Effect::Activate {
                    generation: state.generation,
                    game: state.selected_game,
                    name,
                }]
            } else {
                Vec::new()
            }
        }

        Msg::SelectRelease(index) => {
            state.selected_release = index;
            Vec::new()
        }

        Msg::ToggleDevelop(on) => {
            let cancelled = bump_generation(state);
            state.show_develop = on;
            let mut effects = vec![
                Effect::CancelInFlight {
                    generation: cancelled,
                },
                Effect::SavePref(Pref::ShowDevelop(on)),
            ];
            // Cached per channel choice as well as per game, because turning
            // development builds on queries a second repository rather than
            // filtering what the first returned. So each of the two states is
            // fetched once and then toggling between them is free.
            match cached(state, state.selected_game, on) {
                Some(rows) => effects.extend(adopt_releases(state, rows)),
                None => effects.push(Effect::FetchReleases {
                    generation: state.generation,
                    game: state.selected_game,
                    include_develop: on,
                }),
            }
            effects
        }

        Msg::ToggleAutoUpdate(on) => {
            // The selected game's slot, and nothing else: the preference is
            // per game on disk (`autoInstallUpdates.<gameKey>`) and the
            // `SavePref` below is written under the same
            // `state.selected_game` by `controller::perform`.
            state.set_auto_update(state.selected_game, on);
            let mut effects = vec![Effect::SavePref(Pref::AutoUpdate(on))];
            // The preference is saved either way; only the install it would
            // trigger waits. Something is already running against this
            // game's store, and whatever it is, the next `ReleasesLoaded`
            // re-evaluates this with the setting now on.
            if on
                && !state.is_busy()
                && let Some(tag) = newest_installable(state)
            {
                begin(state, 1);
                effects.push(Effect::Install {
                    generation: state.generation,
                    game: state.selected_game,
                    tag,
                });
            }
            effects
        }

        Msg::ToggleMultiVersion(on) => {
            // Refused while anything is in flight, and refused here rather
            // than relying on the checkbox being disabled. This is the one
            // toggle that issues store operations -- renames of `bin` and
            // of whole version directories -- and `Effect::CancelInFlight`
            // cannot stop one: the cancel flag is read by `download.rs`
            // alone, never by the store. A second toggle arriving mid-rename
            // therefore aims two workers at the same `bin`; `storelock` is
            // what stops them being inside it at once, and this guard is
            // what stops the app claiming to be doing both, which is a
            // separate obligation and not one a lock can meet.
            // `views::apply` runs after every reduce, so the checkbox snaps
            // back to `state.multi_version` instead of showing a value that
            // was not acted on.
            if state.is_busy() {
                return Vec::new();
            }
            let cancelled = bump_generation(state);
            state.multi_version = on;
            let mut effects = vec![
                Effect::CancelInFlight {
                    generation: cancelled,
                },
                Effect::SavePref(Pref::MultiVersion(on)),
            ];
            // Global preference, so every game's layout changes.
            for game in GameId::ALL {
                effects.push(Effect::SetMode {
                    generation: state.generation,
                    game,
                    multi_version: on,
                });
            }
            // After the bump, which zeroes the count: one operation per
            // game, so `busy` holds until the last `ModeChanged` lands.
            begin(state, GameId::ALL.len() as u32);
            effects
        }

        Msg::ClickPlay => match state.selected_installed_dir() {
            Some(dir) if state.play_enabled() => {
                vec![Effect::Launch {
                    game: state.selected_game,
                    dir: dir.to_path_buf(),
                }]
            }
            _ => Vec::new(),
        },

        Msg::ClickDownload => {
            let Some(tag) = state.selected_release_version().map(str::to_string) else {
                return Vec::new();
            };
            if !state.download_enabled() {
                return Vec::new();
            }
            begin(state, 1);
            vec![Effect::Install {
                generation: state.generation,
                game: state.selected_game,
                tag,
            }]
        }

        Msg::ClickRemove => {
            // Cleared first, on every path out of this arm. Cancelling the
            // dialog is simply the absence of a `ConfirmRemove`, so a
            // cancelled removal otherwise leaves its pair sitting here; no
            // route to `ConfirmRemove` exists except a dialog this arm has
            // just re-armed, but leaving a stale deletion target in the
            // state and relying on that is the sort of thing that stops
            // being true later.
            state.pending_removal = None;
            if !state.remove_enabled() {
                return Vec::new();
            }
            let Some(remove) = state.selected_installed_name().map(str::to_string) else {
                return Vec::new();
            };
            // Decided here, not after the modal: see `PendingRemoval`.
            let Some(activate) = switch_target(state, &remove) else {
                return Vec::new();
            };
            state.pending_removal = Some(PendingRemoval {
                activate,
                remove: remove.clone(),
            });
            vec![Effect::ConfirmRemove { name: remove }]
        }

        // The pair is taken on every path, including the refusal: a
        // confirmation that is not acted on must not stay armed for a
        // later one to pick up.
        //
        // `is_busy()` is re-checked here and not only at `ClickRemove`,
        // because `NSAlert::runModal` pumps a nested run loop: messages
        // really are delivered between the click and the answer, so
        // `remove_enabled()` holding at click time does not mean it still
        // holds now.
        Msg::ConfirmRemove => match state.pending_removal.take() {
            Some(pending) if !state.is_busy() => {
                begin(state, 1);
                vec![Effect::SwitchThenRemove {
                    generation: state.generation,
                    game: state.selected_game,
                    activate: pending.activate,
                    remove: pending.remove,
                }]
            }
            // Refused, and *said* so. The user answered a destructive
            // confirmation and is entitled to know that nothing happened:
            // `remove_enabled()` does not depend on `auto_update`, so with
            // auto-update on a `ReleasesLoaded` delivered through the
            // modal's nested run loop starts an install of its own and lands
            // the confirmation here. Silently returning no effects left the
            // Remove button working, the dialog answered, the build still
            // installed, and nothing on screen to explain any of it; the
            // only recourse was to click Remove again and guess.
            //
            // Generation-scoped, like every other error raised while this
            // generation's work is in flight: switching games or toggling a
            // channel dismisses it, which is right, because the operation it
            // was refused in favour of is abandoned by the same bump. The
            // install it is refused in favour of will also clear it when it
            // succeeds, which is the documented edge of the
            // `Option<generation>` error class rather than a decision made
            // here -- and in this one case the clear is defensible, since by
            // then the refusal's reason has gone and Remove works again.
            //
            // `title` is deliberately the same `FailedToRemoveVersion` a
            // failed removal shows: from the user's side the outcome is the
            // same (the version was not removed) and the reason belongs in
            // the message, which is where every other banner in the app puts
            // it.
            Some(_) => {
                state.error = Some(Alert {
                    title: "FailedToRemoveVersion".into(),
                    message: "RemoveBusyMessage".into(),
                    message_is_key: true,
                    generation: Some(state.generation),
                    title_arg: None,
                });
                Vec::new()
            }
            // Nothing was armed, so nothing was refused and there is
            // nothing to report: this is a `ConfirmRemove` with no dialog
            // behind it, which `Msg::ClickRemove`'s disarming makes
            // unreachable today and which must stay silent if it ever is
            // not.
            None => Vec::new(),
        },

        Msg::InstalledLoaded { generation, result } => {
            if generation != state.generation {
                return Vec::new();
            }
            match result {
                Ok((installed, active)) => {
                    // `installed`/`active` are the freshly loaded values,
                    // not yet in `state`, so `active_entry` (which reads
                    // `state.installed`/`state.active`) cannot be used here;
                    // `is_active` is the same mode-aware comparison it uses
                    // internally, applied to these local values instead.
                    state.selected_installed = active
                        .as_deref()
                        .and_then(|a| {
                            installed
                                .iter()
                                .position(|i| is_active(i, a, state.multi_version))
                        })
                        .unwrap_or(0);
                    state.installed = installed;
                    state.active = active;
                }
                Err(message) => {
                    state.error = Some(Alert {
                        title: "FailedToReadInstalledVersions".into(),
                        message,
                        message_is_key: false,
                        generation: Some(generation),
                        title_arg: None,
                    });
                }
            }
            Vec::new()
        }

        Msg::ReleasesLoaded { generation, result } => {
            if generation != state.generation {
                return Vec::new();
            }
            match result {
                Ok(rows) => {
                    state
                        .release_cache
                        .insert((state.selected_game, state.show_develop), rows.clone());
                    return adopt_releases(state, rows);
                }
                Err(message) => {
                    state.error = Some(Alert {
                        title: "FailedToObtainBuilds".into(),
                        message,
                        message_is_key: false,
                        generation: Some(generation),
                        title_arg: None,
                    });
                }
            }
            Vec::new()
        }

        // Only ever *updates* a busy state an initiated operation already
        // established; it never creates one. Setting `busy` here was the
        // whole of the old behaviour, which is why every `!is_busy()` guard
        // stood open until the first report arrived -- seconds into an
        // install, and forever for the operations that report nothing.
        //
        // The `outstanding` test is belt and braces rather than load
        // bearing: the main queue is FIFO, so a worker's last `Progress` is
        // always delivered before the reply that ends it. It costs one
        // comparison to make a lost or reordered report unable to strand
        // the UI busy with nothing running.
        Msg::Progress {
            generation,
            status,
            value,
        } => {
            if generation == state.generation && state.outstanding > 0 {
                state.busy = Some(Busy {
                    status: Some(status),
                    value,
                });
            }
            Vec::new()
        }

        Msg::InstallFinished { generation, result } => {
            if generation != state.generation {
                return Vec::new();
            }
            end(state);
            match result {
                Ok(()) => {
                    state.error = None;
                    vec![Effect::LoadInstalled {
                        generation: state.generation,
                        game: state.selected_game,
                    }]
                }
                Err(message) => {
                    state.error = Some(Alert {
                        title: "DownloadBuildFailedTitle".into(),
                        message,
                        message_is_key: false,
                        generation: Some(generation),
                        title_arg: None,
                    });
                    Vec::new()
                }
            }
        }

        Msg::Activated { generation, result } => {
            finish(state, generation, result, "FailedToSwitchVersion")
        }

        Msg::Removed { generation, result } => {
            if generation != state.generation {
                return Vec::new();
            }
            end(state);
            match result {
                Ok(()) => state.error = None,
                Err(message) => {
                    state.error = Some(Alert {
                        title: "FailedToRemoveVersion".into(),
                        message,
                        message_is_key: false,
                        generation: Some(generation),
                        title_arg: None,
                    });
                }
            }
            // Reloads on the failure path too, which `finish` deliberately
            // does not. A removal is the one operation that can fail after
            // already having changed what is on disk: the delete is only
            // ever attempted once the switch away from the active version
            // has succeeded, so a failed delete still leaves `bin` pointing
            // somewhere new. Without this reload the popup would go on
            // showing the old build as the active one.
            vec![Effect::LoadInstalled {
                generation: state.generation,
                game: state.selected_game,
            }]
        }

        Msg::ModeChanged { generation, result } => {
            if generation != state.generation {
                return Vec::new();
            }
            end(state);
            match result {
                Ok(report) => {
                    state.error = None;
                    let mut effects = vec![Effect::LoadInstalled {
                        generation: state.generation,
                        game: state.selected_game,
                    }];
                    if let Some(report) = report.filter(|r| !r.retained.is_empty()) {
                        effects.push(Effect::ShowDisableReport(report));
                    }
                    effects
                }
                Err(message) => {
                    state.error = Some(Alert {
                        title: "FailedToChangeVersionMode".into(),
                        message,
                        message_is_key: false,
                        generation: Some(generation),
                        title_arg: None,
                    });
                    Vec::new()
                }
            }
        }

        // Scoped to the current generation, which is the one the initial
        // `SelectGame` just established, so the next game switch or channel
        // toggle clears it the same way it clears a failed read of the same
        // store. Deliberately *not* `generation: None`: that class belongs
        // to `LaunchFinished` alone, and `LaunchFinished`'s `Ok` arm clears
        // every error in it, so borrowing it here would let a successful
        // launch dismiss an unrelated repair failure the user had not read.
        Msg::StartupRepairFailed { message } => {
            state.error = Some(Alert {
                title: "FailedToRepairInstallation".into(),
                message,
                message_is_key: false,
                generation: Some(state.generation),
                title_arg: None,
            });
            Vec::new()
        }

        Msg::LaunchFinished { result } => {
            match result {
                Ok(()) => {
                    // The mirror image of bump_generation: that clears
                    // only generation-scoped errors and leaves a launch
                    // error alone; this clears only a launch error
                    // (generation: None) and leaves a generation-scoped
                    // one alone. A launch carries no generation of its
                    // own, so nothing else ever clears a launch error --
                    // a fresh success has to -- but it must not also
                    // dismiss an unrelated fetch or install error the
                    // user hasn't read yet. Each event clears exactly the
                    // error class it owns.
                    if state.error.as_ref().is_some_and(|a| a.generation.is_none()) {
                        state.error = None;
                    }
                }
                Err(message) => {
                    state.error = Some(Alert {
                        title: "FailedToLaunchGame".into(),
                        message,
                        message_is_key: false,
                        generation: None,
                        title_arg: Some(state.selected_game.display_name().to_string()),
                    });
                }
            }
            Vec::new()
        }
    }
}

/// Records that `count` operations have just been initiated, so every
/// `!is_busy()` guard closes *now* rather than whenever the first progress
/// report happens to arrive. Called in the same arm that emits the effects,
/// never anywhere else: an increment with no effect behind it is an
/// operation that never replies, and `busy` would stay set until the next
/// generation bump.
///
/// The `Busy` it establishes carries no status, because at this point
/// nothing has said what it is doing yet; `Msg::Progress` fills that in for
/// the one operation that reports any.
///
/// Deliberately not called for `Effect::Launch`. A launch reports back as
/// `Msg::LaunchFinished`, the one reply with no generation of its own, so
/// it cannot be reconciled with a count that a generation bump zeroes: a
/// bump followed by a new operation and then a late `LaunchFinished` would
/// decrement someone else's count and clear `busy` with work still
/// running. `play_enabled` already refuses to start a launch while busy,
/// which is the guard that actually matters here.
fn begin(state: &mut UiState, count: u32) {
    state.outstanding += count;
    if state.busy.is_none() {
        state.busy = Some(Busy {
            status: None,
            value: None,
        });
    }
}

/// One initiated operation has reported back, successfully or not. `busy`
/// clears only when the last outstanding one does: `Msg::ToggleMultiVersion`
/// issues one `SetMode` per game, so two `ModeChanged` replies arrive for a
/// single initiation, and clearing on the first would re-open every guard
/// while the second store is still mid-rename.
///
/// `saturating_sub` rather than `-= 1` so a reply that somehow outlives its
/// count cannot wrap the counter to `u32::MAX` and leave the UI permanently
/// busy. Every caller is already behind a generation check, and
/// `bump_generation` zeroes the count, so this should not be reachable; the
/// failure it would cause is bad enough to be worth one saturating call.
fn end(state: &mut UiState) {
    state.outstanding = state.outstanding.saturating_sub(1);
    if state.outstanding == 0 {
        state.busy = None;
    }
}

/// Advances to a new generation, returning the one being abandoned so the
/// caller can tell the executor to cancel whatever was running under it.
/// Also clears `busy` and the outstanding count unconditionally, since they
/// always describe in-flight work tied to the old generation, and every
/// reply that would otherwise clear them is itself gated on a generation
/// match, so nothing else ever would. **This is what makes setting `busy`
/// at initiation safe rather than a way to wedge the app**: an operation
/// whose reply is discarded as stale would otherwise leave every control
/// disabled forever, which has already happened once on this branch. Do not
/// make either reset conditional.
///
/// It does not stop the abandoned work itself. `Effect::CancelInFlight`
/// reaches a download; nothing reaches a store operation already mid-rename.
/// So clearing `busy` here really does re-open every `!is_busy()` guard
/// while an abandoned store operation is still running, and the app really
/// will let a new one be *initiated* in that window -- three clicks reach
/// it, the second being a sidebar click, since the sidebar table is the one
/// control never disabled. What it cannot do is let the two run *together*:
/// every store operation runs under one lock (`storelock`), taken on the
/// worker and held for the operation's whole duration, so the new one waits
/// for the abandoned one to finish. Closing the UI-level window instead
/// would mean refusing to switch games during an install, which this app
/// deliberately allows and cancels instead.
///
/// `error` is cleared only when it is generation-scoped: a launch
/// error (`Alert::generation` is `None`) has nothing to do with switching
/// games or toggling a channel, and clearing it here would erase it before
/// the user has read it.
fn bump_generation(state: &mut UiState) -> u64 {
    let previous = state.generation;
    state.generation += 1;
    state.outstanding = 0;
    state.busy = None;
    if state
        .error
        .as_ref()
        .is_some_and(|alert| alert.generation.is_some())
    {
        state.error = None;
    }
    previous
}

fn finish(
    state: &mut UiState,
    generation: u64,
    result: Result<(), String>,
    error_title: &str,
) -> Vec<Effect> {
    if generation != state.generation {
        return Vec::new();
    }
    end(state);
    match result {
        Ok(()) => {
            state.error = None;
            vec![Effect::LoadInstalled {
                generation: state.generation,
                game: state.selected_game,
            }]
        }
        Err(message) => {
            state.error = Some(Alert {
                title: error_title.into(),
                message,
                message_is_key: false,
                generation: Some(generation),
                title_arg: None,
            });
            Vec::new()
        }
    }
}

/// Is `entry` the one `active` (as `VersionStore::active` reports it, in
/// whichever mode is current) refers to? See `UiState::active`'s doc comment
/// for why this cannot be a plain `==`: `active` is an `Installed::name` in
/// multi-version mode but an `Installed::tag` in compatible mode. This is
/// the one place that knows that distinction; `UiState::active_entry` and
/// every reducer arm that needs the same comparison on values not yet
/// stored in `state` (`InstalledLoaded`, below) go through this instead of
/// picking a field directly.
fn is_active(entry: &Installed, active: &str, multi_version: bool) -> bool {
    if multi_version {
        entry.name == active
    } else {
        entry.tag == active
    }
}

/// Which build to switch to before `name` can be deleted: the newest other
/// installed version, or `None` when there is no other one.
///
/// "Newest" is `VersionStore::installed`'s own order, not a re-sort here:
/// `installed_multi` reads each directory's modification time and sorts by
/// `Reverse(modified)`, so entry zero is the most recently written build
/// and the first entry with a different `name` is the newest other one.
/// Compares `name`, never `tag`: two installed entries can share a display
/// tag, and switching to the wrong one of those would leave the build the
/// user asked to delete still active, which `VersionStore::remove` would
/// then refuse.
fn switch_target(state: &UiState, name: &str) -> Option<String> {
    state
        .installed
        .iter()
        .find(|i| i.name != name)
        .map(|i| i.name.clone())
}

/// Takes a set of releases as the current ones, from a fetch or from the
/// cache, and returns whatever that implies.
///
/// Both paths go through here on purpose. Auto-update is evaluated when
/// releases arrive, so a cache hit that merely assigned `state.releases`
/// would silently stop auto-update from ever firing on a game switch: two
/// correct-looking rules composing into a gap neither owns.
fn adopt_releases(state: &mut UiState, rows: Vec<ReleaseRow>) -> Vec<Effect> {
    state.releases = rows;
    state.selected_release = 0;
    // `auto_update()` resolves against `state.selected_game`, so this can
    // only ever act on the setting stored for the game whose releases these
    // are.
    //
    // No `is_busy()` guard, unlike the arms a user drives. This is not a
    // click to be distrusted, and refusing it would mean auto-update quietly
    // not updating for the rest of the session, since nothing re-runs it.
    // `outstanding` is a count precisely so a second operation overlapping a
    // first is accounted for rather than losing one of the two clears.
    if state.auto_update()
        && let Some(tag) = newest_installable(state)
    {
        begin(state, 1);
        return vec![Effect::Install {
            generation: state.generation,
            game: state.selected_game,
            tag,
        }];
    }
    Vec::new()
}

/// The releases for a game and channel choice if they have been fetched
/// already, so the caller can skip the request.
fn cached(state: &UiState, game: GameId, include_develop: bool) -> Option<Vec<ReleaseRow>> {
    state.release_cache.get(&(game, include_develop)).cloned()
}

/// The newest listed release's tag, unless it is already the one active.
/// Compares the release's tag against the *tag* of whichever installed
/// entry currently matches `active`, rather than comparing the release tag
/// directly against `active`: in multi-version mode `active` is an identity
/// (`Installed::name`), not a tag, and even where it is a tag (compatible
/// mode) a directory renamed to resolve a collision keeps its original tag,
/// so only a tag-to-tag comparison is meaningful here.
fn newest_installable(state: &UiState) -> Option<String> {
    let newest = state.releases.first()?;
    let active_tag = state.active_entry().map(|i| i.tag.as_str());
    if active_tag == Some(newest.tag.as_str()) {
        return None;
    }
    Some(newest.tag.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use turnstile_core::store::Installed;

    fn installed(tag: &str, can_launch: bool) -> Installed {
        installed_named(tag, tag, can_launch)
    }

    /// Like `installed`, but lets a test give `name` and `tag` different
    /// values -- the one axis `installed` itself cannot express, and the
    /// one this whole project treats as its worst-bug hazard (see
    /// `Installed`'s own doc comment in `turnstile-core`). Every test that
    /// only calls `installed` is silently unable to catch a bug that
    /// conflates the two; a handful below use this instead specifically to
    /// close that gap.
    fn installed_named(tag: &str, name: &str, can_launch: bool) -> Installed {
        Installed {
            tag: tag.to_string(),
            name: name.to_string(),
            dir: std::path::PathBuf::from(format!("/tmp/{name}")),
            can_launch,
        }
    }

    fn rows(tags: &[&str]) -> Vec<ReleaseRow> {
        tags.iter()
            .map(|t| ReleaseRow {
                tag: (*t).into(),
                age: None,
                channel: Channel::Release,
            })
            .collect()
    }

    #[test]
    fn switching_back_to_a_game_already_fetched_asks_github_nothing() {
        let mut s = UiState::new(GameId::OpenRCT2);
        // Arriving releases are what fills the cache.
        let g = s.generation;
        reduce(
            &mut s,
            Msg::ReleasesLoaded {
                generation: g,
                result: Ok(rows(&["v3", "v2"])),
            },
        );
        let away = reduce(&mut s, Msg::SelectGame(GameId::OpenLoco));
        assert!(
            away.iter()
                .any(|e| matches!(e, Effect::FetchReleases { .. })),
            "the other game has never been fetched, so it must be"
        );
        let back = reduce(&mut s, Msg::SelectGame(GameId::OpenRCT2));
        assert!(
            !back.iter()
                .any(|e| matches!(e, Effect::FetchReleases { .. })),
            "returning to a game already fetched must not ask again"
        );
        assert_eq!(
            s.releases.iter().map(|r| r.tag.as_str()).collect::<Vec<_>>(),
            vec!["v3", "v2"],
            "and the cached releases must actually be restored"
        );
    }

    #[test]
    fn a_cache_hit_still_evaluates_auto_update() {
        // The trap this guards: auto-update is evaluated when releases
        // arrive, so serving them from the cache without going through the
        // same path would stop it firing on a game switch. Both paths run
        // through `adopt_releases` for exactly this reason.
        let mut s = UiState::new(GameId::OpenRCT2);
        s.set_auto_update(GameId::OpenRCT2, true);
        s.installed = vec![installed("v2", true)];
        s.active = Some("v2".into());
        let g = s.generation;
        reduce(
            &mut s,
            Msg::ReleasesLoaded {
                generation: g,
                result: Ok(rows(&["v3", "v2"])),
            },
        );
        reduce(&mut s, Msg::SelectGame(GameId::OpenLoco));
        let back = reduce(&mut s, Msg::SelectGame(GameId::OpenRCT2));
        assert!(
            back.iter()
                .any(|e| matches!(e, Effect::Install { tag, .. } if tag == "v3")),
            "a cache hit must still install the newer build when auto-update is on"
        );
    }

    #[test]
    fn the_cache_is_keyed_on_the_channel_choice_too() {
        // Turning development builds on queries a second repository rather
        // than filtering the first, so a cache keyed on the game alone would
        // serve release-only rows as though they included development ones.
        let mut s = UiState::new(GameId::OpenRCT2);
        let g = s.generation;
        reduce(
            &mut s,
            Msg::ReleasesLoaded {
                generation: g,
                result: Ok(rows(&["v3"])),
            },
        );
        let on = reduce(&mut s, Msg::ToggleDevelop(true));
        assert!(
            on.iter().any(|e| matches!(e, Effect::FetchReleases { .. })),
            "the develop channel has not been fetched, so it must be"
        );
        let g = s.generation;
        reduce(
            &mut s,
            Msg::ReleasesLoaded {
                generation: g,
                result: Ok(rows(&["v4-dev", "v3"])),
            },
        );
        let off = reduce(&mut s, Msg::ToggleDevelop(false));
        assert!(
            !off.iter()
                .any(|e| matches!(e, Effect::FetchReleases { .. })),
            "both channel states have now been fetched, so toggling is free"
        );
        assert_eq!(
            s.releases.iter().map(|r| r.tag.as_str()).collect::<Vec<_>>(),
            vec!["v3"],
            "and turning it off restores the release-only rows"
        );
    }

    fn ready_state() -> UiState {
        let mut s = UiState::new(GameId::OpenRCT2);
        s.installed = vec![installed("v2", true), installed("v1", true)];
        s.active = Some("v2".into());
        s.releases = vec![
            ReleaseRow {
                tag: "v3".into(),
                age: None,
                channel: Channel::Release,
            },
            ReleaseRow {
                tag: "v2".into(),
                age: None,
                channel: Channel::Release,
            },
        ];
        s
    }

    #[test]
    fn a_fresh_state_can_do_nothing_until_something_loads() {
        let s = UiState::new(GameId::OpenRCT2);
        assert!(!s.play_enabled());
        assert!(!s.download_enabled());
        assert!(!s.remove_enabled());
    }

    #[test]
    fn play_is_enabled_only_when_the_selection_is_launchable_and_idle() {
        let mut s = ready_state();
        assert!(s.play_enabled());

        s.installed[0].can_launch = false;
        assert!(!s.play_enabled(), "a broken install is not playable");

        s.installed[0].can_launch = true;
        s.busy = Some(Busy {
            status: Some(Status::Downloading),
            value: Some(0.5),
        });
        assert!(!s.play_enabled(), "nothing is enabled while busy");
    }

    #[test]
    fn download_is_disabled_while_auto_update_is_on() {
        let mut s = ready_state();
        assert!(s.download_enabled());
        s.set_auto_update(s.selected_game, true);
        assert!(!s.download_enabled());
    }

    #[test]
    fn download_is_disabled_when_no_builds_are_listed() {
        let mut s = ready_state();
        s.releases.clear();
        assert!(!s.download_enabled());
    }

    #[test]
    fn download_is_disabled_while_busy() {
        let mut s = ready_state();
        assert!(s.download_enabled());
        s.busy = Some(Busy {
            status: Some(Status::Downloading),
            value: Some(0.5),
        });
        assert!(!s.download_enabled(), "nothing is enabled while busy");
    }

    /// The regression Task 22's smoke test found. Remove used to require
    /// the selection to differ from the active version, which
    /// `SelectInstalled` and `InstalledLoaded` between them make
    /// impossible, so the button was permanently grey. The active version
    /// is removable now because the app switches away from it first.
    #[test]
    fn remove_is_enabled_for_the_active_version() {
        let mut s = ready_state();
        s.multi_version = true;
        s.selected_installed = 0; // v2, the active one
        assert!(
            s.remove_enabled(),
            "the active version is what the popup always shows"
        );
        s.selected_installed = 1; // v1
        assert!(s.remove_enabled());
    }

    #[test]
    fn remove_is_always_disabled_in_compatible_mode() {
        let mut s = ready_state();
        s.multi_version = false;
        s.selected_installed = 1;
        assert!(!s.remove_enabled());
    }

    #[test]
    fn remove_is_disabled_while_busy() {
        let mut s = ready_state();
        s.multi_version = true;
        s.selected_installed = 1; // v1, not the active version
        assert!(s.remove_enabled());
        s.busy = Some(Busy {
            status: Some(Status::Downloading),
            value: Some(0.5),
        });
        assert!(!s.remove_enabled(), "nothing is enabled while busy");
    }

    #[test]
    fn remove_is_disabled_when_only_one_version_is_installed() {
        let mut s = ready_state();
        s.multi_version = true;
        s.installed = vec![installed("v1", true)];
        s.active = Some("v1".into());
        s.selected_installed = 0;
        assert!(
            !s.remove_enabled(),
            "with nothing to switch to first, the only installed version cannot be removed"
        );
    }

    #[test]
    fn popup_is_enabled_unless_busy_or_auto_updating() {
        let mut s = ready_state();
        assert!(s.popup_enabled());

        s.busy = Some(Busy {
            status: Some(Status::Downloading),
            value: Some(0.5),
        });
        assert!(!s.popup_enabled(), "nothing is enabled while busy");
        s.busy = None;

        s.set_auto_update(s.selected_game, true);
        assert!(!s.popup_enabled(), "auto-update owns the version choice");
    }

    #[test]
    fn selecting_an_installed_version_activates_it_in_multi_version_mode() {
        let mut s = ready_state();
        s.multi_version = true;
        let effects = reduce(&mut s, Msg::SelectInstalled(1));
        assert_eq!(s.selected_installed, 1);
        assert!(effects.iter().any(|e| matches!(
            e, Effect::Activate { name, .. } if name == "v1"
        )));
    }

    #[test]
    fn activate_uses_the_installed_name_not_the_shared_display_tag() {
        let mut s = ready_state();
        // A collision two `.version` files can produce: both say "v2", but
        // `adopt_dir`'s free-name walk keeps the directory names distinct.
        s.installed = vec![
            Installed {
                tag: "v2".into(),
                name: "v2".into(),
                dir: std::path::PathBuf::from("/tmp/v2"),
                can_launch: true,
            },
            Installed {
                tag: "v2".into(),
                name: "v2 (2)".into(),
                dir: std::path::PathBuf::from("/tmp/v2-2"),
                can_launch: true,
            },
        ];
        s.active = Some("v2".into()); // the first entry, by name, is active
        s.multi_version = true;

        let effects = reduce(&mut s, Msg::SelectInstalled(1)); // the collision-renamed entry

        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::Activate { name, .. } if name == "v2 (2)")),
            "must activate by the on-disk name; using the shared display tag would either \
             activate the wrong entry or, since both share \"v2\", wrongly conclude nothing \
             changed and activate nothing at all"
        );
    }

    #[test]
    fn selecting_an_installed_version_does_not_activate_in_compatible_mode() {
        let mut s = ready_state();
        s.multi_version = false;
        let effects = reduce(&mut s, Msg::SelectInstalled(0));
        assert!(!effects.iter().any(|e| matches!(e, Effect::Activate { .. })));
    }

    #[test]
    fn switching_game_bumps_the_generation_and_refreshes() {
        let mut s = ready_state();
        let before = s.generation;
        let effects = reduce(&mut s, Msg::SelectGame(GameId::OpenLoco));
        assert_eq!(s.selected_game, GameId::OpenLoco);
        assert!(
            s.generation > before,
            "a new generation invalidates in-flight replies"
        );
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::LoadInstalled { .. }))
        );
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::FetchReleases { .. }))
        );
    }

    #[test]
    fn switching_game_cancels_in_flight_work_and_unsticks_the_ui() {
        let mut s = ready_state();
        s.busy = Some(Busy {
            status: Some(Status::Downloading),
            value: Some(0.5),
        });
        s.error = Some(Alert {
            title: "old".into(),
            message: "stale".into(),
            message_is_key: false,
            generation: Some(s.generation),
            title_arg: None,
        });
        let before = s.generation;

        let effects = reduce(&mut s, Msg::SelectGame(GameId::OpenLoco));

        assert!(
            s.busy.is_none(),
            "a generation bump must not leave the UI stuck busy"
        );
        assert!(
            s.error.is_none(),
            "a stale error must not follow into the new generation"
        );
        assert!(
            effects.iter().any(
                |e| matches!(e, Effect::CancelInFlight { generation } if *generation == before)
            )
        );
    }

    #[test]
    fn toggling_develop_cancels_in_flight_work_and_unsticks_the_ui() {
        let mut s = ready_state();
        s.busy = Some(Busy {
            status: Some(Status::Downloading),
            value: Some(0.5),
        });
        s.error = Some(Alert {
            title: "old".into(),
            message: "stale".into(),
            message_is_key: false,
            generation: Some(s.generation),
            title_arg: None,
        });
        let before = s.generation;

        let effects = reduce(&mut s, Msg::ToggleDevelop(true));

        assert!(
            s.busy.is_none(),
            "a generation bump must not leave the UI stuck busy"
        );
        assert!(
            s.error.is_none(),
            "a stale error must not follow into the new generation"
        );
        assert!(
            effects.iter().any(
                |e| matches!(e, Effect::CancelInFlight { generation } if *generation == before)
            )
        );
    }

    /// No `busy` here, unlike the two tests above: this toggle is refused
    /// outright while anything is in flight (see the test below), so
    /// "unsticks a stuck UI" is no longer a property it can have.
    /// `SelectGame` and `ToggleDevelop` do still interrupt, and they are
    /// what keeps that property covered.
    #[test]
    fn toggling_multi_version_cancels_stale_work_and_clears_a_stale_error() {
        let mut s = ready_state();
        s.error = Some(Alert {
            title: "old".into(),
            message: "stale".into(),
            message_is_key: false,
            generation: Some(s.generation),
            title_arg: None,
        });
        let before = s.generation;

        let effects = reduce(&mut s, Msg::ToggleMultiVersion(true));

        assert!(
            s.error.is_none(),
            "a stale error must not follow into the new generation"
        );
        assert!(
            effects.iter().any(
                |e| matches!(e, Effect::CancelInFlight { generation } if *generation == before)
            )
        );
    }

    #[test]
    fn a_reply_from_a_stale_generation_is_discarded() {
        let mut s = ready_state();
        reduce(&mut s, Msg::SelectGame(GameId::OpenLoco));
        let stale = s.generation - 1;

        let effects = reduce(
            &mut s,
            Msg::ReleasesLoaded {
                generation: stale,
                result: Ok(vec![ReleaseRow {
                    tag: "wrong-game".into(),
                    age: None,
                    channel: Channel::Release,
                }]),
            },
        );

        assert!(effects.is_empty());
        assert!(
            !s.releases.iter().any(|b| b.tag == "wrong-game"),
            "a stale reply must not populate the list"
        );
    }

    #[test]
    fn a_reply_from_the_current_generation_is_applied() {
        let mut s = ready_state();
        let generation = s.generation;
        reduce(
            &mut s,
            Msg::ReleasesLoaded {
                generation,
                result: Ok(vec![ReleaseRow {
                    tag: "v9".into(),
                    age: None,
                    channel: Channel::Release,
                }]),
            },
        );
        assert_eq!(s.releases.len(), 1);
        assert_eq!(s.releases[0].tag, "v9");
        assert_eq!(s.selected_release, 0);
    }

    #[test]
    fn installed_loaded_from_a_stale_generation_is_discarded() {
        let mut s = ready_state();
        reduce(&mut s, Msg::SelectGame(GameId::OpenLoco));
        let stale = s.generation - 1;

        let effects = reduce(
            &mut s,
            Msg::InstalledLoaded {
                generation: stale,
                result: Ok((vec![installed("wrong-game", true)], None)),
            },
        );

        assert!(effects.is_empty());
        assert!(
            !s.installed.iter().any(|i| i.tag == "wrong-game"),
            "a stale reply must not populate the list"
        );
    }

    #[test]
    fn installed_loaded_from_the_current_generation_is_applied() {
        let mut s = ready_state();
        let generation = s.generation;
        reduce(
            &mut s,
            Msg::InstalledLoaded {
                generation,
                result: Ok((vec![installed("v9", true)], Some("v9".into()))),
            },
        );
        assert_eq!(s.installed.len(), 1);
        assert_eq!(s.installed[0].tag, "v9");
        assert_eq!(s.active.as_deref(), Some("v9"));
    }

    #[test]
    fn progress_from_a_stale_generation_is_ignored() {
        let mut s = ready_state();
        reduce(&mut s, Msg::SelectGame(GameId::OpenLoco));
        let stale = s.generation - 1;

        reduce(
            &mut s,
            Msg::Progress {
                generation: stale,
                status: Status::Downloading,
                value: Some(0.5),
            },
        );

        assert!(
            s.busy.is_none(),
            "a stale progress report must not resurrect the busy state"
        );
    }

    #[test]
    fn progress_from_the_current_generation_fills_in_what_is_running() {
        let mut s = ready_state();
        reduce(&mut s, Msg::ClickDownload);
        assert_eq!(
            s.busy,
            Some(Busy {
                status: None,
                value: None
            }),
            "initiating is what makes it busy; nothing has reported a phase yet"
        );

        let generation = s.generation;
        reduce(
            &mut s,
            Msg::Progress {
                generation,
                status: Status::Downloading,
                value: Some(0.5),
            },
        );

        assert_eq!(
            s.busy,
            Some(Busy {
                status: Some(Status::Downloading),
                value: Some(0.5)
            })
        );
    }

    /// `Progress` updates a busy state, it never creates one. The main
    /// queue is FIFO so a report after the reply that ended its operation
    /// should not be constructible, but if one ever were, it must not
    /// leave every control disabled with nothing running.
    #[test]
    fn progress_with_nothing_running_does_not_make_the_ui_busy() {
        let mut s = ready_state();
        let generation = s.generation;

        reduce(
            &mut s,
            Msg::Progress {
                generation,
                status: Status::Downloading,
                value: Some(0.5),
            },
        );

        assert!(s.busy.is_none());
    }

    #[test]
    fn toggling_develop_versions_bumps_the_generation_and_refetches() {
        let mut s = ready_state();
        let before = s.generation;
        let effects = reduce(&mut s, Msg::ToggleDevelop(true));
        assert!(s.show_develop);
        assert!(s.generation > before);
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::FetchReleases {
                include_develop: true,
                ..
            }
        )));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SavePref(Pref::ShowDevelop(true))))
        );
    }

    #[test]
    fn toggling_multi_version_emits_a_mode_change_for_every_game() {
        let mut s = ready_state();
        let effects = reduce(&mut s, Msg::ToggleMultiVersion(true));
        assert!(s.multi_version);
        let changes = effects
            .iter()
            .filter(|e| matches!(e, Effect::SetMode { .. }))
            .count();
        assert_eq!(
            changes,
            GameId::ALL.len(),
            "the preference is global, so every game changes"
        );
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SavePref(Pref::MultiVersion(true))))
        );
    }

    #[test]
    fn clicking_remove_asks_for_confirmation_before_deleting() {
        let mut s = ready_state();
        s.multi_version = true;
        s.selected_installed = 1;

        let effects = reduce(&mut s, Msg::ClickRemove);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::ConfirmRemove { name } if name == "v1"))
        );
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::SwitchThenRemove { .. })),
            "not yet"
        );

        let effects = reduce(&mut s, Msg::ConfirmRemove);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SwitchThenRemove { remove, .. } if remove == "v1"))
        );
    }

    /// `installed` is newest first, so the switch target is the first
    /// entry that is not the one being deleted.
    #[test]
    fn removing_a_version_switches_to_the_newest_other_one_first() {
        let mut s = ready_state();
        s.multi_version = true;
        s.installed = vec![
            installed("v3", true),
            installed("v2", true),
            installed("v1", true),
        ];
        s.active = Some("v3".into());
        s.selected_installed = 0; // v3, the active and newest one

        reduce(&mut s, Msg::ClickRemove);
        let effects = reduce(&mut s, Msg::ConfirmRemove);
        assert!(
            effects.iter().any(|e| matches!(
                e,
                Effect::SwitchThenRemove { activate, remove, .. } if activate == "v2" && remove == "v3"
            )),
            "got {effects:?}"
        );
    }

    /// The names are decided at click time and not looked at again: a
    /// reload that lands while the modal is open (`runModal` pumps a nested
    /// run loop, so it really can) must not redirect the deletion.
    #[test]
    fn a_reload_during_the_confirmation_cannot_redirect_the_deletion() {
        let mut s = ready_state();
        s.multi_version = true;
        s.installed = vec![installed("v3", true), installed("v2", true)];
        s.active = Some("v3".into());
        s.selected_installed = 0;

        let effects = reduce(&mut s, Msg::ClickRemove);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::ConfirmRemove { name } if name == "v3"))
        );

        // The modal is open; a `LoadInstalled` reply arrives and reorders
        // everything under it.
        let generation = s.generation;
        reduce(
            &mut s,
            Msg::InstalledLoaded {
                generation,
                result: Ok((
                    vec![installed("v2", true), installed("v3", true)],
                    Some("v2".into()),
                )),
            },
        );
        assert_eq!(
            s.selected_installed, 0,
            "the reload moved the selection to v2"
        );

        let effects = reduce(&mut s, Msg::ConfirmRemove);
        assert!(
            effects.iter().any(|e| matches!(
                e,
                Effect::SwitchThenRemove { activate, remove, .. } if activate == "v2" && remove == "v3"
            )),
            "the version the dialog named is still the one deleted: {effects:?}"
        );
    }

    #[test]
    fn cancelling_the_confirmation_leaves_everything_alone() {
        let mut s = ready_state();
        s.multi_version = true;
        s.selected_installed = 0;

        reduce(&mut s, Msg::ClickRemove);
        // Cancel is simply the absence of `ConfirmRemove`: nothing else is
        // sent, and nothing in `UiState` changed on the way here.
        assert_eq!(s.selected_installed, 0);
        assert_eq!(s.installed.len(), 2);
        assert_eq!(s.active.as_deref(), Some("v2"));
        assert!(s.error.is_none());
        assert!(!s.is_busy());

        // And the cancelled pair cannot be picked up afterwards: a second
        // click that finds Remove disabled disarms it.
        s.installed = vec![installed("v2", true)];
        reduce(&mut s, Msg::ClickRemove);
        assert!(reduce(&mut s, Msg::ConfirmRemove).is_empty());
    }

    #[test]
    fn clicking_remove_does_nothing_when_remove_is_disabled() {
        let mut s = ready_state();
        s.multi_version = true;
        s.installed = vec![installed("v1", true)];
        s.active = Some("v1".into());
        s.selected_installed = 0;
        let effects = reduce(&mut s, Msg::ClickRemove);
        assert!(
            effects.is_empty(),
            "the reducer must not trust the view's idea of enablement"
        );
        assert!(
            reduce(&mut s, Msg::ConfirmRemove).is_empty(),
            "and nothing is left pending for a stray confirmation to pick up"
        );
    }

    /// A switch that failed comes back as `Activated`, and the reducer must
    /// not turn that into a deletion of any kind.
    #[test]
    fn a_failed_switch_produces_no_deletion() {
        let mut s = ready_state();
        s.multi_version = true;
        s.selected_installed = 0;

        reduce(&mut s, Msg::ClickRemove);
        reduce(&mut s, Msg::ConfirmRemove);

        let generation = s.generation;
        let effects = reduce(
            &mut s,
            Msg::Activated {
                generation,
                result: Err("read-only volume".into()),
            },
        );
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::SwitchThenRemove { .. }))
        );
        assert_eq!(
            s.error.as_ref().map(|a| a.title.as_str()),
            Some("FailedToSwitchVersion"),
            "the user sees why the switch failed, not a removal error"
        );
        assert!(
            effects.is_empty(),
            "and nothing is reloaded, because nothing changed on disk"
        );
    }

    /// The other half of that: a removal that failed *after* a successful
    /// switch has already moved `bin`, so the list has to be reread even
    /// though the operation errored.
    #[test]
    fn a_failed_removal_still_reloads_because_the_switch_already_happened() {
        let mut s = ready_state();
        s.multi_version = true;
        let generation = s.generation;
        let effects = reduce(
            &mut s,
            Msg::Removed {
                generation,
                result: Err("directory not empty".into()),
            },
        );
        assert_eq!(
            s.error.as_ref().map(|a| a.title.as_str()),
            Some("FailedToRemoveVersion")
        );
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::LoadInstalled { .. }))
        );
    }

    #[test]
    fn clicking_play_launches_the_selected_installed_directory() {
        let mut s = ready_state();
        let effects = reduce(&mut s, Msg::ClickPlay);
        assert!(effects.iter().any(|e| matches!(
            e, Effect::Launch { dir, .. } if dir == std::path::Path::new("/tmp/v2")
        )));
    }

    #[test]
    fn clicking_play_does_nothing_when_play_is_disabled() {
        let mut s = ready_state();
        s.installed[0].can_launch = false; // the selected (index 0) build is broken
        let effects = reduce(&mut s, Msg::ClickPlay);
        assert!(
            effects.is_empty(),
            "the reducer must not trust the view's idea of enablement"
        );
    }

    #[test]
    fn clicking_download_installs_the_selected_release() {
        let mut s = ready_state();
        let effects = reduce(&mut s, Msg::ClickDownload);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::Install { tag, .. } if tag == "v3"))
        );
    }

    #[test]
    fn clicking_download_does_nothing_when_download_is_disabled() {
        let mut s = ready_state();
        s.set_auto_update(s.selected_game, true);
        let effects = reduce(&mut s, Msg::ClickDownload);
        assert!(
            effects.is_empty(),
            "the reducer must not trust the view's idea of enablement"
        );
    }

    #[test]
    fn an_error_is_surfaced_and_clears_the_busy_state() {
        let mut s = ready_state();
        let generation = s.generation;
        s.busy = Some(Busy {
            status: Some(Status::Downloading),
            value: Some(0.5),
        });

        reduce(
            &mut s,
            Msg::InstallFinished {
                generation,
                result: Err("the server hung up".into()),
            },
        );

        assert!(
            s.busy.is_none(),
            "the UI must not stay stuck busy after a failure"
        );
        let alert = s.error.as_ref().expect("an error should be shown");
        assert!(alert.message.contains("the server hung up"));
    }

    #[test]
    fn a_successful_install_clears_any_previous_error() {
        let mut s = ready_state();
        let generation = s.generation;
        s.error = Some(Alert {
            title: "old".into(),
            message: "stale".into(),
            message_is_key: false,
            generation: Some(generation),
            title_arg: None,
        });

        reduce(
            &mut s,
            Msg::InstallFinished {
                generation,
                result: Ok(()),
            },
        );

        assert!(s.error.is_none());
        assert!(s.busy.is_none());
    }

    #[test]
    fn a_generation_bump_clears_a_fetch_or_install_error() {
        let mut s = ready_state();
        let generation = s.generation;
        s.error = Some(Alert {
            title: "old".into(),
            message: "stale".into(),
            message_is_key: false,
            generation: Some(generation),
            title_arg: None,
        });

        reduce(&mut s, Msg::SelectGame(GameId::OpenLoco));

        assert!(
            s.error.is_none(),
            "an error tied to the old generation's work must not follow into the new one"
        );
    }

    #[test]
    fn a_generation_bump_does_not_clear_a_launch_error() {
        let mut s = ready_state();
        s.error = Some(Alert {
            title: "FailedToLaunchGame".into(),
            message: "boom".into(),
            message_is_key: false,
            generation: None,
            title_arg: None,
        });

        reduce(&mut s, Msg::SelectGame(GameId::OpenLoco));

        assert!(
            s.error.is_some(),
            "a launch error has nothing to do with switching games and must survive a generation bump"
        );
    }

    #[test]
    fn a_failed_launch_is_surfaced_as_an_error() {
        let mut s = ready_state();

        reduce(
            &mut s,
            Msg::LaunchFinished {
                result: Err("no such file".into()),
            },
        );

        let alert = s.error.as_ref().expect("a launch failure should be shown");
        assert!(alert.message.contains("no such file"));
        assert_eq!(
            alert.generation, None,
            "a launch error is not tied to any generation"
        );
    }

    #[test]
    fn a_failed_launch_carries_the_selected_games_name_as_its_title_argument() {
        // `FailedToLaunchGame` is "Failed to launch {0}"; the reducer must
        // hand over the game name as data for `strings::format1` to
        // substitute, not a formatted string, since `state.rs` stays free
        // of `strings.rs`.
        let mut s = ready_state();
        s.selected_game = GameId::OpenLoco;

        reduce(
            &mut s,
            Msg::LaunchFinished {
                result: Err("no such file".into()),
            },
        );

        let alert = s.error.as_ref().expect("a launch failure should be shown");
        assert_eq!(alert.title, "FailedToLaunchGame");
        assert_eq!(
            alert.title_arg.as_deref(),
            Some(GameId::OpenLoco.display_name())
        );
    }

    #[test]
    fn a_successful_launch_clears_a_previous_launch_error() {
        let mut s = ready_state();
        s.error = Some(Alert {
            title: "FailedToLaunchGame".into(),
            message: "boom".into(),
            message_is_key: false,
            generation: None,
            title_arg: None,
        });

        reduce(&mut s, Msg::LaunchFinished { result: Ok(()) });

        assert!(
            s.error.is_none(),
            "a launch error must be able to clear on the next successful launch"
        );
    }

    // --- Minor 3: a startup repair failure is surfaced, not swallowed ----

    #[test]
    fn a_startup_repair_failure_is_surfaced_as_a_generation_scoped_error() {
        let mut s = ready_state();

        let effects = reduce(
            &mut s,
            Msg::StartupRepairFailed {
                message: "OpenRCT2: .bin.incoming exists".into(),
            },
        );

        assert!(effects.is_empty(), "surfacing an error starts no work");
        let alert = s.error.as_ref().expect("the failure must reach the user");
        assert_eq!(alert.title, "FailedToRepairInstallation");
        assert!(alert.message.contains(".bin.incoming exists"));
        assert_eq!(
            alert.generation,
            Some(s.generation),
            "scoped to a generation, so a game switch clears it like any other store error"
        );
    }

    /// The reason it is not `generation: None`. That class belongs to
    /// `LaunchFinished`, whose success arm clears every error in it, so a
    /// repair failure filed there would be dismissed by an unrelated
    /// successful launch. Each event clears exactly the class it owns.
    #[test]
    fn a_successful_launch_does_not_clear_a_startup_repair_failure() {
        let mut s = ready_state();
        reduce(
            &mut s,
            Msg::StartupRepairFailed {
                message: "unrepaired".into(),
            },
        );

        reduce(&mut s, Msg::LaunchFinished { result: Ok(()) });

        assert!(
            s.error
                .as_ref()
                .is_some_and(|a| a.title == "FailedToRepairInstallation"),
            "a launch succeeding says nothing about whether the store was repaired"
        );
    }

    /// And the reload the initial `SelectGame` starts must not wipe it
    /// either, which is what deferred item 18 already relies on: the `Ok`
    /// arms of the two load replies do not clear `error`.
    #[test]
    fn a_reload_does_not_clear_a_startup_repair_failure() {
        let mut s = ready_state();
        reduce(
            &mut s,
            Msg::StartupRepairFailed {
                message: "unrepaired".into(),
            },
        );
        let generation = s.generation;

        reduce(
            &mut s,
            Msg::InstalledLoaded {
                generation,
                result: Ok((vec![installed("v1", true)], Some("v1".into()))),
            },
        );
        reduce(
            &mut s,
            Msg::ReleasesLoaded {
                generation,
                result: Ok(vec![ReleaseRow {
                    tag: "v1".into(),
                    age: None,
                    channel: Channel::Release,
                }]),
            },
        );

        assert!(
            s.error
                .as_ref()
                .is_some_and(|a| a.title == "FailedToRepairInstallation"),
            "the banner has to outlive the reload that startup itself began"
        );
    }

    #[test]
    fn a_successful_launch_does_not_clear_an_unrelated_generation_scoped_error() {
        let mut s = ready_state();
        let generation = s.generation;
        s.error = Some(Alert {
            title: "old".into(),
            message: "stale fetch".into(),
            message_is_key: false,
            generation: Some(generation),
            title_arg: None,
        });

        reduce(&mut s, Msg::LaunchFinished { result: Ok(()) });

        assert!(
            s.error.is_some(),
            "a successful launch has nothing to do with a fetch or install error and must leave it alone"
        );
    }

    #[test]
    fn auto_update_installs_the_newest_build_when_builds_arrive() {
        let mut s = ready_state();
        s.set_auto_update(s.selected_game, true);
        s.installed.clear();
        s.active = None;
        let generation = s.generation;

        let effects = reduce(
            &mut s,
            Msg::ReleasesLoaded {
                generation,
                result: Ok(vec![ReleaseRow {
                    tag: "v3".into(),
                    age: None,
                    channel: Channel::Release,
                }]),
            },
        );

        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::Install { tag, .. } if tag == "v3"))
        );
    }

    #[test]
    fn auto_update_does_not_reinstall_what_is_already_active_in_compatible_mode() {
        // `ready_state` leaves `multi_version` false, i.e. compatible mode,
        // where `VersionStore::active` reports a *tag* and the sole
        // installed entry's `name` is always the literal string "bin" --
        // never equal to its tag. `installed_named` is what lets this test
        // say so, rather than `installed`'s `name == tag` shortcut, which
        // would hide exactly this bug: comparing `active` against `name`
        // here never matches, `newest_installable` concludes nothing is
        // installed, and auto-update re-downloads a build that is already
        // on disk and already running, over the user's live installation.
        let mut s = ready_state();
        s.set_auto_update(s.selected_game, true);
        s.active = Some("v3".into()); // compatible mode: `active` is the tag
        s.installed = vec![installed_named("v3", "bin", true)];
        let generation = s.generation;

        let effects = reduce(
            &mut s,
            Msg::ReleasesLoaded {
                generation,
                result: Ok(vec![ReleaseRow {
                    tag: "v3".into(),
                    age: None,
                    channel: Channel::Release,
                }]),
            },
        );

        assert!(
            !effects.iter().any(|e| matches!(e, Effect::Install { .. })),
            "must not re-download a build that is already installed and active"
        );
    }

    // --- C1: the auto-update preference is per game -----------------------

    /// The one-click failure this whole field shape exists to prevent.
    /// Auto-update is on for OpenRCT2 and off for OpenLoco; clicking
    /// OpenLoco in the sidebar must not install an OpenLoco build, which in
    /// compatible mode would replace `bin` and delete whatever the user was
    /// deliberately keeping.
    #[test]
    fn switching_to_a_game_with_auto_update_off_installs_nothing() {
        let mut s = ready_state();
        s.set_auto_update(GameId::OpenRCT2, true);
        s.set_auto_update(GameId::OpenLoco, false);

        reduce(&mut s, Msg::SelectGame(GameId::OpenLoco));
        let generation = s.generation;
        let effects = reduce(
            &mut s,
            Msg::ReleasesLoaded {
                generation,
                result: Ok(vec![ReleaseRow {
                    tag: "loco-1".into(),
                    age: None,
                    channel: Channel::Release,
                }]),
            },
        );

        assert!(
            !effects.iter().any(|e| matches!(e, Effect::Install { .. })),
            "the previous game's setting must not install a build for this one: {effects:?}"
        );
        assert!(!s.auto_update(), "and the checkbox shows this game's value");
        assert!(
            s.download_enabled(),
            "so the controls this game owns are live"
        );
        assert!(s.popup_enabled());
    }

    /// The mirror case, which is the silent one: a game whose stored value
    /// is on must still auto-update after a switch, rather than inheriting
    /// the previous game's off.
    #[test]
    fn switching_to_a_game_with_auto_update_on_still_installs() {
        let mut s = ready_state();
        s.set_auto_update(GameId::OpenRCT2, false);
        s.set_auto_update(GameId::OpenLoco, true);

        reduce(&mut s, Msg::SelectGame(GameId::OpenLoco));
        let generation = s.generation;
        let effects = reduce(
            &mut s,
            Msg::ReleasesLoaded {
                generation,
                result: Ok(vec![ReleaseRow {
                    tag: "loco-1".into(),
                    age: None,
                    channel: Channel::Release,
                }]),
            },
        );

        assert!(
            effects.iter().any(|e| matches!(
                e, Effect::Install { tag, game, .. } if tag == "loco-1" && *game == GameId::OpenLoco
            )),
            "got {effects:?}"
        );
    }

    /// The write-back half. `controller::perform` saves `Pref::AutoUpdate`
    /// under `state.selected_game`, so the slot the reducer writes has to be
    /// that same game's, or the in-memory value and the stored key diverge
    /// on the very next switch.
    #[test]
    fn toggling_auto_update_writes_only_the_selected_games_slot() {
        let mut s = ready_state();
        s.set_auto_update(GameId::OpenRCT2, true);
        reduce(&mut s, Msg::SelectGame(GameId::OpenLoco));

        let effects = reduce(&mut s, Msg::ToggleAutoUpdate(true));

        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SavePref(Pref::AutoUpdate(true))))
        );
        assert_eq!(
            s.selected_game,
            GameId::OpenLoco,
            "which is the game perform saves under"
        );
        assert!(s.auto_update());

        reduce(&mut s, Msg::SelectGame(GameId::OpenRCT2));
        assert!(
            s.auto_update(),
            "the other game's stored value is untouched"
        );

        reduce(&mut s, Msg::ToggleAutoUpdate(false));
        assert!(!s.auto_update());
        reduce(&mut s, Msg::SelectGame(GameId::OpenLoco));
        assert!(
            s.auto_update(),
            "and turning one off did not turn the other off"
        );
    }

    // --- I1: busy is set when an operation is initiated -------------------

    /// Every long-running operation, at the moment it is issued. Until this
    /// held, `busy` arrived only with the first progress report -- seconds
    /// into an install, and never at all for the three that report none --
    /// so every `!is_busy()` guard stood open for exactly as long as the
    /// work it was meant to exclude.
    #[test]
    fn initiating_an_operation_is_what_makes_the_ui_busy() {
        let mut s = ready_state();
        let effects = reduce(&mut s, Msg::ClickDownload);
        assert!(effects.iter().any(|e| matches!(e, Effect::Install { .. })));
        assert!(s.is_busy(), "install");

        let mut s = ready_state();
        s.multi_version = true;
        let effects = reduce(&mut s, Msg::SelectInstalled(1));
        assert!(effects.iter().any(|e| matches!(e, Effect::Activate { .. })));
        assert!(s.is_busy(), "activate");

        let mut s = ready_state();
        s.multi_version = true;
        s.selected_installed = 1;
        reduce(&mut s, Msg::ClickRemove);
        assert!(!s.is_busy(), "the confirmation dialog is not itself work");
        let effects = reduce(&mut s, Msg::ConfirmRemove);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SwitchThenRemove { .. }))
        );
        assert!(s.is_busy(), "switch-then-remove");

        let mut s = ready_state();
        let effects = reduce(&mut s, Msg::ToggleMultiVersion(true));
        assert!(effects.iter().any(|e| matches!(e, Effect::SetMode { .. })));
        assert!(s.is_busy(), "set mode");

        let mut s = ready_state();
        s.set_auto_update(s.selected_game, true);
        s.installed.clear();
        s.active = None;
        let generation = s.generation;
        let effects = reduce(
            &mut s,
            Msg::ReleasesLoaded {
                generation,
                result: Ok(vec![ReleaseRow {
                    tag: "v3".into(),
                    age: None,
                    channel: Channel::Release,
                }]),
            },
        );
        assert!(effects.iter().any(|e| matches!(e, Effect::Install { .. })));
        assert!(s.is_busy(), "auto-update install");
    }

    /// The other half of the obligation, and the one that has already been
    /// got wrong on this branch: everything that sets `busy` must clear it,
    /// on failure as well as success. A missed clear leaves the app inert
    /// with every control disabled.
    #[test]
    fn every_operation_clears_busy_on_success_and_on_failure() {
        for result in [Ok(()), Err("boom".to_string())] {
            let mut s = ready_state();
            reduce(&mut s, Msg::ClickDownload);
            let generation = s.generation;
            reduce(
                &mut s,
                Msg::InstallFinished {
                    generation,
                    result: result.clone(),
                },
            );
            assert!(!s.is_busy(), "install, {result:?}");

            let mut s = ready_state();
            s.multi_version = true;
            reduce(&mut s, Msg::SelectInstalled(1));
            let generation = s.generation;
            reduce(
                &mut s,
                Msg::Activated {
                    generation,
                    result: result.clone(),
                },
            );
            assert!(!s.is_busy(), "activate, {result:?}");

            // Both phases of the two-phase effect: a failure in the switch
            // reports as `Activated`, a failure in the delete as `Removed`,
            // and either one ends the operation.
            for phase in ["activated", "removed"] {
                let mut s = ready_state();
                s.multi_version = true;
                s.selected_installed = 1;
                reduce(&mut s, Msg::ClickRemove);
                reduce(&mut s, Msg::ConfirmRemove);
                assert!(s.is_busy(), "busy must span both phases");
                let generation = s.generation;
                let msg = if phase == "activated" {
                    Msg::Activated {
                        generation,
                        result: result.clone(),
                    }
                } else {
                    Msg::Removed {
                        generation,
                        result: result.clone(),
                    }
                };
                reduce(&mut s, msg);
                assert!(!s.is_busy(), "switch-then-remove {phase}, {result:?}");
            }

            let mut s = ready_state();
            reduce(&mut s, Msg::ToggleMultiVersion(true));
            let generation = s.generation;
            for _ in GameId::ALL {
                reduce(
                    &mut s,
                    Msg::ModeChanged {
                        generation,
                        result: result.clone().map(|()| None),
                    },
                );
            }
            assert!(!s.is_busy(), "set mode, {result:?}");
        }
    }

    /// One toggle issues one store operation per game, so `busy` has to
    /// outlast the first reply. Clearing on it would re-open every guard
    /// while the second store is still mid-rename, which is the state the
    /// review traced to a store left with no `bin` at all.
    #[test]
    fn a_mode_change_stays_busy_until_every_games_store_has_answered() {
        let mut s = ready_state();
        let effects = reduce(&mut s, Msg::ToggleMultiVersion(true));
        assert_eq!(
            effects
                .iter()
                .filter(|e| matches!(e, Effect::SetMode { .. }))
                .count(),
            GameId::ALL.len()
        );

        let generation = s.generation;
        reduce(
            &mut s,
            Msg::ModeChanged {
                generation,
                result: Ok(None),
            },
        );
        assert!(
            s.is_busy(),
            "one game answered; the other is still renaming"
        );

        reduce(
            &mut s,
            Msg::ModeChanged {
                generation,
                result: Ok(None),
            },
        );
        assert!(!s.is_busy());
    }

    /// The double-click the review traced to its end: nothing can cancel a
    /// rename already under way, so a second mode change must not be
    /// startable until the first has finished.
    #[test]
    fn a_second_mode_change_cannot_be_started_while_the_first_is_running() {
        let mut s = ready_state();
        reduce(&mut s, Msg::ToggleMultiVersion(true));
        assert!(s.is_busy());

        let effects = reduce(&mut s, Msg::ToggleMultiVersion(false));

        assert!(effects.is_empty(), "got {effects:?}");
        assert!(
            s.multi_version,
            "and the refused value was not applied to the state"
        );
    }

    /// The window I1(b) describes: `Download` used to leave every guard
    /// open until the first progress report, several seconds in, so a click
    /// on Remove in between issued a deletion against a store an install
    /// was already writing.
    #[test]
    fn remove_cannot_be_started_in_the_gap_before_a_download_reports_progress() {
        let mut s = ready_state();
        s.multi_version = true;
        s.selected_installed = 1;
        assert!(s.remove_enabled());

        reduce(&mut s, Msg::ClickDownload);

        assert!(
            !s.remove_enabled(),
            "no progress has arrived yet, but work has started"
        );
        assert!(reduce(&mut s, Msg::ClickRemove).is_empty());
        assert!(
            reduce(&mut s, Msg::ConfirmRemove).is_empty(),
            "and nothing is left armed"
        );
    }

    /// `NSAlert::runModal` pumps a nested run loop, so the world can change
    /// between the click that opened the dialog and the answer that closes
    /// it. The confirmation re-checks rather than trusting the click.
    #[test]
    fn a_confirmation_answered_after_work_has_started_is_refused() {
        let mut s = ready_state();
        s.multi_version = true;
        s.selected_installed = 1;
        reduce(&mut s, Msg::ClickRemove);

        // Delivered while the modal is open.
        let generation = s.generation;
        reduce(
            &mut s,
            Msg::Progress {
                generation,
                status: Status::Downloading,
                value: Some(0.1),
            },
        );
        s.busy = Some(Busy {
            status: Some(Status::Downloading),
            value: Some(0.1),
        });

        let effects = reduce(&mut s, Msg::ConfirmRemove);
        assert!(effects.is_empty(), "got {effects:?}");
        assert!(
            reduce(&mut s, Msg::ConfirmRemove).is_empty(),
            "and the refused pair is not left armed for a later confirmation"
        );
    }

    /// Minor B. The refusal above is correct, but it used to be *silent*:
    /// the dialog closed, the build stayed, and nothing said why. This is
    /// the exact route there, built out of messages rather than by assigning
    /// `busy` by hand, because the route is the finding -- `remove_enabled()`
    /// does not consult `auto_update`, so Remove is clickable with
    /// auto-update on, and the `ReleasesLoaded` that arrives through the
    /// modal's nested run loop then starts an install of its own.
    #[test]
    fn a_confirmation_refused_because_work_started_says_so() {
        let mut s = ready_state();
        s.multi_version = true;
        s.selected_installed = 1;
        s.set_auto_update(s.selected_game, true);
        assert!(s.remove_enabled(), "Remove does not depend on auto-update");

        reduce(&mut s, Msg::ClickRemove);

        // Delivered while the dialog is on screen. With auto-update on this
        // is what takes the store away from the removal.
        let generation = s.generation;
        let releases = vec![
            ReleaseRow {
                tag: "v3".into(),
                age: None,
                channel: Channel::Release,
            },
            ReleaseRow {
                tag: "v2".into(),
                age: None,
                channel: Channel::Release,
            },
        ];
        let effects = reduce(
            &mut s,
            Msg::ReleasesLoaded {
                generation,
                result: Ok(releases),
            },
        );
        assert!(
            effects.iter().any(|e| matches!(e, Effect::Install { .. })),
            "the probe has to actually reach the busy state: {effects:?}"
        );
        assert!(s.is_busy());

        let effects = reduce(&mut s, Msg::ConfirmRemove);
        assert!(effects.is_empty(), "nothing may be deleted: {effects:?}");
        let alert = s
            .error
            .as_ref()
            .expect("a refused removal must say something");
        assert_eq!(alert.title, "FailedToRemoveVersion");
        assert_eq!(alert.message, "RemoveBusyMessage");
        assert!(
            alert.message_is_key,
            "the reducer has no reported text, so it names a key"
        );
        assert_eq!(
            alert.generation,
            Some(s.generation),
            "scoped to this generation's work, so a game switch dismisses it"
        );
    }

    /// The other half of the arm: a `ConfirmRemove` with nothing armed is
    /// not a refusal and must stay silent. Otherwise the banner would
    /// appear for a confirmation the user never gave.
    #[test]
    fn a_confirmation_with_nothing_armed_reports_nothing() {
        let mut s = ready_state();
        s.multi_version = true;
        s.busy = Some(Busy {
            status: Some(Status::Downloading),
            value: Some(0.5),
        });

        assert!(reduce(&mut s, Msg::ConfirmRemove).is_empty());
        assert!(s.error.is_none(), "got {:?}", s.error);
    }

    /// Selecting is switching, so it is refused outright while something is
    /// running: moving the selection without activating would leave the
    /// popup naming a build that is not the one in use.
    #[test]
    fn selecting_another_installed_version_is_refused_while_busy() {
        let mut s = ready_state();
        s.multi_version = true;
        reduce(&mut s, Msg::ClickDownload);

        let effects = reduce(&mut s, Msg::SelectInstalled(1));

        assert!(effects.is_empty(), "got {effects:?}");
        assert_eq!(s.selected_installed, 0, "and the selection did not move");
    }

    /// The hazard that made setting `busy` at initiation dangerous the
    /// first time it was tried: a bump discards the reply that would have
    /// cleared it, so the bump has to clear it itself, unconditionally.
    #[test]
    fn a_generation_bump_clears_busy_that_was_set_at_initiation() {
        for interrupt in [Msg::SelectGame(GameId::OpenLoco), Msg::ToggleDevelop(true)] {
            let mut s = ready_state();
            reduce(&mut s, Msg::ClickDownload);
            assert!(s.is_busy());
            let stale = s.generation;

            reduce(&mut s, interrupt);
            assert!(s.busy.is_none(), "a bump must not leave the UI stuck busy");

            // And the discarded reply does not resurrect or unbalance it.
            reduce(
                &mut s,
                Msg::InstallFinished {
                    generation: stale,
                    result: Ok(()),
                },
            );
            assert!(s.busy.is_none());

            // `SelectGame` clears the release list, so give the new
            // generation something to install before asking again.
            let generation = s.generation;
            reduce(
                &mut s,
                Msg::ReleasesLoaded {
                    generation,
                    result: Ok(vec![ReleaseRow {
                        tag: "v9".into(),
                        age: None,
                        channel: Channel::Release,
                    }]),
                },
            );
            reduce(&mut s, Msg::ClickDownload);
            assert!(s.is_busy(), "the next operation still works afterwards");
            reduce(
                &mut s,
                Msg::InstallFinished {
                    generation,
                    result: Ok(()),
                },
            );
            assert!(!s.is_busy(), "and still clears");
        }
    }

    #[test]
    fn auto_update_does_not_reinstall_what_is_already_active_in_multi_version_mode() {
        // The mirror image of the compatible-mode test above: here `active`
        // is a *name*, and a collision walk can make it differ from the
        // entry's `tag` (the exact shape `adopt_dir`'s free-name walk
        // produces). Comparing by tag instead of name would wrongly
        // conclude this entry is not the active one.
        let mut s = ready_state();
        s.set_auto_update(s.selected_game, true);
        s.multi_version = true;
        s.active = Some("v3-2".into()); // multi-version mode: `active` is the name
        s.installed = vec![installed_named("v3", "v3-2", true)];
        let generation = s.generation;

        let effects = reduce(
            &mut s,
            Msg::ReleasesLoaded {
                generation,
                result: Ok(vec![ReleaseRow {
                    tag: "v3".into(),
                    age: None,
                    channel: Channel::Release,
                }]),
            },
        );

        assert!(
            !effects.iter().any(|e| matches!(e, Effect::Install { .. })),
            "must not re-download a build that is already installed and active, even when a \
             name/tag collision means its on-disk name differs from its tag"
        );
    }
}
