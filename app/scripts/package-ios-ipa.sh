#!/usr/bin/env bash
# Sign the wrapper-built release .app and package it as an App Store IPA.
#
# Prereqs (one-time, see plans/release-runbook.md):
#   - "Apple Distribution" certificate in the login keychain (made via Xcode)
#   - The App Store provisioning profile for com.number0.irohdoctor installed
#     at the default path below (or pass a path)
#   - A fresh release build: BUILD_NUMBER=<n> scripts/bundle-mobile.sh ios --release
#
# Signing entitlements are extracted from the provisioning profile itself, so
# the team / app-id prefix can never drift from what the store expects.
#
# Usage:
#   scripts/package-ios-ipa.sh [path-to.mobileprovision] [output.ipa]
set -euo pipefail

PROFILE="${1:-$HOME/Library/MobileDevice/Provisioning Profiles/iroh_doctor_appstore.mobileprovision}"
[[ -f "$PROFILE" ]] || { echo "no provisioning profile at $PROFILE" >&2; exit 2; }

APP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORKSPACE="$(cd "$APP_DIR/.." && pwd)"
APP="$WORKSPACE/target/dx/iroh-doctor-app/release/ios/IrohDoctorApp.app"
[[ -d "$APP" ]] || { echo "no release .app at $APP — run: BUILD_NUMBER=<n> scripts/bundle-mobile.sh ios --release" >&2; exit 1; }

BUILD_NUM="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$APP/Info.plist")"
OUT="${2:-$WORKSPACE/target/iroh-doctor-$BUILD_NUM.ipa}"

# App Store validation requires the DT* toolchain-metadata keys that Xcode
# normally stamps (error 90507); dx 0.7.9's Info.plist template has none of
# them. Derive them from the local toolchain, before signing seals the plist.
SDK_VER="$(xcrun --sdk iphoneos --show-sdk-version)"
SDK_BUILD="$(xcrun --sdk iphoneos --show-sdk-build-version)"
PLATFORM_VER="$(xcrun --sdk iphoneos --show-sdk-platform-version)"
XCODE_VER="$(xcodebuild -version | awk '/^Xcode/ {print $2}')"
XCODE_BUILD="$(xcodebuild -version | awk '/^Build version/ {print $3}')"
DTXCODE="$(echo "$XCODE_VER" | awk -F. '{printf "%d%d%d0", $1, int($2/10), $2%10}')"
PLIST="$APP/Info.plist"
plist_set() { /usr/libexec/PlistBuddy -c "Set :$1 $2" "$PLIST" 2>/dev/null \
  || /usr/libexec/PlistBuddy -c "Add :$1 string $2" "$PLIST"; }
plist_set DTPlatformName iphoneos
plist_set DTPlatformVersion "$PLATFORM_VER"
plist_set DTPlatformBuild "$SDK_BUILD"
plist_set DTSDKName "iphoneos$SDK_VER"
plist_set DTSDKBuild "$SDK_BUILD"
plist_set DTXcode "$DTXCODE"
plist_set DTXcodeBuild "$XCODE_BUILD"
plist_set DTCompiler com.apple.compilers.llvm.clang.1_0
plist_set BuildMachineOSBuild "$(sw_vers -buildVersion)"
# More dx-template gaps the store rejects: the bundle type code must be APPL
# (error 90183), and CFBundleSupportedPlatforms must hold exactly one value —
# dx writes [iPhoneOS, iPadOS] (error 91177). iPad support is unaffected;
# UIDeviceFamily [1,2] governs that.
plist_set CFBundlePackageType APPL
/usr/libexec/PlistBuddy -c "Delete :CFBundleSupportedPlatforms" "$PLIST" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Add :CFBundleSupportedPlatforms array" "$PLIST"
/usr/libexec/PlistBuddy -c "Add :CFBundleSupportedPlatforms:0 string iPhoneOS" "$PLIST"

# The store rejects bundles whose embedded profile doesn't match the signature,
# so the profile is copied in before signing (the signature seals it).
cp "$PROFILE" "$APP/embedded.mobileprovision"

ENT="$(mktemp -t entitlements).plist"
security cms -D -i "$PROFILE" 2>/dev/null | plutil -extract Entitlements xml1 -o "$ENT" -

echo ">> signing as: $(plutil -extract 'application-identifier' raw "$ENT")"
codesign --force --sign "Apple Distribution" --entitlements "$ENT" "$APP"
codesign --verify --strict "$APP"

echo ">> packaging $OUT"
STAGE="$(mktemp -d)"
mkdir -p "$STAGE/Payload"
cp -R "$APP" "$STAGE/Payload/"
rm -f "$OUT"
(cd "$STAGE" && zip -qry "$OUT" Payload)
rm -rf "$STAGE" "$ENT"

echo ">> done: $OUT (build $BUILD_NUM)"
echo "Upload (API key in ~/.appstoreconnect/private_keys is found automatically):"
echo "  xcrun altool --upload-app -f \"$OUT\" -t ios --apiKey <KEY_ID> --apiIssuer <ISSUER_ID>"
