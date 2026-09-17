#!/bin/bash
# Builds a universal Turnstile.app, signed with a Developer ID identity when
# one is available and ad-hoc otherwise.
#
# Signing identity: set SIGN_IDENTITY to choose one explicitly. Unset, the
# script takes the only Developer ID Application identity in the keychain, and
# falls back to ad-hoc if there is none. The fallback is deliberate: a
# contributor without the certificate must still be able to build and run,
# and an ad-hoc build is perfectly good for that. It is not good for giving to
# anyone else, which is why the script says which one it produced.
#
# A Developer ID build also gets the hardened runtime and a secure timestamp,
# because the notary service requires both. Timestamping contacts Apple's
# timestamp server, so a Developer ID build needs network; ad-hoc does not.
#
# No entitlements file: upstream needed com.apple.security.cs.allow-jit and
# disable-library-validation only to get CoreCLR running under the hardened
# runtime. A Rust binary needs neither, and this app's two subprocesses
# (/usr/bin/ditto for extraction, and the game itself) are plain execs that
# the hardened runtime does not restrict. Verified rather than assumed, by
# signing a probe with the identical options and running it: both the ditto
# extraction and a detached child with piped stdio work.
#
# No codesign --deep: there are no nested binaries to sign, and Apple has
# deprecated the flag.
set -euo pipefail

APP="dist/Turnstile.app"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
# sed exits 0 when it matches nothing, and so does head, so set -e cannot see
# this fail. Without the guard a reorganised Cargo.toml would silently ship a
# bundle whose CFBundleShortVersionString is the empty string.
if [ -z "$VERSION" ]; then
  echo "error: no version found in Cargo.toml" >&2
  exit 1
fi
export MACOSX_DEPLOYMENT_TARGET=11.0

# Both Apple targets are declared in rust-toolchain.toml, so rustup installs
# them on demand and this needs no check of its own. Doing it here instead
# would also break a non-rustup toolchain, where `rustup target add` does not
# exist but the build is otherwise fine.
echo "Building Turnstile $VERSION for both architectures"
cargo build --release -p turnstile --target aarch64-apple-darwin
cargo build --release -p turnstile --target x86_64-apple-darwin

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

lipo -create \
  target/aarch64-apple-darwin/release/turnstile \
  target/x86_64-apple-darwin/release/turnstile \
  -output "$APP/Contents/MacOS/turnstile"

sed "s/APP_VERSION/$VERSION/g" resources/Info.plist > "$APP/Contents/Info.plist"
# Compile the Icon Composer document into Assets.car, which is what carries
# the Liquid Glass icon and its dark, tinted and clear appearances on macOS 26
# and later. actool takes the .icon directly; no .xcassets wrapper is needed.
#
# Optional, because actool ships with Xcode and a contributor may only have the
# command line tools. Without it the app still gets its icon from AppIcon.icns,
# just without the appearance variants.
# Every icon in the bundle comes from actool compiling resources/AppIcon.icon.
# It writes two things: Assets.car, which carries the Liquid Glass icon and its
# dark, tinted and clear appearances on macOS 26 and later, and an AppIcon.icns
# for older systems. The .icon document is the single source; there is no
# hand-built icns to keep in step with it.
#
# actool ships with Xcode. Without it the bundle gets no icon at all, so this
# warns rather than failing: the app still builds and runs, it just shows the
# generic application icon.
ACTOOL="$(xcode-select -p 2>/dev/null)/usr/bin/actool"
if [ -x "$ACTOOL" ]; then
  echo "Compiling AppIcon.icon with actool"
  # --standalone-icon-behavior all is what makes the loose AppIcon.icns carry
  # every size up to 1024 rather than actool's default handful topping out at
  # 256. Without it a 512pt Finder icon is upscaled and visibly soft on any
  # system old enough to be reading the icns rather than Assets.car.
  "$ACTOOL" --compile "$APP/Contents/Resources" \
    --platform macosx --minimum-deployment-target 11.0 \
    --app-icon AppIcon --standalone-icon-behavior all \
    --output-partial-info-plist "$APP/Contents/Resources/.icon.plist" \
    --errors --warnings resources/AppIcon.icon >/dev/null
  rm -f "$APP/Contents/Resources/.icon.plist"
  # CFBundleIconName is what points at the icon inside Assets.car.
  /usr/libexec/PlistBuddy -c "Add :CFBundleIconName string AppIcon" "$APP/Contents/Info.plist"
else
  echo "warning: actool not found, so this build has NO icon." >&2
  echo "warning: install Xcode to get one; the Command Line Tools alone are" >&2
  echo "warning: not enough, because actool is not part of them." >&2
fi

cp resources/icon-openrct2.png resources/icon-openloco.png "$APP/Contents/Resources/"

# Both licenses travel inside the bundle. The game icons and the translations
# come from OpenLauncher under MIT, which requires its notice to be included
# in all copies, and a .app handed to somebody is a copy. Turnstile's own
# license goes in beside it so the bundle says what it is on its own terms.
cp LICENSE "$APP/Contents/Resources/LICENSE"
cp LICENSE-OpenLauncher "$APP/Contents/Resources/LICENSE-OpenLauncher"
for lproj in resources/*.lproj; do
  cp -R "$lproj" "$APP/Contents/Resources/"
done

# `|| true` because grep exits 1 on no match, which set -e would take as fatal
# when having no certificate is a supported case.
IDENTITY="${SIGN_IDENTITY:-$(security find-identity -v -p codesigning 2>/dev/null \
  | grep 'Developer ID Application' | head -1 | sed 's/.*"\(.*\)"/\1/' || true)}"

if [ -n "$IDENTITY" ]; then
  echo "Signing as: $IDENTITY"
  codesign --sign "$IDENTITY" --options runtime --timestamp --force "$APP"
else
  echo "No Developer ID Application identity found; signing ad-hoc."
  echo "This build will be refused by Gatekeeper on any other Mac."
  codesign --sign - --force "$APP"
fi

echo "Built $APP"
lipo -info "$APP/Contents/MacOS/turnstile"
codesign --verify --verbose=2 "$APP"

# What was actually produced, rather than what was intended. An ad-hoc
# signature reports `Signature=adhoc` and no TeamIdentifier.
codesign -dv --verbose=4 "$APP" 2>&1 | grep -E '^(Signature|TeamIdentifier|Authority)=' || true
