#!/bin/bash
# Sets the three notarization secrets on the release environment from an App
# Store Connect API key.
#
#   ./scripts/set-notary-secrets.sh path/to/AuthKey_XXXXXXXXXX.p8
#
# The credentials are tried against Apple before anything is uploaded, by
# asking the notary service for this key's submission history. A key that is
# wrong, revoked, or lacking the role otherwise surfaces several minutes into
# a release, after a universal build and a disk image have already been made.
#
# The key is piped straight to gh. It is never copied elsewhere on disk.
#
# An App Store Connect key rather than an Apple ID and app-specific password:
# it belongs to the team rather than a person, can be revoked on its own, and
# does not stop working when somebody changes their password or leaves.
set -euo pipefail

P8="${1:?usage: set-notary-secrets.sh path/to/AuthKey_XXXXXXXXXX.p8}"
REPO="${REPO:-northbymidwest/turnstile}"
ENVIRONMENT="${ENVIRONMENT:-release}"

[ -f "${P8}" ] || {
  echo "error: no such file: ${P8}" >&2
  exit 1
}
command -v gh >/dev/null || {
  echo "error: gh is required" >&2
  exit 1
}
xcrun notarytool --version >/dev/null 2>&1 || {
  echo "error: notarytool is required (Xcode or the Command Line Tools)" >&2
  exit 1
}

# The key id is in the filename Apple gives the download, so offer it rather
# than making somebody retype ten characters they cannot check by eye.
suggested=$(basename "${P8}" .p8 | sed -n 's/^AuthKey_//p')
if [ -n "${suggested}" ]; then
  printf 'Key ID [%s]: ' "${suggested}" >&2
  read -r KEY_ID
  KEY_ID="${KEY_ID:-${suggested}}"
else
  printf 'Key ID (10 characters): ' >&2
  read -r KEY_ID
fi
[ -n "${KEY_ID}" ] || {
  echo "error: no key id given" >&2
  exit 1
}

printf 'Issuer ID (the UUID above the key list): ' >&2
read -r ISSUER_ID
[ -n "${ISSUER_ID}" ] || {
  echo "error: no issuer id given" >&2
  exit 1
}

echo "Checking the credentials against Apple..." >&2
if ! xcrun notarytool history --key "${P8}" --key-id "${KEY_ID}" --issuer "${ISSUER_ID}" >/dev/null 2>&1; then
  echo "error: Apple rejected these credentials." >&2
  echo "Check the key id and issuer id, that the key has not been revoked," >&2
  echo "and that it has at least the Developer role." >&2
  exit 1
fi
echo "Accepted." >&2

echo
echo "Repository: ${REPO}, environment ${ENVIRONMENT}"
printf 'Set NOTARY_KEY_P8, NOTARY_KEY_ID and NOTARY_ISSUER_ID? [y/N] '
read -r reply
case "${reply}" in
  y | Y) ;;
  *)
    echo "Nothing was uploaded."
    exit 0
    ;;
esac

gh secret set NOTARY_KEY_P8 --env "${ENVIRONMENT}" --repo "${REPO}" <"${P8}"
printf '%s' "${KEY_ID}" | gh secret set NOTARY_KEY_ID --env "${ENVIRONMENT}" --repo "${REPO}"
printf '%s' "${ISSUER_ID}" | gh secret set NOTARY_ISSUER_ID --env "${ENVIRONMENT}" --repo "${REPO}"

echo
echo "Set. Keep the .p8 somewhere safe: Apple lets it be downloaded only once."
