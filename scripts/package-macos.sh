#!/usr/bin/env bash
# Builds dist/Destruct-O-Vision.app and dist/Destruct-O-Vision-<version>.dmg.
#
#   scripts/package-macos.sh               this Mac's architecture
#   UNIVERSAL=1 scripts/package-macos.sh   Apple silicon and Intel in one binary
#
# Without credentials the app is ad-hoc signed: it runs on this Mac, but on
# other Macs Gatekeeper blocks it until the user allows it in System Settings
# > Privacy & Security. For distribution, set:
#   DOV_SIGN_ID         "Developer ID Application: Your Name (TEAMID)"
#   DOV_NOTARY_PROFILE  a profile saved with `xcrun notarytool store-credentials`
# DOV_BUNDLE_ID overrides the bundle identifier.
set -euo pipefail
cd "$(dirname "$0")/.."

NAME=Destruct-O-Vision
BIN=destructovision
BUNDLE_ID=${DOV_BUNDLE_ID:-org.destructovision.DestructOVision}
VERSION=$(cargo metadata --no-deps --format-version 1 |
    python3 -I -c 'import json, sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "dov-app"))')
export MACOSX_DEPLOYMENT_TARGET=11.0

if [[ "${UNIVERSAL:-0}" == 1 ]]; then
    targets=(aarch64-apple-darwin x86_64-apple-darwin)
    binaries=()
    for target in "${targets[@]}"; do
        rustup target add "$target" >/dev/null
        cargo build --release --locked -p dov-app --target "$target"
        binaries+=("target/$target/release/$BIN")
    done
    mkdir -p target/universal
    lipo -create -output "target/universal/$BIN" "${binaries[@]}"
    built="target/universal/$BIN"
else
    targets=("$(rustc -vV | sed -n 's/^host: //p')")
    cargo build --release --locked -p dov-app
    built="target/release/$BIN"
fi

app="dist/$NAME.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$built" "$app/Contents/MacOS/$BIN"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@BUNDLE_ID@/$BUNDLE_ID/g" packaging/macos/Info.plist >"$app/Contents/Info.plist"
plutil -lint -s "$app/Contents/Info.plist"
cp packaging/macos/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
cp LICENSE "$app/Contents/Resources/LICENSE.txt"
cp LICENSE-MIT "$app/Contents/Resources/LICENSE-MIT.txt"
python3 -I scripts/third_party_licenses.py "${targets[@]}" >"$app/Contents/Resources/THIRD_PARTY_LICENSES.txt"

if [[ -n "${DOV_SIGN_ID:-}" ]]; then
    codesign --force --timestamp --options runtime \
        --entitlements packaging/macos/entitlements.plist --sign "$DOV_SIGN_ID" "$app"
else
    codesign --force --sign - "$app"
fi
codesign --verify --strict "$app"

dmg="dist/$NAME-$VERSION.dmg"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
cp LICENSE "$stage/LICENSE.txt"
sed "s/@VERSION@/$VERSION/" packaging/macos/README.txt >"$stage/README.txt"
cp -R examples/modules "$stage/Example Modules"
rm -f "$dmg"
hdiutil create -quiet -volname "$NAME $VERSION" -srcfolder "$stage" -format UDZO "$dmg"

if [[ -n "${DOV_SIGN_ID:-}" ]]; then
    codesign --force --timestamp --sign "$DOV_SIGN_ID" "$dmg"
fi
if [[ -n "${DOV_NOTARY_PROFILE:-}" ]]; then
    xcrun notarytool submit "$dmg" --keychain-profile "$DOV_NOTARY_PROFILE" --wait
    xcrun stapler staple "$dmg"
fi
echo "$dmg"
