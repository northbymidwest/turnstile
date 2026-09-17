#!/bin/sh
# Every tracked text file must be pure ASCII, except the translations.
#
# No em dashes, en dashes, ellipses, arrows, or other typographic characters in
# prose or code: each has an ASCII spelling that reads as well (`-` or a comma
# for a dash, `...`, `->`, `x`, `us`), and mixing the two is worse than either.
#
# Two kinds of file are exempt, and have to be. resources/*.lproj and
# resources/turnstile-strings.json hold nine languages, and Catalan, Czech,
# German, Spanish, French, Hungarian, Korean and Dutch are not expressible in
# ASCII. crates/core/tests/fixtures holds captured GitHub API responses, whose
# whole value is being byte for byte what the API returned, typographic quotes
# in OpenRCT2's own changelog included.
#
# The rule is about this project's own writing, not about data it carries.
#
#   scripts/check-ascii.sh              # every tracked file
#   scripts/check-ascii.sh FILE...      # just these
#
# LC_ALL=C makes grep match bytes rather than characters, so every byte of a
# UTF-8 sequence falls outside printable ASCII and the negated class catches
# it. `-I` skips binary files; `/dev/null` is a second argument so grep prints
# the file name even when given exactly one. The class is a literal range
# (tab, then space through tilde), not a POSIX class, so it is portable across
# BSD grep, GNU grep, and drop-in replacements.
set -eu
here=$(dirname "$0")
cd "${here}/.."

exempt() {
  case "$1" in
    resources/*.lproj/* | resources/turnstile-strings.json) return 0 ;;
    crates/core/tests/fixtures/*) return 0 ;;
    *) return 1 ;;
  esac
}

if [ "$#" -eq 0 ]; then
  # Capture the file list on its own line so a git failure stops the script
  # rather than being masked into an empty (falsely passing) argument list.
  files=$(git ls-files)
  [ -n "${files}" ] || exit 0
  IFS='
'
  # shellcheck disable=SC2086 # split the newline-separated list into arguments
  set -- ${files}
fi

checked=""
for f in "$@"; do
  exempt "${f}" && continue
  [ -f "${f}" ] || continue
  checked="${checked}${f}
"
done
[ -n "${checked}" ] || exit 0

tab=$(printf '\t')
IFS='
'
# shellcheck disable=SC2086 # split the newline-separated list into arguments
set -- ${checked}
found=$(LC_ALL=C grep -n -I "[^${tab} -~]" "$@" /dev/null || true)

if [ -n "${found}" ]; then
  printf '%s\n' "${found}" >&2
  printf '\nNon-ASCII above. This repository is ASCII only outside the\n' >&2
  printf 'translations; see the header of %s.\n' "scripts/check-ascii.sh" >&2
  exit 1
fi
