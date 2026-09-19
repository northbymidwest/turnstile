//! The interface's behavior, with no AppKit anywhere near it. `UiState` holds
//! what is on screen; `reduce` is the only thing allowed to change it, and it
//! returns `Effect`s for something else to carry out and report back as
//! another `Msg`. Control enablement is always derived from state, never a
//! separately-assigned flag.
//!
//! Every effect that resolves a directory is tagged with the `generation`
//! current when it was issued, and any reply carrying a stale generation is
//! discarded.

mod gamedata;
mod releases;
mod store;

use std::collections::HashMap;
use std::path::PathBuf;

use turnstile_core::age::Age;
use turnstile_core::download::Status;
use turnstile_core::game::GameId;
use turnstile_core::gamedata::OriginalGame;
use turnstile_core::release::{Channel, Download};
use turnstile_core::store::{DisableReport, Installed};

/// One row of the available-releases popup, already formatted for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseRow {
    pub tag: String,
    pub age: Option<Age>,
    pub channel: Channel,
    /// Carried from the fetch that listed this release, so installing it needs
    /// no second request.
    pub download: Download,
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

/// Why installing game data failed. `Unrecognised` is the one case with
/// wording of its own, so it travels as a key rather than a worker's message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameDataError {
    Unrecognised,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pref {
    CheckForUpdates(bool),
    /// Where the original games' data is unpacked. `None` is the default root,
    /// which has to survive a restart as an answer distinct from a path that
    /// happens to equal today's default.
    InstallRoot(Option<String>),
    ShowDevelop(bool),
    AutoUpdate(GameId, bool),
    MultiVersion(bool),
    SelectedGame(GameId),
}

#[derive(Debug)]
pub struct UiState {
    pub selected_game: GameId,
    pub installed: Vec<Installed>,
    /// Read through `installed_row`, never directly: a reply can shrink
    /// `installed` under an index the popup handed us a moment earlier.
    selected_installed: usize,
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
    /// Read through `release_row`, for the same reason as
    /// `selected_installed`.
    selected_release: usize,
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

    /// Which row of the installed popup to show as selected, or `None` when
    /// there is nothing to select. Always within the list, so a caller cannot
    /// hand AppKit an index the popup does not have.
    #[must_use]
    pub fn installed_row(&self) -> Option<usize> {
        row_within(self.selected_installed, self.installed.len())
    }

    /// The same, for the available-builds popup.
    #[must_use]
    pub fn release_row(&self) -> Option<usize> {
        row_within(self.selected_release, self.releases.len())
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
        result: Result<PathBuf, GameDataError>,
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
        download: Download,
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
        Msg::SelectGame(game) => select_game(state, game),
        Msg::UpdateChecked(found) => update_checked(state, found),
        Msg::DismissUpdate => dismiss_update(state),
        Msg::OpenUpdatePage => open_update_page(state),
        Msg::ToggleCheckForUpdates(on) => toggle_check_for_updates(state, on),
        Msg::Progress {
            generation,
            status,
            value,
        } => progress(state, generation, status, value),
        Msg::StartupRepairFailed { message } => startup_repair_failed(state, message),
        Msg::ShowSettings => vec![Effect::ShowSettings],
        Msg::SelectRelease(index) => releases::select_release(state, index),
        Msg::ToggleDevelop(on) => releases::toggle_develop(state, on),
        Msg::ToggleAutoUpdate(on) => releases::toggle_auto_update(state, on),
        Msg::ClickDownload => releases::click_download(state),
        Msg::ReleasesLoaded { generation, result } => {
            releases::releases_loaded(state, generation, result)
        }
        Msg::InstallFinished { generation, result } => {
            releases::install_finished(state, generation, result)
        }
        Msg::SelectInstalled(index) => store::select_installed(state, index),
        Msg::ToggleMultiVersion(on) => store::toggle_multi_version(state, on),
        Msg::ClickPlay => store::click_play(state),
        Msg::ClickRemove => store::click_remove(state),
        Msg::ConfirmRemove => store::confirm_remove(state),
        Msg::InstalledLoaded { generation, result } => {
            store::installed_loaded(state, generation, result)
        }
        Msg::Activated { generation, result } => store::activated(state, generation, result),
        Msg::Removed { generation, result } => store::removed(state, generation, result),
        Msg::ModeChanged { generation, result } => store::mode_changed(state, generation, result),
        Msg::LaunchFinished { result } => store::launch_finished(state, result),
        Msg::GameDataScanned(found) => gamedata::game_data_scanned(state, found),
        Msg::InstallerChosen { game, installer } => {
            gamedata::installer_chosen(state, game, installer)
        }
        Msg::GameDataInstalled {
            generation,
            game,
            result,
        } => gamedata::game_data_installed(state, generation, game, result),
        Msg::InstallRootChosen(root) => gamedata::install_root_chosen(state, root),
        Msg::ChooseInstaller(game) => vec![Effect::PickInstaller(game)],
        Msg::ChooseInstallRoot => vec![Effect::PickInstallRoot],
    }
}

fn select_game(state: &mut UiState, game: GameId) -> Vec<Effect> {
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
    match releases::cached(state, game, state.show_develop) {
        Some(rows) => effects.extend(releases::adopt_releases(state, rows)),
        None => effects.push(Effect::FetchReleases {
            generation: state.generation,
            game,
            include_develop: state.show_develop,
        }),
    }
    effects
}

fn update_checked(
    state: &mut UiState,
    found: Option<turnstile_core::selfupdate::Update>,
) -> Vec<Effect> {
    // Only when the check is still wanted: the setting can be turned
    // off while the request is in flight.
    if state.check_for_updates {
        state.update = found;
    }
    Vec::new()
}

fn dismiss_update(state: &mut UiState) -> Vec<Effect> {
    state.update = None;
    Vec::new()
}

fn open_update_page(state: &mut UiState) -> Vec<Effect> {
    // Taken before clearing, and the banner goes either way: somebody
    // who has opened the page has been told.
    let effects = match &state.update {
        Some(update) => vec![Effect::OpenUrl(update.url.clone())],
        None => Vec::new(),
    };
    state.update = None;
    effects
}

fn toggle_check_for_updates(state: &mut UiState, on: bool) -> Vec<Effect> {
    state.check_for_updates = on;
    // Turning it off takes down a banner already showing.
    if !on {
        state.update = None;
    }
    vec![Effect::SavePref(Pref::CheckForUpdates(on))]
}

/// Only ever *updates* a busy state an initiated operation established; it
/// never creates one. The `outstanding` test is belt and braces -- the main
/// queue is FIFO -- but it costs one comparison to make a lost or reordered
/// report unable to strand the UI busy with nothing running.
fn progress(
    state: &mut UiState,
    generation: u64,
    status: Status,
    value: Option<f64>,
) -> Vec<Effect> {
    if generation == state.generation && state.outstanding > 0 {
        state.busy = Some(Busy {
            status: Some(status),
            value,
        });
    }
    Vec::new()
}

fn startup_repair_failed(state: &mut UiState, message: String) -> Vec<Effect> {
    state.error = Some(Alert {
        title: "FailedToRepairInstallation".into(),
        message,
        message_is_key: false,
        generation: Some(state.generation),
        title_arg: None,
    });
    Vec::new()
}

/// Records that `count` operations have just been initiated, so every
/// `!is_busy()` guard closes *now* rather than whenever the first progress
/// report happens to arrive. Called in the same arm that emits the effects,
/// never anywhere else: an increment with no effect behind it is an operation
/// that never replies.
///
/// `Effect::Launch` is counted too, so a second click cannot start a second
/// copy of the game while the first is still being probed. Its reply carries
/// no generation, but `end`'s `saturating_sub` makes a bump that zeroed the
/// count harmless.
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
/// Clamps rather than rejects: the stored index is the last thing chosen, and
/// showing the nearest row that exists beats showing none while a reload is in
/// flight.
fn row_within(index: usize, len: usize) -> Option<usize> {
    len.checked_sub(1).map(|last| index.min(last))
}

fn is_active(entry: &Installed, active: &str, multi_version: bool) -> bool {
    if multi_version {
        entry.name == active
    } else {
        entry.tag == active
    }
}

#[cfg(test)]
mod tests;
