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
