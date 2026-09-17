#!/bin/bash
# Regenerates resources/<lang>.lproj/Localizable.strings from upstream
# OpenLauncher's .resx resources. Upstream is MIT licensed (LICENSE-openlauncher),
# and the imported translations stay under it rather than this repository's 0BSD;
# the translations
# are carried over verbatim, including their {0} placeholder syntax, so future
# upstream translation updates drop straight in.
#
# The source is a PINNED COMMIT fetched from upstream, not a checkout that
# happens to be sitting next to this one. An earlier version defaulted to
# `../OpenLauncher`, which made the output depend on whatever that directory
# contained: in practice it was a personal fork sitting on a local commit that
# had never been pushed, so nobody else could have reproduced this import at
# all. A pin means anyone can regenerate these files and get the same bytes.
#
# To take newer translations, change UPSTREAM_COMMIT and re-run. Review the
# diff: the pin is the record of which upstream state these strings came from,
# so moving it is a deliberate act rather than a side effect of someone else's
# working directory.
set -euo pipefail

UPSTREAM_REPO="${UPSTREAM_REPO:-https://github.com/OpenRCT2/OpenLauncher.git}"
# Last upstream commit touching src/openlauncher/Properties/, confirmed present
# in OpenRCT2/OpenLauncher rather than only in a fork.
UPSTREAM_COMMIT="${UPSTREAM_COMMIT:-74b4f8b0b7549dd5e06f3393457cf1e49227fe6d}"

OUT="resources"

# An explicit local checkout is still allowed, for working offline or testing
# a translation before it is merged upstream. It is deliberately not the
# default, and it says so, because output from an arbitrary working directory
# is not reproducible by anyone else.
if [ $# -gt 0 ]; then
  SRC="$1/src/openlauncher/Properties"
  echo "warning: importing from local checkout $1 rather than the pinned commit." >&2
  echo "warning: the result is not reproducible from this repository alone." >&2
  if [ ! -d "$SRC" ]; then
    echo "error: no .resx sources at $SRC" >&2
    exit 1
  fi
else
  WORK="$(mktemp -d)"
  trap 'rm -rf "$WORK"' EXIT
  echo "Fetching $UPSTREAM_COMMIT from $UPSTREAM_REPO"
  git init -q "$WORK"
  # Fetching the SHA directly, so a moved branch cannot change what is
  # imported. --depth 1 keeps it to the one commit.
  if ! git -C "$WORK" fetch --depth 1 -q "$UPSTREAM_REPO" "$UPSTREAM_COMMIT"; then
    echo "error: could not fetch $UPSTREAM_COMMIT from $UPSTREAM_REPO" >&2
    echo "Check network access, or pass a local checkout path as \$1." >&2
    exit 1
  fi
  git -C "$WORK" checkout -q FETCH_HEAD
  # Belt and braces: confirm we are on the commit we asked for before
  # overwriting nine translation files from it.
  GOT="$(git -C "$WORK" rev-parse HEAD)"
  if [ "$GOT" != "$UPSTREAM_COMMIT" ]; then
    echo "error: fetched $GOT but expected $UPSTREAM_COMMIT" >&2
    exit 1
  fi
  SRC="$WORK/src/openlauncher/Properties"
fi

python3 - "$SRC" "$OUT" "$OUT/turnstile-strings.json" <<'PY'
import json, os, re, sys, xml.etree.ElementTree as ET

src, out = sys.argv[1], sys.argv[2]

# Keys that describe the launcher itself rather than the games. The product
# name is not translated. The launcher self-update strings are skipped because
# that feature was cut deliberately: this is a separate application, so
# upstream's release feed says nothing about whether THIS app is out of date.
# Two of them also name "OpenLauncher" in their translated text, which has no
# business being shipped inside a differently-named app.
SKIP = {"OpenLauncher", "LauncherUpdateTitle", "LauncherUpdateMessage", "Update"}

# Strings Turnstile adds, plus the upstream keys upstream itself left
# untranslated in some languages. Held in their own file so re-running this
# import never loses them; a hardcoded English-only table here would silently
# overwrite every translation on the next run.
OURS_PATH = sys.argv[3]
with open(OURS_PATH, encoding='utf-8') as f:
    OURS = {k: v for k, v in json.load(f).items() if not k.startswith('_')}

def escape(s):
    return s.replace('\\', '\\\\').replace('"', '\\"').replace('\n', '\\n')

for name in sorted(os.listdir(src)):
    m = re.fullmatch(r'Resources(?:\.([a-z]{2}))?\.resx', name)
    if not m:
        continue
    lang = m.group(1) or 'en'
    pairs = {}
    for data in ET.parse(os.path.join(src, name)).getroot().findall('data'):
        key = data.get('name')
        value = data.findtext('value')
        if key and value is not None and key not in SKIP:
            pairs[key] = value
    # Ours win over the .resx import: for a key upstream left untranslated in
    # this language, upstream has no value at all, and for our own keys it has
    # none either. English falls back through BUILTIN_EN at runtime for
    # anything still missing.
    #
    # That override is unconditional, which is correct today (every key in
    # OURS is one upstream currently has no translation for) but not
    # forever: if a future upstream .resx starts translating one of these
    # keys, this would keep silently discarding it. Warn instead of
    # swallowing that, so a reimport surfaces the conflict for a human to
    # resolve rather than hiding it.
    ours_for_lang = OURS.get(lang, {})
    for key, ours_value in ours_for_lang.items():
        upstream_value = pairs.get(key)
        if upstream_value:
            print(
                f'warning: {lang}: upstream now translates "{key}" as '
                f'{upstream_value!r}, but turnstile-strings.json overrides '
                f'it with {ours_value!r}. Remove the entry from '
                f'turnstile-strings.json to use the upstream translation.',
                file=sys.stderr,
            )
    pairs.update(ours_for_lang)

    d = os.path.join(out, f'{lang}.lproj')
    os.makedirs(d, exist_ok=True)
    with open(os.path.join(d, 'Localizable.strings'), 'w', encoding='utf-8') as f:
        f.write('/* Generated by scripts/import-resx.sh. Do not edit by hand. */\n')
        for key in sorted(pairs):
            f.write(f'"{escape(key)}" = "{escape(pairs[key])}";\n')
    print(f'{lang}: {len(pairs)} strings')
PY
