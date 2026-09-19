//! Installed versions: selecting, switching, removing, and the storage mode.

use turnstile_core::game::GameId;
use turnstile_core::store::{DisableReport, Installed};

use super::{
    Alert, Effect, PendingRemoval, Pref, UiState, begin, bump_generation, end, finish, is_active,
};

pub(super) fn select_installed(state: &mut UiState, index: usize) -> Vec<Effect> {
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
    if state.multi_version && state.active_entry().map(|e| e.name.as_str()) != Some(name.as_str()) {
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

pub(super) fn toggle_multi_version(state: &mut UiState, on: bool) -> Vec<Effect> {
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

pub(super) fn click_play(state: &mut UiState) -> Vec<Effect> {
    match state
        .selected_installed_dir()
        .map(std::path::Path::to_path_buf)
    {
        Some(dir) if state.play_enabled() => {
            begin(state, 1);
            vec![Effect::Launch {
                game: state.selected_game,
                dir,
            }]
        }
        _ => Vec::new(),
    }
}

pub(super) fn click_remove(state: &mut UiState) -> Vec<Effect> {
    {
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
}

/// The pair is taken on every path, including the refusal: a confirmation that
/// is not acted on must not stay armed for a later one. `is_busy()` is
/// re-checked here and not only at `click_remove`, because `NSAlert::runModal`
/// pumps a nested run loop: messages really are delivered between the click and
/// the answer.
pub(super) fn confirm_remove(state: &mut UiState) -> Vec<Effect> {
    match state.pending_removal.take() {
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
    }
}

pub(super) fn installed_loaded(
    state: &mut UiState,
    generation: u64,
    result: Result<(Vec<Installed>, Option<String>), String>,
) -> Vec<Effect> {
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

pub(super) fn activated(
    state: &mut UiState,
    generation: u64,
    result: Result<(), String>,
) -> Vec<Effect> {
    finish(state, generation, result, "FailedToSwitchVersion")
}

pub(super) fn removed(
    state: &mut UiState,
    generation: u64,
    result: Result<(), String>,
) -> Vec<Effect> {
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

pub(super) fn mode_changed(
    state: &mut UiState,
    generation: u64,
    result: Result<Option<DisableReport>, String>,
) -> Vec<Effect> {
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

pub(super) fn launch_finished(state: &mut UiState, result: Result<(), String>) -> Vec<Effect> {
    end(state);
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
