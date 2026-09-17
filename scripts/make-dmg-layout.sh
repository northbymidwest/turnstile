#!/bin/bash
# Regenerates resources/dmg-DS_Store, which positions the icons and sizes the
# window Finder opens when somebody mounts the release disk image.
#
# Run this on a Mac with a logged-in graphical session, not in CI. Arranging a
# Finder window is AppleScript against Finder, and a CI runner has no Finder to
# talk to. That is the whole reason the result is committed rather than built:
# the release workflow copies this file into the image and never needs Finder.
#
# It opens a Finder window briefly while it works. That is the tool doing its
# job, not something going wrong.
#
# Committed as dmg-DS_Store rather than .DS_Store because .gitignore excludes
# the latter, here and nearly everywhere else. scripts/bundle.sh's companion in
# the release workflow renames it on the way into the image.
set -euo pipefail

APP="${1:-dist/Turnstile.app}"
OUT="resources/dmg-DS_Store"
VOLUME="TurnstileLayout"

[ -d "${APP}" ] || {
  echo "error: no app bundle at ${APP}. Run ./scripts/bundle.sh first." >&2
  exit 1
}

work="$(mktemp -d)"
trap 'rm -rf "${work}"' EXIT
mkdir -p "${work}/stage"
cp -R "${APP}" "${work}/stage/"
ln -s /Applications "${work}/stage/Applications"

# Read-write, because Finder has to be able to write the .DS_Store back.
hdiutil create -quiet -volname "${VOLUME}" -srcfolder "${work}/stage" \
  -ov -format UDRW "${work}/rw.dmg"
mount_point=$(hdiutil attach "${work}/rw.dmg" -nobrowse | tail -1 |
  sed 's/.*\(\/Volumes\/.*\)/\1/')
trap 'hdiutil detach "${mount_point}" -quiet >/dev/null 2>&1 || true; rm -rf "${work}"' EXIT

# The application first and the drop target second, reading left to right,
# which is the order the gesture happens in.
osascript <<APPLESCRIPT
tell application "Finder"
  tell disk "${VOLUME}"
    open
    set current view of container window to icon view
    set toolbar visible of container window to false
    set statusbar visible of container window to false
    set the bounds of container window to {200, 150, 800, 540}
    set opts to the icon view options of container window
    set arrangement of opts to not arranged
    set icon size of opts to 128
    set text size of opts to 13
    set position of item "$(basename "${APP}")" of container window to {150, 190}
    set position of item "Applications" of container window to {450, 190}
    close
    open
    update without registering applications
    delay 2
    close
  end tell
end tell
APPLESCRIPT

# Finder writes asynchronously; give it a moment before reading the file back.
sleep 2
[ -f "${mount_point}/.DS_Store" ] || {
  echo "error: Finder wrote no .DS_Store. Is a graphical session available?" >&2
  exit 1
}
cp "${mount_point}/.DS_Store" "${OUT}"
echo "Wrote ${OUT} ($(stat -c%s "${OUT}") bytes)"
