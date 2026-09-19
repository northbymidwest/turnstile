//! The original games' data: where it is, and unpacking it from an installer.

use std::path::PathBuf;

use turnstile_core::gamedata::OriginalGame;

use super::{Alert, Effect, GameDataError, Pref, UiState, begin, end};

pub(super) fn game_data_scanned(
    state: &mut UiState,
    found: Vec<(OriginalGame, Option<PathBuf>)>,
) -> Vec<Effect> {
    for (game, path) in found {
        state.game_data[UiState::data_slot(game)] = path;
    }
    Vec::new()
}

pub(super) fn installer_chosen(
    state: &mut UiState,
    game: OriginalGame,
    installer: Option<PathBuf>,
) -> Vec<Effect> {
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

pub(super) fn game_data_installed(
    state: &mut UiState,
    generation: u64,
    game: OriginalGame,
    result: Result<PathBuf, GameDataError>,
) -> Vec<Effect> {
    if generation != state.generation {
        return Vec::new();
    }
    end(state);

    match result {
        Ok(path) => {
            state.game_data[UiState::data_slot(game)] = Some(path);
            Vec::new()
        }
        Err(error) => {
            let (message, message_is_key) = match error {
                GameDataError::Unrecognised => ("UnrecognisedInstaller".into(), true),
                GameDataError::Failed(message) => (message, false),
            };
            state.error = Some(Alert {
                title: "FailedToInstallGameData".into(),
                message,
                message_is_key,
                generation: Some(generation),
                title_arg: None,
            });
            Vec::new()
        }
    }
}

pub(super) fn install_root_chosen(state: &mut UiState, root: Option<PathBuf>) -> Vec<Effect> {
    state.install_root = root.clone();
    vec![
        Effect::SavePref(Pref::InstallRoot(
            root.map(|path| path.to_string_lossy().into_owned()),
        )),
        // Where the data is depends on where it is unpacked.
        Effect::ScanGameData,
    ]
}
