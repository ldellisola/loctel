#!/usr/bin/env bash
# Builds loctel.app and loctel-vX.Y.Z.dmg for a release. macOS only: it uses
# the Mac's own codesign, PlistBuddy and hdiutil. The release job runs it
# (.github/workflows/release.yml); nothing on Linux can.
#
# Usage, from anywhere:  script/bundle-mac.sh vX.Y.Z
#
# Needs cargo-bundle 0.12.0 (`cargo install cargo-bundle --version 0.12.0
# --locked`). It leaves, in the repository root:
#
#   loctel.app          the bundle, signed ad hoc
#   loctel-vX.Y.Z.dmg   a compressed disk image holding loctel.app and an
#                       Applications shortcut, to drag it onto
#
# The bundle's contents (name, identifier, icon, LSUIElement, minimum macOS)
# come from [package.metadata.bundle] in crates/loctel-app/Cargo.toml. See
# docs/areas/release.md.
set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: script/bundle-mac.sh vX.Y.Z" >&2
  exit 2
fi
VERSION=$1
if ! [[ $VERSION =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "bundle-mac: '$VERSION' is not a version like v0.1.0" >&2
  exit 2
fi
if [ "$(uname -s)" != "Darwin" ]; then
  echo "bundle-mac: this only runs on macOS" >&2
  exit 1
fi
if ! command -v cargo-bundle >/dev/null 2>&1; then
  echo "bundle-mac: cargo-bundle is not installed;" \
    "run: cargo install cargo-bundle --version 0.12.0 --locked" >&2
  exit 1
fi

cd "$(dirname "${BASH_SOURCE[0]}")/.."

APP=loctel.app
DMG="loctel-$VERSION.dmg"
TARGET_DIR=${CARGO_TARGET_DIR:-target}

# 1. The bundle. `--format osx` is the .app alone: on a Mac cargo-bundle also
#    makes a .dmg by default, and we make ours below. It runs from the
#    repository root, where the icon and plist paths in Cargo.toml start.
cargo bundle --release --format osx -p loctel-app
rm -rf "$APP"
ditto "$TARGET_DIR/release/bundle/osx/$APP" "$APP"

# 2. The version the About box and Finder show: the tag without its `v`.
PLIST="$APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString ${VERSION#v}" "$PLIST"

# The plist must still be well formed after the extra entries went in, and it
# must be an accessory app: without LSUIElement a Dock icon flashes at launch.
plutil -lint "$PLIST"
if [ "$(/usr/libexec/PlistBuddy -c "Print :LSUIElement" "$PLIST")" != "true" ]; then
  echo "bundle-mac: LSUIElement is not true in $PLIST" >&2
  exit 1
fi

# 3. Sign ad hoc, with no certificate (as Zed does). arm64 code must carry a
#    signature to run at all, and a broken one shows up as "damaged".
codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict "$APP"

# 4. The disk image: the app and a shortcut to /Applications.
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
ditto "$APP" "$STAGE/$APP"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
hdiutil create -volname loctel -srcfolder "$STAGE" -ov -format UDZO "$DMG"

echo "bundle-mac: wrote $APP and $DMG"
