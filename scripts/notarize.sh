#!/bin/bash
# Submits an already-built, Developer ID signed Turnstile.app to Apple's
# notary service, then staples the ticket to it.
#
# Separate from bundle.sh on purpose. Bundling is local and offline;
# notarization uploads the binary to Apple, takes minutes rather than
# seconds, and needs credentials. Folding it into every build would make the
# ordinary path slow and network-dependent for no gain.
#
# Credentials are read from a keychain profile, never from this file and
# never from the environment. Create one once:
#
#   xcrun notarytool store-credentials turnstile-notary \
#     --apple-id <your-apple-id> --team-id <your-team-id>
#
# It prompts for an app-specific password, generated at appleid.apple.com.
# That is not your Apple ID password: an app-specific password can be
# revoked on its own without disturbing anything else. An App Store Connect
# API key works too and is better for CI, since it is not tied to a person.
#
# Override the profile name with NOTARY_PROFILE if you use a different one.
set -euo pipefail

APP="dist/Turnstile.app"
PROFILE="${NOTARY_PROFILE:-turnstile-notary}"
ZIP="dist/Turnstile-notarize.zip"

if [ ! -d "$APP" ]; then
  echo "error: $APP not found. Run ./scripts/bundle.sh first." >&2
  exit 1
fi

# Read the signature once into a variable rather than piping codesign into
# grep twice. `grep -q` exits on its first match, which closes the pipe and
# leaves codesign killed by SIGPIPE; under `pipefail` that makes the whole
# pipeline non-zero and the guard fires backwards, reporting a correctly
# signed app as unsigned. Found the hard way.
SIGINFO="$(codesign -dv --verbose=4 "$APP" 2>&1)"

# Refuse an ad-hoc build rather than letting Apple reject it several minutes
# later with a less obvious message. An ad-hoc signature has no team behind
# it, so it can never notarize.
if printf '%s' "$SIGINFO" | grep -q '^Signature=adhoc'; then
  echo "error: $APP is ad-hoc signed and cannot be notarized." >&2
  echo "Build on a machine with a Developer ID Application certificate," >&2
  echo "or set SIGN_IDENTITY, then run ./scripts/bundle.sh again." >&2
  exit 1
fi

# The hardened runtime is not optional for notarization; check it here for
# the same reason, so the failure is immediate and legible.
if ! printf '%s' "$SIGINFO" | grep -q 'flags=.*runtime'; then
  echo "error: $APP is not signed with the hardened runtime." >&2
  echo "bundle.sh adds it automatically for a Developer ID build." >&2
  exit 1
fi

# Two ways to authenticate, because a person and a CI runner want different
# things. An App Store Connect API key is preferred where both are available:
# it belongs to a team rather than a person, and it can be revoked on its own.
#
# NOTARY_KEY_P8 holds the key's contents, not a path, so a workflow can pass it
# straight from a secret without ever writing it to the workspace. It is
# written to a file here because notarytool takes a path, in a directory only
# this user can read, and removed on exit however the script ends.
AUTH=()
if [ -n "${NOTARY_KEY_P8:-}" ]; then
  : "${NOTARY_KEY_ID:?NOTARY_KEY_P8 is set, so NOTARY_KEY_ID must be too}"
  : "${NOTARY_ISSUER_ID:?NOTARY_KEY_P8 is set, so NOTARY_ISSUER_ID must be too}"
  KEYDIR="$(mktemp -d)"
  chmod 700 "$KEYDIR"
  trap 'rm -rf "$KEYDIR"' EXIT
  printf '%s' "$NOTARY_KEY_P8" > "$KEYDIR/key.p8"
  AUTH=(--key "$KEYDIR/key.p8" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER_ID")
  echo "Authenticating with an App Store Connect API key"
elif xcrun notarytool history --keychain-profile "$PROFILE" >/dev/null 2>&1; then
  AUTH=(--keychain-profile "$PROFILE")
  echo "Authenticating with keychain profile '$PROFILE'"
else
  echo "error: no usable notary credentials." >&2
  echo "Either create a keychain profile:" >&2
  echo "  xcrun notarytool store-credentials $PROFILE \\" >&2
  echo "    --apple-id <your-apple-id> --team-id <your-team-id>" >&2
  echo "or set NOTARY_KEY_P8, NOTARY_KEY_ID and NOTARY_ISSUER_ID." >&2
  exit 1
fi

# notarytool does not take a .app directly. ditto rather than zip, to
# preserve the signature and extended attributes the way Apple expects.
rm -f "$ZIP"
/usr/bin/ditto -c -k --keepParent "$APP" "$ZIP"

echo "Submitting to Apple. This usually takes a few minutes."
xcrun notarytool submit "$ZIP" "${AUTH[@]}" --wait

# Staple the ticket into the bundle so it validates with no network. Without
# this the app still passes, but only on a machine that can reach Apple.
xcrun stapler staple "$APP"
rm -f "$ZIP"

echo
echo "Stapled. Verifying the way Gatekeeper will:"
xcrun stapler validate "$APP"
spctl -a -vv -t exec "$APP"
