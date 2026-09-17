#!/bin/bash
# Sets the two signing secrets on the release environment from a .p12 export.
#
#   ./scripts/set-signing-secrets.sh [path/to/cert.p12]
#
# Everything is checked locally before anything is uploaded: that the password
# opens the file, that it holds a Developer ID Application certificate, and
# that the certificate has not expired. A wrong password otherwise becomes a
# failure several minutes into a release, in a job that has already imported a
# keychain, rather than an error here.
#
# The encoded certificate is piped straight to gh. It is never written to a
# file, never put on the clipboard, and the password is never echoed.
set -euo pipefail

P12="${1:-${HOME}/Documents/Certificates.p12}"
REPO="${REPO:-northbymidwest/turnstile}"
ENVIRONMENT="${ENVIRONMENT:-release}"

[ -f "${P12}" ] || {
  echo "error: no such file: ${P12}" >&2
  echo "Pass the path to your .p12 export as the first argument." >&2
  exit 1
}

command -v gh >/dev/null || {
  echo "error: gh is required" >&2
  exit 1
}
command -v openssl >/dev/null || {
  echo "error: openssl is required" >&2
  exit 1
}

printf 'Password for %s: ' "${P12}" >&2
read -rs PASSWORD
printf '\n' >&2
[ -n "${PASSWORD}" ] || {
  echo "error: no password given" >&2
  exit 1
}

# OpenSSL 3 refuses the older ciphers Keychain Access still exports with, so
# fall back to -legacy rather than reporting a good password as wrong.
certs=""
if certs=$(openssl pkcs12 -in "${P12}" -passin pass:"${PASSWORD}" -nokeys -clcerts 2>/dev/null); then
  :
elif certs=$(openssl pkcs12 -legacy -in "${P12}" -passin pass:"${PASSWORD}" -nokeys -clcerts 2>/dev/null); then
  :
else
  echo "error: could not open ${P12} with that password" >&2
  exit 1
fi
[ -n "${certs}" ] || {
  echo "error: ${P12} opened but holds no certificate" >&2
  exit 1
}

subject=$(printf '%s' "${certs}" | openssl x509 -noout -subject 2>/dev/null || true)
case "${subject}" in
  *"Developer ID Application"*) ;;
  *)
    echo "error: this is not a Developer ID Application certificate." >&2
    echo "  ${subject}" >&2
    echo "Export the one named 'Developer ID Application: ...' from Keychain Access." >&2
    exit 1
    ;;
esac

# An expired certificate imports and signs, and the notary service then
# rejects the result. Better to hear it now.
if ! printf '%s' "${certs}" | openssl x509 -noout -checkend 0 >/dev/null 2>&1; then
  echo "error: that certificate has expired." >&2
  printf '%s\n' "${certs}" | openssl x509 -noout -enddate >&2
  exit 1
fi

name=$(printf '%s' "${subject}" | sed -n 's/.*CN *= *\([^,]*\).*/\1/p')
expiry=$(printf '%s' "${certs}" | openssl x509 -noout -enddate | sed 's/notAfter=//')
echo "Certificate: ${name}"
echo "Expires:     ${expiry}"
echo "Repository:  ${REPO}, environment ${ENVIRONMENT}"
printf 'Set MACOS_CERT_P12 and MACOS_CERT_PASSWORD? [y/N] '
read -r reply
case "${reply}" in
  y | Y) ;;
  *)
    echo "Nothing was uploaded."
    exit 0
    ;;
esac

base64 -i "${P12}" | gh secret set MACOS_CERT_P12 --env "${ENVIRONMENT}" --repo "${REPO}"
printf '%s' "${PASSWORD}" | gh secret set MACOS_CERT_PASSWORD --env "${ENVIRONMENT}" --repo "${REPO}"

echo
echo "Set. Still needed, from an App Store Connect API key:"
echo "  gh secret set NOTARY_KEY_P8    --env ${ENVIRONMENT} --repo ${REPO} < AuthKey_XXXXXXXXXX.p8"
echo "  gh secret set NOTARY_KEY_ID    --env ${ENVIRONMENT} --repo ${REPO}"
echo "  gh secret set NOTARY_ISSUER_ID --env ${ENVIRONMENT} --repo ${REPO}"
