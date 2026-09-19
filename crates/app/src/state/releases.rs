//! The available-builds list, and the installs it drives.

use turnstile_core::game::GameId;
use turnstile_core::release::Download;

use super::{Alert, Effect, Pref, ReleaseRow, UiState, begin, bump_generation, end};

pub(super) fn select_release(state: &mut UiState, index: usize) -> Vec<Effect> {
    state.selected_release = index;
    Vec::new()
}

pub(super) fn toggle_develop(state: &mut UiState, on: bool) -> Vec<Effect> {
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

pub(super) fn toggle_auto_update(state: &mut UiState, on: bool) -> Vec<Effect> {
    state.set_auto_update(state.selected_game, on);
    let mut effects = vec![Effect::SavePref(Pref::AutoUpdate(state.selected_game, on))];
    // The preference is saved either way; only the install it would
    // trigger waits. The next `ReleasesLoaded` re-evaluates this.
    if on
        && !state.is_busy()
        && let Some((tag, download)) = newest_installable(state)
    {
        begin(state, 1);
        effects.push(Effect::Install {
            generation: state.generation,
            game: state.selected_game,
            tag,
            download,
        });
    }
    effects
}

pub(super) fn click_download(state: &mut UiState) -> Vec<Effect> {
    let Some(row) = state.releases.get(state.selected_release) else {
        return Vec::new();
    };
    let (tag, download) = (row.tag.clone(), row.download.clone());
    if !state.download_enabled() {
        return Vec::new();
    }
    begin(state, 1);
    vec![Effect::Install {
        generation: state.generation,
        game: state.selected_game,
        tag,
        download,
    }]
}

pub(super) fn releases_loaded(
    state: &mut UiState,
    generation: u64,
    result: Result<Vec<ReleaseRow>, String>,
) -> Vec<Effect> {
    {
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
}

pub(super) fn install_finished(
    state: &mut UiState,
    generation: u64,
    result: Result<(), String>,
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

/// Takes a set of releases as the current ones, from a fetch or from the
/// cache, and returns whatever that implies. Both paths go through here on
/// purpose: a cache hit that merely assigned `state.releases` would silently
/// stop auto-update from ever firing on a game switch.
pub(super) fn adopt_releases(state: &mut UiState, rows: Vec<ReleaseRow>) -> Vec<Effect> {
    state.releases = rows;
    state.selected_release = 0;
    // No `is_busy()` guard, unlike the arms a user drives. This is not a click
    // to be distrusted, and refusing it would mean auto-update quietly not
    // updating for the rest of the session, since nothing re-runs it.
    if state.auto_update()
        && let Some((tag, download)) = newest_installable(state)
    {
        begin(state, 1);
        return vec![Effect::Install {
            generation: state.generation,
            game: state.selected_game,
            tag,
            download,
        }];
    }
    Vec::new()
}

pub(super) fn cached(
    state: &UiState,
    game: GameId,
    include_develop: bool,
) -> Option<Vec<ReleaseRow>> {
    state.release_cache.get(&(game, include_develop)).cloned()
}

/// The newest listed release's tag, unless it is already the one active.
/// Tag-to-tag: `active` is an identity rather than a tag in multi-version
/// mode, and a directory renamed to resolve a collision keeps its original
/// tag.
fn newest_installable(state: &UiState) -> Option<(String, Download)> {
    let newest = state.releases.first()?;
    let active_tag = state.active_entry().map(|i| i.tag.as_str());
    if active_tag == Some(newest.tag.as_str()) {
        return None;
    }
    Some((newest.tag.clone(), newest.download.clone()))
}
