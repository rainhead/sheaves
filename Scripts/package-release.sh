#!/usr/bin/env bash
# Builds the Release configuration and packages it the way a download has to
# arrive: a zip made with ditto, which is the only archiver that preserves a
# code signature intact.
#
# CI runs exactly this, so a local run is a real rehearsal of the published
# artifact rather than an approximation of it.
#
#     Scripts/package-release.sh          # -> dist/Sheaves-<version>.zip
set -euo pipefail

cd "$(dirname "$0")/.."

derived=build/release
products=$derived/Build/Products/Release
app=$products/Sheaves.app

xcodegen generate

# Ad-hoc, stated here rather than left to Config/Base.xcconfig's default. CI has
# no Config/Local.xcconfig and would sign ad-hoc anyway, but a developer machine
# has one — and a release carrying a personal "Apple Development" identity is
# worse than an unsigned one: Gatekeeper rejects it for distribution just the
# same, and the certificate expires in a week. Saying it on the command line
# makes the artifact the same wherever it is built.
#
# Ad-hoc is still a real signature, unlike the test job's CODE_SIGNING_ALLOWED=NO
# — an arm64 app with no signature at all will not launch.
xcodebuild \
  -project Sheaves.xcodeproj \
  -scheme Sheaves \
  -configuration Release \
  -destination 'platform=macOS' \
  -derivedDataPath "$derived" \
  CODE_SIGN_STYLE=Manual \
  CODE_SIGN_IDENTITY=- \
  DEVELOPMENT_TEAM= \
  build

# The version the app itself claims, rather than a second copy of it kept here.
version=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")

# A signature that does not verify produces a download that cannot be opened at
# all, which looks exactly like a corrupt upload. Fail here instead.
codesign --verify --deep --strict --verbose=2 "$app"

mkdir -p dist
zip="dist/Sheaves-$version.zip"
rm -f "$zip"
ditto -c -k --keepParent --sequesterRsrc "$app" "$zip"

echo "Packaged $zip"

# Handed to the workflow when there is one, so the release step does not have to
# reconstruct either value.
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  {
    echo "version=$version"
    echo "zip=$zip"
  } >> "$GITHUB_OUTPUT"
fi
