#!/usr/bin/env bash
# Builds dist/Destruct-O-Vision.app and dist/Destruct-O-Vision-<version>.dmg.
#
#   scripts/package-macos.sh               this Mac's architecture
#   UNIVERSAL=1 scripts/package-macos.sh   Apple silicon and Intel in one binary
#   DOV_PREBUILT=1 scripts/package-macos.sh
#       signs and packages the dist/Destruct-O-Vision.app an earlier run built,
#       without building (or running cargo at all), so signing keys never meet
#       code from dependencies
#
# Without credentials the app is ad-hoc signed: it runs on this Mac, but on
# other Macs Gatekeeper blocks it until the user allows it in System Settings
# > Privacy & Security. For distribution, set:
#   DOV_SIGN_ID         "Developer ID Application: Your Name (TEAMID)"
# and to notarize, either
#   DOV_NOTARY_PROFILE  a profile saved with `xcrun notarytool store-credentials`
# or an App Store Connect API key:
#   DOV_NOTARY_KEY      the key's .p8 file
#   DOV_NOTARY_KEY_ID   its Key ID
#   DOV_NOTARY_ISSUER   its Issuer ID
# Notarizing staples the ticket to the app and the DMG, so they open without
# a warning, even offline. DOV_BUNDLE_ID overrides the bundle identifier.
set -euo pipefail
cd "$(dirname "$0")/.."

NAME=Destruct-O-Vision
BIN=destructovision
BUNDLE_ID=${DOV_BUNDLE_ID:-org.destructovision.DestructOVision}
export MACOSX_DEPLOYMENT_TARGET=11.0
app="dist/$NAME.app"

notary=()
if [[ -n "${DOV_NOTARY_KEY:-}" ]]; then
    notary=(--key "$DOV_NOTARY_KEY" --key-id "$DOV_NOTARY_KEY_ID" --issuer "$DOV_NOTARY_ISSUER")
elif [[ -n "${DOV_NOTARY_PROFILE:-}" ]]; then
    notary=(--keychain-profile "$DOV_NOTARY_PROFILE")
fi
if [[ ${#notary[@]} -gt 0 && -z "${DOV_SIGN_ID:-}" ]]; then
    echo "notarizing needs DOV_SIGN_ID" >&2
    exit 1
fi

# Sends `$1` to Apple's notary service and staples the ticket to `$2`. On a
# rejection, prints Apple's log of what was wrong.
notarize() {
    local result id status
    result=$(xcrun notarytool submit "$1" "${notary[@]}" --wait --output-format json)
    id=$(python3 -I -c 'import json, sys; print(json.load(sys.stdin)["id"])' <<<"$result")
    status=$(python3 -I -c 'import json, sys; print(json.load(sys.stdin)["status"])' <<<"$result")
    if [[ "$status" != Accepted ]]; then
        xcrun notarytool log "$id" "${notary[@]}" >&2
        echo "notarizing $1: $status" >&2
        exit 1
    fi
    xcrun stapler staple "$2"
}

if [[ "${DOV_PREBUILT:-0}" == 1 ]]; then
    VERSION=$(plutil -extract CFBundleShortVersionString raw "$app/Contents/Info.plist")
else
    VERSION=$(cargo metadata --no-deps --format-version 1 |
        python3 -I -c 'import json, sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "dov-app"))')
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

    rm -rf "$app"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    cp "$built" "$app/Contents/MacOS/$BIN"
    sed -e "s/@VERSION@/$VERSION/g" -e "s/@BUNDLE_ID@/$BUNDLE_ID/g" packaging/macos/Info.plist >"$app/Contents/Info.plist"
    plutil -lint -s "$app/Contents/Info.plist"
    cp packaging/macos/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
    cp LICENSE "$app/Contents/Resources/LICENSE.txt"
    cp LICENSE-MIT "$app/Contents/Resources/LICENSE-MIT.txt"
    python3 -I scripts/third_party_licenses.py "${targets[@]}" >"$app/Contents/Resources/THIRD_PARTY_LICENSES.txt"
fi

if [[ -n "${DOV_SIGN_ID:-}" ]]; then
    codesign --force --timestamp --options runtime \
        --entitlements packaging/macos/entitlements.plist --sign "$DOV_SIGN_ID" "$app"
else
    codesign --force --sign - "$app"
fi
codesign --verify --strict "$app"
if [[ ${#notary[@]} -gt 0 ]]; then
    zip="dist/$NAME.zip"
    ditto -c -k --keepParent "$app" "$zip"
    notarize "$zip" "$app"
    rm "$zip"
    spctl --assess --type execute -vv "$app"
fi

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
if [[ ${#notary[@]} -gt 0 ]]; then
    notarize "$dmg" "$dmg"
fi
echo "$dmg"
