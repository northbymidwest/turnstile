use super::*;
use turnstile_core::store::Installed;

fn a_download() -> Download {
    Download {
        url: "https://example.invalid/build.zip".into(),
        bytes: 1,
    }
}

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
            result: Err(GameDataError::Failed(
                "the installer did not contain Data/g1.dat".into(),
            )),
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

/// `select_installed` stores whatever index it is handed and returns early
/// when that index names nothing, so the stored one really can outlive the
/// list. The popup is rebuilt from `installed` on the next render, and an
/// index past its end is not a row it has.
#[test]
fn a_selection_past_the_end_of_the_list_still_names_a_row_that_exists() {
    let mut s = UiState::new(GameId::OpenRCT2);
    s.installed = vec![installed("v1", true), installed("v2", true)];

    reduce(&mut s, Msg::SelectInstalled(99));
    let row = s
        .installed_row()
        .expect("a row, since the list is not empty");
    assert!(row < s.installed.len(), "row {row} is not in the list");

    reduce(&mut s, Msg::SelectRelease(99));
    s.releases = rows(&["a", "b"]);
    let row = s.release_row().expect("a row, since the list is not empty");
    assert!(row < s.releases.len(), "row {row} is not in the list");
}

#[test]
fn nothing_is_selected_when_a_list_is_empty() {
    let mut s = UiState::new(GameId::OpenRCT2);
    reduce(&mut s, Msg::SelectInstalled(3));
    reduce(&mut s, Msg::SelectRelease(3));
    assert_eq!(s.installed_row(), None);
    assert_eq!(s.release_row(), None);
}

#[test]
fn a_selection_within_the_list_is_left_alone() {
    let mut s = UiState::new(GameId::OpenRCT2);
    s.installed = vec![installed("v1", true), installed("v2", true)];
    reduce(&mut s, Msg::SelectInstalled(1));
    assert_eq!(s.installed_row(), Some(1));
}

#[test]
fn a_second_play_click_cannot_start_a_second_copy_of_the_game() {
    let mut s = UiState::new(GameId::OpenRCT2);
    s.installed = vec![installed("v1", true)];
    s.active = Some("v1".into());

    assert_eq!(reduce(&mut s, Msg::ClickPlay).len(), 1);
    assert!(s.is_busy());
    assert!(
        reduce(&mut s, Msg::ClickPlay).is_empty(),
        "a launch is already in flight"
    );

    reduce(&mut s, Msg::LaunchFinished { result: Ok(()) });
    assert!(!s.is_busy());
    assert_eq!(reduce(&mut s, Msg::ClickPlay).len(), 1);
}

#[test]
fn an_unrecognised_installer_reports_a_localizable_message() {
    let mut s = UiState::new(GameId::OpenRCT2);
    reduce(
        &mut s,
        Msg::InstallerChosen {
            game: OriginalGame::Locomotion,
            installer: Some("/tmp/setup.exe".into()),
        },
    );
    let generation = s.generation;
    reduce(
        &mut s,
        Msg::GameDataInstalled {
            generation,
            game: OriginalGame::Locomotion,
            result: Err(GameDataError::Unrecognised),
        },
    );

    let error = s.error.as_ref().expect("an error must be raised");
    assert_eq!(error.message, "UnrecognisedInstaller");
    assert!(
        error.message_is_key,
        "the one localizable failure must be looked up, not shown raw"
    );
    assert!(!s.is_busy());
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
            download: a_download(),
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
            download: a_download(),
        },
        ReleaseRow {
            tag: "v2".into(),
            age: None,
            channel: Channel::Release,
            download: a_download(),
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
        effects
            .iter()
            .any(|e| matches!(e, Effect::CancelInFlight { generation } if *generation == before))
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
        effects
            .iter()
            .any(|e| matches!(e, Effect::CancelInFlight { generation } if *generation == before))
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
        effects
            .iter()
            .any(|e| matches!(e, Effect::CancelInFlight { generation } if *generation == before))
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
                download: a_download(),
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
                download: a_download(),
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
                download: a_download(),
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
                download: a_download(),
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
                download: a_download(),
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
                download: a_download(),
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
                download: a_download(),
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
            .any(|e| matches!(e, Effect::SavePref(Pref::AutoUpdate(_, true))))
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
                download: a_download(),
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
            download: a_download(),
        },
        ReleaseRow {
            tag: "v2".into(),
            age: None,
            channel: Channel::Release,
            download: a_download(),
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
                    download: a_download(),
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
                download: a_download(),
            }]),
        },
    );

    assert!(
        !effects.iter().any(|e| matches!(e, Effect::Install { .. })),
        "must not re-download a build that is already installed and active, even when a \
         name/tag collision means its on-disk name differs from its tag"
    );
}
