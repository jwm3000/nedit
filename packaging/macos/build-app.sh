#!/bin/sh
# Build nEdit.app for macOS – one universal binary for Apple Silicon and Intel – plus
#   nEdit-macOS.dmg         to install (drag nEdit into Applications)
#   nEdit-macOS.zip         the zipped app, used by nEdit's built-in updater
#   nedit-macos-universal   the bare binary (command line use, older updaters)
#
# Run on macOS from the repository root:  packaging/macos/build-app.sh [outdir]
set -eu

OUT=${1:-dist}
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
export MACOSX_DEPLOYMENT_TARGET=11.0

rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo build --release --target aarch64-apple-darwin
cargo build --release --target x86_64-apple-darwin

mkdir -p "$OUT"
APP="$OUT/nEdit.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create -output "$APP/Contents/MacOS/nedit" \
  target/aarch64-apple-darwin/release/nedit \
  target/x86_64-apple-darwin/release/nedit
sed "s/@VERSION@/$VERSION/g" packaging/macos/Info.plist > "$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist"
cp assets/nedit.icns "$APP/Contents/Resources/nedit.icns"
printf 'APPL????' > "$APP/Contents/PkgInfo"

# Ad-hoc signature (no Apple developer account): required for Apple Silicon and keeps the
# bundle consistent. Without notarization macOS asks once before the first start.
codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"
lipo -info "$APP/Contents/MacOS/nedit"

(cd "$OUT" && rm -f nEdit-macOS.zip && ditto -c -k --norsrc --keepParent nEdit.app nEdit-macOS.zip)
cp "$APP/Contents/MacOS/nedit" "$OUT/nedit-macos-universal"

# Disk image: the app next to a link to /Applications
STAGE=$(mktemp -d)
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$OUT/nEdit-macOS.dmg"
hdiutil create -volname "nEdit $VERSION" -srcfolder "$STAGE" -ov -format UDZO "$OUT/nEdit-macOS.dmg"
rm -rf "$STAGE"

ls -la "$OUT"
