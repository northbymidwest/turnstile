//! The interface's behavior, with no AppKit anywhere near it. `UiState` holds
//! what is on screen; `reduce` is the only thing allowed to change it, and it
//! returns `Effect`s for something else to carry out and report back as
//! another `Msg`. Control enablement is always derived from state, never a
//! separately-assigned flag.
//!
//! Every effect that resolves a directory is tagged with the `generation`
//! current when it was issued, and any reply carrying a stale generation is
//! discarded.

use std::collections::HashMap;
use std::path::PathBuf;

use turnstile_core::age::Age;
use turnstile_core::download::Status;
use turnstile_core::game::GameId;
use turnstile_core::gamedata::OriginalGame;
use turnstile_core::release::Channel;
use turnstile_core::store::{DisableReport, Installed};

/// One row of the available-releases popup, already formatted for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseRow {
    pub tag: String,
    pub age: Option<Age>,
    pub channel: Channel,
}

/// Something is running: present from the moment an operation is *initiated*
/// until its last reply lands, not from its first progress report.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Busy {
    /// What the worker last said it was doing. `None` before the first
    /// `Msg::Progress`, and for the whole of an operation that reports none.
    pub status: Option<Status>,
    pub value: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub title: String,
    pub message: String,
    /// `true` when `message` names a localization key rather than holding text
    /// to display verbatim. `state.rs` must stay free of `strings.rs`, so the
    /// choice is made here and `views::apply` does the lookup.
    pub message_is_key: bool,
    /// The generation this error belongs to, so `bump_generation` can clear it
    /// along with everything else that generation's work produced. `None` for
    /// errors raised by work with no generation (`LaunchFinished` alone).
    pub generation: Option<u64>,
    /// The argument for `title`'s `{0}`, as data rather than formatted text:
    /// `apply` substitutes it via `strings::format1`. Only `FailedToLaunchGame`
    /// has a placeholder.
    pub title_arg: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pref {
    CheckForUpdates(bool),
    /// Where the original games' data is unpacked. `None` is the default root,
    /// which has to survive a restart as an answer distinct from a path that
    /// happens to equal today's default.
    InstallRoot(Option<String>),
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
    /// Whatever `VersionStore::active` reported: an `Installed::name` in
    /// multi-version mode, an `Installed::tag` in compatible mode. Nothing
    /// outside `active_entry` may compare it against either field directly.
    pub active: Option<String>,
    pub releases: Vec<ReleaseRow>,
    /// Releases already fetched, keyed on both things that decide what a fetch
    /// returns: the game, and whether development builds were included, since
    /// that changes which repositories are queried rather than merely filtering
    /// the result. Never invalidated; quitting and reopening refetches.
    release_cache: HashMap<(GameId, bool), Vec<ReleaseRow>>,
    pub selected_release: usize,
    pub show_develop: bool,
    pub check_for_updates: bool,
    /// Where each original game's data was found, one slot per `OriginalGame`.
    game_data: [Option<PathBuf>; 3],
    /// The chosen root for unpacking game data, or `None` for the default.
    pub install_root: Option<PathBuf>,
    /// The newer release, once one has been found and not yet dismissed.
    pub update: Option<turnstile_core::selfupdate::Update>,
    /// Per game, mirroring the `autoInstallUpdates.<gameKey>` preference. Read
    /// only through `auto_update()`, which resolves it against
    /// `selected_game`, so no call site can pick the wrong game's slot.
    auto_update: [bool; GameId::ALL.len()],
    pub multi_version: bool,
    pub busy: Option<Busy>,
    /// Initiated operations not yet reported back, from which `busy` is
    /// derived. A count rather than a flag because `Msg::ToggleMultiVersion`
    /// issues one `SetMode` per game. `bump_generation` zeroes it
    /// unconditionally, so a lost reply cannot strand the UI busy.
    outstanding: u32,
    pub error: Option<Alert>,
    pub generation: u64,
    pending_removal: Option<PendingRemoval>,
}

/// What `ClickRemove` decided, held until the modal answers. Both names are
/// captured at click time: `NSAlert::runModal` pumps a nested run loop, so an
/// `InstalledLoaded` already in flight can reshuffle `state.installed` while
/// the dialog is open.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingRemoval {
    activate: String,
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
            // Somebody running an old build with a fixed bug in it is the
            // case this exists for, and they will not go looking for it.
            check_for_updates: true,
            update: None,
            auto_update: [false; GameId::ALL.len()],
            multi_version: false,
            busy: None,
            outstanding: 0,
            error: None,
            generation: 0,
            pending_removal: None,
            game_data: [None, None, None],
            install_root: None,
        }
    }

    /// Where this game's data is, if it is installed at all.
    #[must_use]
    pub fn game_data(&self, game: OriginalGame) -> Option<&std::path::Path> {
        self.game_data[Self::data_slot(game)].as_deref()
    }

    /// `OriginalGame` has no index of its own, and giving it one would put a
    /// second ordering next to `ALL`'s that could drift from it.
    fn data_slot(game: OriginalGame) -> usize {
        OriginalGame::ALL
            .iter()
            .position(|candidate| *candidate == game)
            .unwrap_or(0)
    }

    pub fn is_busy(&self) -> bool {
        self.busy.is_some()
    }

    pub fn auto_update(&self) -> bool {
        self.auto_update[self.selected_game.index()]
    }

    pub fn set_auto_update(&mut self, game: GameId, on: bool) {
        self.auto_update[game.index()] = on;
    }

    /// The on-disk identity of the current selection, never the display tag.
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
    /// for whichever mode is current. The only place allowed to compare
    /// `self.active` against `Installed::name` or `Installed::tag`.
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
    /// active version. It is made removable by switching away from it first
    /// (see `switch_target` and `Effect::SwitchThenRemove`), which is why two
    /// installed versions are the requirement: with one there is nothing to
    /// switch to.
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
    /// The answer to `Effect::CheckForUpdate`. `Ok(None)` means up to date.
    /// There is no error case: a failed check produces nothing, because an
    /// update notice that can raise an error is worse than one that
    /// occasionally fails to notice.
    UpdateChecked(Option<turnstile_core::selfupdate::Update>),
    DismissUpdate,
    OpenUpdatePage,
    ToggleCheckForUpdates(bool),
    ShowSettings,
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
    /// Where each original game's data is, as found on disk.
    GameDataScanned(Vec<(OriginalGame, Option<PathBuf>)>),
    ChooseInstaller(OriginalGame),
    /// `None` is a cancelled picker, which is not a failure.
    InstallerChosen {
        game: OriginalGame,
        installer: Option<PathBuf>,
    },
    GameDataInstalled {
        generation: u64,
        game: OriginalGame,
        result: Result<PathBuf, String>,
    },
    ChooseInstallRoot,
    InstallRootChosen(Option<PathBuf>),
    /// `VersionStore::reconcile` failed for at least one game at launch.
    /// Posted by `controller::start` after the first `SelectGame`, since the
    /// repair runs before any generation exists.
    StartupRepairFailed {
        message: String,
    },
}

#[derive(Debug)]
pub enum Effect {
    /// Ask whether a newer Turnstile has been published. No generation, unlike
    /// every other network effect: this one is about the application, and
    /// nothing the user does while it is in flight can make the answer wrong.
    CheckForUpdate,
    ShowSettings,
    /// Open a URL in the browser. The update banner is the only caller:
    /// replacing a running, notarized bundle in place is a different feature.
    OpenUrl(String),
    LoadInstalled {
        generation: u64,
        game: GameId,
    },
    /// Look on disk for each original game's data.
    ScanGameData,
    /// The reply is `Msg::InstallerChosen`, including when cancelled.
    PickInstaller(OriginalGame),
    PickInstallRoot,
    /// Unpack `installer` and point the game at the result. The destination is
    /// resolved where the effect is performed, since the reducer has no
    /// filesystem and the default root is not a fact it holds.
    InstallGameData {
        generation: u64,
        game: OriginalGame,
        installer: PathBuf,
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
    /// `name` is `Installed::name`, the on-disk identity, never the display
    /// tag: `VersionStore::activate` takes exactly this, by contract.
    Activate {
        generation: u64,
        game: GameId,
        name: String,
    },
    /// Switch to `activate`, then -- only if that switched for real -- delete
    /// `remove`. One effect rather than two because the ordering is the whole
    /// point: `VersionStore::remove` refuses to delete the active version, and
    /// splitting this would put a main-loop turn between the halves, during
    /// which another message could pair the wrong two together.
    ///
    /// Both names carry the same identity contract as `Activate`.
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
    /// `generation` is the one being abandoned, not the new one.
    ///
    /// The executor holds one cancel flag per in-flight install and sets
    /// whichever is currently stored, so it does not branch on `generation`;
    /// only this module's tests read the field.
    CancelInFlight {
        #[allow(dead_code)]
        generation: u64,
    },
}

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
            // Through `adopt_releases` rather than by assigning, so a cache hit
            // still evaluates auto-update.
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
            // moving the selection and declining only the activation: the popup
            // always shows the active version, so a selection that moved
            // without activating would be a lie about what is running.
            if state.is_busy() {
                return Vec::new();
            }
            state.selected_installed = index;
            let Some(name) = state.selected_installed_name().map(str::to_string) else {
                return Vec::new();
            };
            // Selecting *is* switching, but only where switching exists. The
            // comparison routes through `active_entry` rather than reading
            // `state.active`, which is a fact a reader would otherwise have to
            // re-derive per site.
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

        Msg::UpdateChecked(found) => {
            // Only when the check is still wanted: the setting can be turned
            // off while the request is in flight.
            if state.check_for_updates {
                state.update = found;
            }
            Vec::new()
        }

        Msg::DismissUpdate => {
            state.update = None;
            Vec::new()
        }

        Msg::OpenUpdatePage => {
            // Taken before clearing, and the banner goes either way: somebody
            // who has opened the page has been told.
            let effects = match &state.update {
                Some(update) => vec![Effect::OpenUrl(update.url.clone())],
                None => Vec::new(),
            };
            state.update = None;
            effects
        }

        Msg::ShowSettings => vec![Effect::ShowSettings],

        Msg::ToggleCheckForUpdates(on) => {
            state.check_for_updates = on;
            // Turning it off takes down a banner already showing.
            if !on {
                state.update = None;
            }
            vec![Effect::SavePref(Pref::CheckForUpdates(on))]
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
            // filtering what the first returned.
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
            state.set_auto_update(state.selected_game, on);
            let mut effects = vec![Effect::SavePref(Pref::AutoUpdate(on))];
            // The preference is saved either way; only the install it would
            // trigger waits. The next `ReleasesLoaded` re-evaluates this.
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
            // Refused here rather than relying on the checkbox being disabled.
            // This is the one toggle that issues store operations -- renames of
            // `bin` and of whole version directories -- and
            // `Effect::CancelInFlight` cannot stop one: the cancel flag is read
            // by `download.rs` alone. `storelock` stops two workers being
            // inside the same store at once; this stops the app claiming to be
            // doing both, which a lock cannot do.
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
            // After the bump, which zeroes the count: one operation per game,
            // so `busy` holds until the last `ModeChanged` lands.
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
            // Cleared on every path out of this arm. Cancelling the dialog is
            // simply the absence of a `ConfirmRemove`, so a cancelled removal
            // would otherwise leave its pair sitting here.
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
        // confirmation that is not acted on must not stay armed for a later
        // one. `is_busy()` is re-checked here and not only at `ClickRemove`,
        // because `NSAlert::runModal` pumps a nested run loop: messages really
        // are delivered between the click and the answer.
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
            // confirmation and is entitled to know that nothing happened;
            // returning no effects silently left the dialog answered, the build
            // still installed, and nothing on screen to explain it. The title
            // is the same one a failed removal shows, because from the user's
            // side the outcome is the same and the reason belongs in the
            // message.
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
            None => Vec::new(),
        },

        Msg::InstalledLoaded { generation, result } => {
            if generation != state.generation {
                return Vec::new();
            }
            match result {
                Ok((installed, active)) => {
                    // `installed`/`active` are not in `state` yet, so
                    // `active_entry` cannot be used here; `is_active` is the
                    // same mode-aware comparison applied to the local values.
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

        // Only ever *updates* a busy state an initiated operation established;
        // it never creates one. The `outstanding` test is belt and braces --
        // the main queue is FIFO -- but it costs one comparison to make a lost
        // or reordered report unable to strand the UI busy with nothing
        // running.
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
            // does not. The delete is only attempted once the switch away from
            // the active version succeeded, so a failed delete still leaves
            // `bin` pointing somewhere new.
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

        Msg::GameDataScanned(found) => {
            for (game, path) in found {
                state.game_data[UiState::data_slot(game)] = path;
            }
            Vec::new()
        }

        Msg::ChooseInstaller(game) => vec![Effect::PickInstaller(game)],

        Msg::InstallerChosen { game, installer } => {
            let Some(installer) = installer else {
                return Vec::new();
            };

            begin(state, 1);
            vec![Effect::InstallGameData {
                generation: state.generation,
                game,
                installer,
            }]
        }

        Msg::GameDataInstalled {
            generation,
            game,
            result,
        } => {
            if generation != state.generation {
                return Vec::new();
            }
            end(state);

            match result {
                Ok(path) => {
                    state.game_data[UiState::data_slot(game)] = Some(path);
                    Vec::new()
                }
                Err(message) => {
                    state.error = Some(Alert {
                        title: "FailedToInstallGameData".into(),
                        message,
                        message_is_key: false,
                        generation: Some(state.generation),
                        title_arg: None,
                    });
                    Vec::new()
                }
            }
        }

        Msg::ChooseInstallRoot => vec![Effect::PickInstallRoot],

        Msg::InstallRootChosen(root) => {
            state.install_root = root.clone();
            vec![
                Effect::SavePref(Pref::InstallRoot(
                    root.map(|path| path.to_string_lossy().into_owned()),
                )),
                // Where the data is depends on where it is unpacked.
                Effect::ScanGameData,
            ]
        }

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
                    // Each event clears exactly the error class it owns. A
                    // launch carries no generation of its own, so nothing else
                    // ever clears a launch error -- but this must not dismiss
                    // an unrelated one the user has not read.
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
/// never anywhere else: an increment with no effect behind it is an operation
/// that never replies.
///
/// Deliberately not called for `Effect::Launch`, whose reply carries no
/// generation of its own and so cannot be reconciled with a count that a
/// generation bump zeroes. `play_enabled` already refuses to start a launch
/// while busy.
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
/// issues one `SetMode` per game, so clearing on the first reply would re-open
/// every guard while the second store is still mid-rename.
///
/// `saturating_sub` so a reply that somehow outlives its count cannot wrap the
/// counter and leave the UI permanently busy.
fn end(state: &mut UiState) {
    state.outstanding = state.outstanding.saturating_sub(1);
    if state.outstanding == 0 {
        state.busy = None;
    }
}

/// Advances to a new generation, returning the one being abandoned so the
/// caller can tell the executor to cancel whatever was running under it.
///
/// Clears `busy` and the outstanding count unconditionally: every reply that
/// would otherwise clear them is itself gated on a generation match, so an
/// operation whose reply is discarded as stale would leave every control
/// disabled forever. Do not make either reset conditional.
///
/// It does not stop the abandoned work itself, so a new operation really can
/// be *initiated* while an abandoned store operation is still running. What it
/// cannot do is let the two run *together*: every store operation runs under
/// one lock (`storelock`), so the new one waits.
///
/// `error` is cleared only when it is generation-scoped; a launch error has
/// nothing to do with switching games.
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

/// Is `entry` the one `active` refers to? See `UiState::active`: this cannot
/// be a plain `==`, because `active` is an `Installed::name` in multi-version
/// mode but an `Installed::tag` in compatible mode.
fn is_active(entry: &Installed, active: &str, multi_version: bool) -> bool {
    if multi_version {
        entry.name == active
    } else {
        entry.tag == active
    }
}

/// Which build to switch to before `name` can be deleted: the newest other
/// installed version, or `None` when there is no other one. "Newest" is
/// `VersionStore::installed`'s own order rather than a re-sort here. Compares
/// `name`, never `tag`: two installed entries can share a display tag.
fn switch_target(state: &UiState, name: &str) -> Option<String> {
    state
        .installed
        .iter()
        .find(|i| i.name != name)
        .map(|i| i.name.clone())
}

/// Takes a set of releases as the current ones, from a fetch or from the
/// cache, and returns whatever that implies. Both paths go through here on
/// purpose: a cache hit that merely assigned `state.releases` would silently
/// stop auto-update from ever firing on a game switch.
fn adopt_releases(state: &mut UiState, rows: Vec<ReleaseRow>) -> Vec<Effect> {
    state.releases = rows;
    state.selected_release = 0;
    // No `is_busy()` guard, unlike the arms a user drives. This is not a click
    // to be distrusted, and refusing it would mean auto-update quietly not
    // updating for the rest of the session, since nothing re-runs it.
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

fn cached(state: &UiState, game: GameId, include_develop: bool) -> Option<Vec<ReleaseRow>> {
    state.release_cache.get(&(game, include_develop)).cloned()
}

/// The newest listed release's tag, unless it is already the one active.
/// Tag-to-tag: `active` is an identity rather than a tag in multi-version
/// mode, and a directory renamed to resolve a collision keeps its original
/// tag.
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
    /// values -- the one axis `installed` itself cannot express.
    fn installed_named(tag: &str, name: &str, can_launch: bool) -> Installed {
        Installed {
            tag: tag.to_string(),
            name: name.to_string(),
            dir: std::path::PathBuf::from(format!("/tmp/{name}")),
            can_launch,
        }
    }

    fn an_update(version: &str) -> turnstile_core::selfupdate::Update {
        turnstile_core::selfupdate::Update {
            version: version.into(),
            url: format!("https://example.invalid/{version}"),
        }
    }

    #[test]
    fn a_found_update_shows_and_dismissing_clears_it() {
        let mut s = UiState::new(GameId::OpenRCT2);
        reduce(&mut s, Msg::UpdateChecked(Some(an_update("0.2.0"))));
        assert_eq!(s.update.as_ref().map(|u| u.version.as_str()), Some("0.2.0"));
        reduce(&mut s, Msg::DismissUpdate);
        assert!(s.update.is_none());
    }

    #[test]
    fn a_check_that_answers_after_the_setting_was_turned_off_shows_nothing() {
        let mut s = UiState::new(GameId::OpenRCT2);
        reduce(&mut s, Msg::ToggleCheckForUpdates(false));
        reduce(&mut s, Msg::UpdateChecked(Some(an_update("0.2.0"))));
        assert!(s.update.is_none());
    }

    #[test]
    fn turning_the_setting_off_takes_down_a_banner_already_showing() {
        let mut s = UiState::new(GameId::OpenRCT2);
        reduce(&mut s, Msg::UpdateChecked(Some(an_update("0.2.0"))));
        assert!(s.update.is_some());
        reduce(&mut s, Msg::ToggleCheckForUpdates(false));
        assert!(
            s.update.is_none(),
            "the setting must apply to what is on screen"
        );
    }

    #[test]
    fn opening_the_page_asks_for_that_url_and_clears_the_banner() {
        let mut s = UiState::new(GameId::OpenRCT2);
        reduce(&mut s, Msg::UpdateChecked(Some(an_update("0.2.0"))));
        let effects = reduce(&mut s, Msg::OpenUpdatePage);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::OpenUrl(u) if u == "https://example.invalid/0.2.0")),
            "the url must come from the update that was found"
        );
        assert!(
            s.update.is_none(),
            "somebody who opened the page has been told"
        );
    }

    #[test]
    fn opening_the_page_with_no_update_does_nothing() {
        let mut s = UiState::new(GameId::OpenRCT2);
        assert!(reduce(&mut s, Msg::OpenUpdatePage).is_empty());
    }

    #[test]
    fn the_setting_is_persisted_whichever_way_it_moves() {
        let mut s = UiState::new(GameId::OpenRCT2);
        for on in [false, true] {
            let effects = reduce(&mut s, Msg::ToggleCheckForUpdates(on));
            assert!(
                effects
                    .iter()
                    .any(|e| matches!(e, Effect::SavePref(Pref::CheckForUpdates(v)) if *v == on)),
                "toggling to {on} must be saved"
            );
        }
    }

    #[test]
    fn checking_for_updates_defaults_on() {
        assert!(UiState::new(GameId::OpenRCT2).check_for_updates);
    }

    #[test]
    fn a_cancelled_picker_does_nothing_at_all() {
        let mut s = UiState::new(GameId::OpenRCT2);
        let effects = reduce(
            &mut s,
            Msg::InstallerChosen {
                game: OriginalGame::RollerCoasterTycoon2,
                installer: None,
            },
        );
        assert!(effects.is_empty());
        assert!(!s.is_busy(), "a cancelled picker must not leave it busy");
    }

    #[test]
    fn choosing_an_installer_goes_busy_and_asks_for_the_install() {
        let mut s = UiState::new(GameId::OpenRCT2);
        let effects = reduce(
            &mut s,
            Msg::InstallerChosen {
                game: OriginalGame::Locomotion,
                installer: Some("/tmp/setup.exe".into()),
            },
        );

        assert!(s.is_busy());
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::InstallGameData {
                game: OriginalGame::Locomotion,
                ..
            }
        )));
    }

    #[test]
    fn an_installed_game_records_where_it_landed() {
        let mut s = UiState::new(GameId::OpenRCT2);
        reduce(
            &mut s,
            Msg::InstallerChosen {
                game: OriginalGame::RollerCoasterTycoon2,
                installer: Some("/tmp/setup.exe".into()),
            },
        );

        let generation = s.generation;
        reduce(
            &mut s,
            Msg::GameDataInstalled {
                generation,
                game: OriginalGame::RollerCoasterTycoon2,
                result: Ok("/games/rct2".into()),
            },
        );

        assert_eq!(
            s.game_data(OriginalGame::RollerCoasterTycoon2),
            Some(std::path::Path::new("/games/rct2"))
        );
        assert!(!s.is_busy());
        assert!(s.error.is_none());
    }

    #[test]
    fn one_game_finishing_does_not_touch_another() {
        let mut s = UiState::new(GameId::OpenRCT2);
        reduce(
            &mut s,
            Msg::GameDataScanned(vec![
                (OriginalGame::RollerCoasterTycoon1, Some("/a".into())),
                (OriginalGame::RollerCoasterTycoon2, None),
                (OriginalGame::Locomotion, Some("/c".into())),
            ]),
        );

        assert_eq!(
            s.game_data(OriginalGame::RollerCoasterTycoon1),
            Some(std::path::Path::new("/a"))
        );
        assert_eq!(s.game_data(OriginalGame::RollerCoasterTycoon2), None);
        assert_eq!(
            s.game_data(OriginalGame::Locomotion),
            Some(std::path::Path::new("/c"))
        );
    }

    #[test]
    fn a_game_data_reply_from_a_stale_generation_is_discarded() {
        let mut s = UiState::new(GameId::OpenRCT2);
        reduce(
            &mut s,
            Msg::InstallerChosen {
                game: OriginalGame::Locomotion,
                installer: Some("/tmp/setup.exe".into()),
            },
        );
        let stale = s.generation;
        reduce(&mut s, Msg::SelectGame(GameId::OpenLoco));

        reduce(
            &mut s,
            Msg::GameDataInstalled {
                generation: stale,
                game: OriginalGame::Locomotion,
                result: Ok("/games/locomotion".into()),
            },
        );

        assert_eq!(s.game_data(OriginalGame::Locomotion), None);
    }

    #[test]
    fn a_failed_install_reports_it_and_leaves_the_path_alone() {
        let mut s = UiState::new(GameId::OpenRCT2);
        reduce(
            &mut s,
            Msg::GameDataScanned(vec![(
                OriginalGame::RollerCoasterTycoon2,
                Some("/old/rct2".into()),
            )]),
        );
        reduce(
            &mut s,
            Msg::InstallerChosen {
                game: OriginalGame::RollerCoasterTycoon2,
                installer: Some("/tmp/setup.exe".into()),
            },
        );

        let generation = s.generation;
        reduce(
            &mut s,
            Msg::GameDataInstalled {
                generation,
                game: OriginalGame::RollerCoasterTycoon2,
                result: Err("the installer did not contain Data/g1.dat".into()),
            },
        );

        assert!(s.error.is_some());
        assert!(!s.is_busy());
        assert_eq!(
            s.game_data(OriginalGame::RollerCoasterTycoon2),
            Some(std::path::Path::new("/old/rct2")),
            "a failed install must not forget the install that is still there"
        );
    }

    #[test]
    fn changing_the_install_directory_saves_it_and_looks_again() {
        let mut s = UiState::new(GameId::OpenRCT2);
        let effects = reduce(&mut s, Msg::InstallRootChosen(Some("/elsewhere".into())));

        assert_eq!(
            s.install_root.as_deref(),
            Some(std::path::Path::new("/elsewhere"))
        );
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::SavePref(Pref::InstallRoot(Some(path))) if path == "/elsewhere"
        )));
        assert!(effects.iter().any(|e| matches!(e, Effect::ScanGameData)));
    }

    #[test]
    fn going_back_to_the_default_saves_the_absence_of_a_path() {
        let mut s = UiState::new(GameId::OpenRCT2);
        reduce(&mut s, Msg::InstallRootChosen(Some("/elsewhere".into())));
        let effects = reduce(&mut s, Msg::InstallRootChosen(None));

        assert!(s.install_root.is_none());
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SavePref(Pref::InstallRoot(None))))
        );
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
            !back
                .iter()
                .any(|e| matches!(e, Effect::FetchReleases { .. })),
            "returning to a game already fetched must not ask again"
        );
        assert_eq!(
            s.releases
                .iter()
                .map(|r| r.tag.as_str())
                .collect::<Vec<_>>(),
            vec!["v3", "v2"],
            "and the cached releases must actually be restored"
        );
    }

    #[test]
    fn a_cache_hit_still_evaluates_auto_update() {
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
            s.releases
                .iter()
                .map(|r| r.tag.as_str())
                .collect::<Vec<_>>(),
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
        assert_eq!(s.selected_installed, 0);
        assert_eq!(s.installed.len(), 2);
        assert_eq!(s.active.as_deref(), Some("v2"));
        assert!(s.error.is_none());
        assert!(!s.is_busy());

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

    #[test]
    fn a_confirmation_answered_after_work_has_started_is_refused() {
        let mut s = ready_state();
        s.multi_version = true;
        s.selected_installed = 1;
        reduce(&mut s, Msg::ClickRemove);

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

    #[test]
    fn a_confirmation_refused_because_work_started_says_so() {
        let mut s = ready_state();
        s.multi_version = true;
        s.selected_installed = 1;
        s.set_auto_update(s.selected_game, true);
        assert!(s.remove_enabled(), "Remove does not depend on auto-update");

        reduce(&mut s, Msg::ClickRemove);

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

    #[test]
    fn selecting_another_installed_version_is_refused_while_busy() {
        let mut s = ready_state();
        s.multi_version = true;
        reduce(&mut s, Msg::ClickDownload);

        let effects = reduce(&mut s, Msg::SelectInstalled(1));

        assert!(effects.is_empty(), "got {effects:?}");
        assert_eq!(s.selected_installed, 0, "and the selection did not move");
    }

    #[test]
    fn a_generation_bump_clears_busy_that_was_set_at_initiation() {
        for interrupt in [Msg::SelectGame(GameId::OpenLoco), Msg::ToggleDevelop(true)] {
            let mut s = ready_state();
            reduce(&mut s, Msg::ClickDownload);
            assert!(s.is_busy());
            let stale = s.generation;

            reduce(&mut s, interrupt);
            assert!(s.busy.is_none(), "a bump must not leave the UI stuck busy");

            reduce(
                &mut s,
                Msg::InstallFinished {
                    generation: stale,
                    result: Ok(()),
                },
            );
            assert!(s.busy.is_none());

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
