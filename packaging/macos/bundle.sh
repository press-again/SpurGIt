#!/bin/sh
# Builds target/release/Spur.app and a zip of it.
#   packaging/macos/bundle.sh [zip-path]
# Signs ad-hoc unless CODESIGN_IDENTITY names a Developer ID certificate.
# Notarization is not done here; see the README for the Gatekeeper note.
set -eu
cd "$(dirname "$0")/../.."

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
zip=${1:-target/release/Spur-$version-macos-$(uname -m).zip}
app=target/release/Spur.app

cargo build --release
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/spurgit "$app/Contents/MacOS/spurgit"
cp assets/spur.icns "$app/Contents/Resources/spur.icns"
sed "s/@VERSION@/$version/g" packaging/macos/Info.plist > "$app/Contents/Info.plist"
# The fonts' OFL and the MIT notices must travel with the binary.
cp LICENSE assets/fonts/THIRD_PARTY_NOTICES.md assets/fonts/Geist-OFL.txt "$app/Contents/Resources/"

codesign --force --deep --sign "${CODESIGN_IDENTITY:--}" "$app"
codesign --verify --deep --strict "$app"

rm -f "$zip"
ditto -c -k --keepParent "$app" "$zip"
echo "built $app and $zip"
