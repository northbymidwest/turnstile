# Changelog

## 0.3.0 - 2026-09-18

### Added

- Installs the original games' data from the Windows installers sold by GOG.
  OpenRCT2 and OpenLoco reimplement the game engines, not the games, and need the
  graphics, sounds and scenarios from the original release before they will run.
  There was nothing to point them at on a Mac, because these games were never
  released for one. Turnstile now reads the installer, puts the game in a
  directory, and tells the game where it is.
- A "Game data" section in each game's pane, showing where its data is or that it
  is missing. OpenRCT2 shows both RollerCoaster Tycoon 2, which it needs, and
  RollerCoaster Tycoon 1, whose scenarios and objects it uses when they are there.
  A game whose data is missing used to start and then fail with nothing said in
  advance.
- A setting for where game data is unpacked, with the default under Turnstile's
  own directory, and a button beside it to go back to that default.

### Changed

- Where a game's data is comes from the game's own configuration rather than from
  Turnstile's settings, so a directory chosen by hand, or years ago, is found and
  shown rather than quietly replaced.

## 0.2.0 - 2026-09-17

### Added

- Checks on launch whether a newer Turnstile has been released, and offers to open the
  releases page if there is one. Nothing is downloaded or installed, prereleases are
  ignored, and a check that fails says nothing rather than reporting a problem you did
  not ask about.
- A Settings window, on Command-comma, for the settings that are about the application
  rather than about a game. "Keep multiple versions installed" moves there from the
  main window, and the update check joins it. The two settings that stay in the main
  window are per-game.
- Finder files Turnstile under Games when applications are arranged by category.

### Fixed

- Closing the window quits the application. It used to keep running with no window and
  no way to get one back.

### Changed

- The list of available builds is fetched once per game and kept, so switching between
  OpenRCT2 and OpenLoco no longer refetches it.

## 0.1.0 - 2026-09-17

First release.

### Added

- Downloads OpenRCT2 and OpenLoco builds from GitHub releases, installs them, and
  launches the game. Stable and development channels, with an optional automatic
  update per game.
- Compatible by default with OpenLauncher's layout: the install location and the
  `.version` file are the same, so a game installed by either application is visible
  to the other.
- Several versions can be installed and switched between instantly, if the option is
  turned on. Turning it off again collapses back to a single install and leaves the
  other builds on disk rather than deleting them.
- Installs stage the download and extraction somewhere harmless and touch the
  destination only through two renames, with a marker file recording intent, so a
  crash at any point is recoverable and cannot leave you without a playable game.
- Nine languages, imported from upstream OpenLauncher's resources at a pinned commit,
  with the strings Turnstile adds translated alongside them.
- Universal binary, signed with a Developer ID certificate and notarized, so it opens
  without a Gatekeeper warning. Requires macOS 11 or later; on macOS 26 and later it
  carries a Liquid Glass icon.
- The bundle carries both licenses: Turnstile's own 0BSD, and OpenLauncher's MIT, which
  covers the icons and translations borrowed from it.
