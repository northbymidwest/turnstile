# Changelog

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
  without a Gatekeeper warning. The bundle carries both licenses: Turnstile's own 0BSD
  and OpenLauncher's MIT, which covers the icons and translations it borrows. Requires macOS 11 or later; on macOS 26 and later it
  carries a Liquid Glass icon.
