#!/usr/bin/env bash
# Sign the wrapper-built release .app and package it as an App Store IPA.
#
# Prereqs (one-time, see plans/release-runbook.md):
#   - "Apple Distribution" certificate in the login keychain (made via Xcode)
#   - App Store provisioning profile for com.number0.irohdoctor downloaded
#   - A fresh release build: BUILD_NUMBER=<n> scripts/bundle-mobile.sh ios --release
#
# Usage:
#   scripts/package-ios-ipa.sh <path-to.mobileprovision> [output.ipa]
set -euo pipefail

PROFILE="${1:-}"
[[ -f "$PROFILE" ]] || { echo "usage: $0 <path-to.mobileprovision> [output.ipa]" >&2; exit 2; }

APP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORKSPACE="$(cd "$APP_DIR/.." && pwd)"
APP="$WORKSPACE/target/dx/iroh-doctor-app/release/ios/IrohDoctorApp.app"
[[ -d "$APP" ]] || { echo "no release .app at $APP — run: BUILD_NUMBER=<n> scripts/bundle-mobile.sh ios --release" >&2; exit 1; }

BUILD_NUM="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$APP/Info.plist")"
OUT="${2:-$WORKSPACE/target/iroh-doctor-$BUILD_NUM.ipa}"

TEAM_ID="84T7UAWDW5"
BUNDLE_ID="com.number0.irohdoctor"

# The store rejects bundles whose embedded profile doesn't match the signature,
# so the profile is copied in before signing (the signature seals it).
cp "$PROFILE" "$APP/embedded.mobileprovision"

ENT="$(mktemp -t entitlements).plist"
cat > "$ENT" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>application-identifier</key><string>$TEAM_ID.$BUNDLE_ID</string>
  <key>com.apple.developer.team-identifier</key><string>$TEAM_ID</string>
</dict></plist>
EOF

echo ">> codesign (Apple Distribution, team $TEAM_ID)"
codesign --force --sign "Apple Distribution" --entitlements "$ENT" "$APP"
codesign --verify --deep --strict "$APP"

echo ">> packaging $OUT"
STAGE="$(mktemp -d)"
mkdir -p "$STAGE/Payload"
cp -R "$APP" "$STAGE/Payload/"
rm -f "$OUT"
(cd "$STAGE" && zip -qry "$OUT" Payload)
rm -rf "$STAGE" "$ENT"

echo ">> done: $OUT (build $BUILD_NUM)"
echo "Upload via Transporter.app, or with an App Store Connect API key:"
echo "  xcrun altool --upload-app -f \"$OUT\" -t ios --apiKey <KEY_ID> --apiIssuer <ISSUER_ID>"
