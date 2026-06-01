#!/usr/bin/env bash
# Build a mobile bundle with the real iroh doctor branding.
#
# Why this exists: dx 0.7.9 regenerates the native iOS/Android projects from
# templates on every `dx build` and has no config hook for the app icon or the
# on-device display name (both verified — see plans/app-store-release-design.md).
# This wrapper runs `dx build`, then injects the branding into the freshly
# generated project before producing the final artifact.
#
# Usage:
#   scripts/bundle-mobile.sh ios     [--release]
#   scripts/bundle-mobile.sh android [--release]
#
# iOS  : compiles assets/icon/ios/Assets.xcassets with actool into the .app,
#        merges the icon keys + display name into Info.plist, and copies in the
#        privacy manifest. The .app is then ready to sign/archive.
# Android: overwrites the launcher mipmaps + adaptive icon + app_name in the
#        generated project, then runs Gradle directly so the APK carries them.
#
# Android env (auto-defaulted to a standard macOS Android Studio install; export
# to override): JAVA_HOME, ANDROID_HOME, ANDROID_NDK_HOME.
set -euo pipefail

PLATFORM="${1:-}"
PROFILE_FLAG="${2:-}"
case "$PLATFORM" in ios|android) ;; *) echo "usage: $0 <ios|android> [--release]" >&2; exit 2;; esac

PROFILE="debug"
DX_RELEASE=""
if [[ "$PROFILE_FLAG" == "--release" ]]; then PROFILE="release"; DX_RELEASE="--release"; fi

APP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORKSPACE="$(cd "$APP_DIR/.." && pwd)"
ICON_DIR="$APP_DIR/assets/icon"
OUT="$WORKSPACE/target/dx/iroh-doctor-app/$PROFILE/$PLATFORM"
DISPLAY_NAME="iroh doctor"

# Android toolchain env (needed by `dx build --android` too, so set it first).
# Auto-defaulted to a standard macOS Android Studio install; export to override.
if [[ "$PLATFORM" == "android" ]]; then
  export JAVA_HOME="${JAVA_HOME:-/Applications/Android Studio.app/Contents/jbr/Contents/Home}"
  export ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
  export ANDROID_SDK_ROOT="${ANDROID_SDK_ROOT:-$ANDROID_HOME}"
  export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$(ls -d "$ANDROID_HOME"/ndk/* 2>/dev/null | sort | tail -1)}"
  export PATH="$JAVA_HOME/bin:$PATH"
fi

echo ">> dx build --$PLATFORM $PROFILE"
# shellcheck disable=SC2086  # $DX_RELEASE is "" or "--release"; intentional split
( cd "$APP_DIR" && dx build "--$PLATFORM" $DX_RELEASE )

if [[ "$PLATFORM" == "ios" ]]; then
  APP_BUNDLE="$OUT/IrohDoctorApp.app"
  [[ -d "$APP_BUNDLE" ]] || { echo "no .app at $APP_BUNDLE" >&2; exit 1; }
  TMP="$(mktemp -d)"

  echo ">> actool: compiling app icon into the .app"
  xcrun actool "$ICON_DIR/ios/Assets.xcassets" \
    --compile "$APP_BUNDLE" \
    --app-icon AppIcon \
    --platform iphoneos \
    --minimum-deployment-target 13.0 \
    --output-partial-info-plist "$TMP/icon-info.plist" \
    --errors --warnings >/dev/null

  echo ">> merging icon keys + display name into Info.plist"
  /usr/libexec/PlistBuddy -c "Merge $TMP/icon-info.plist" "$APP_BUNDLE/Info.plist"
  /usr/libexec/PlistBuddy -c "Set :CFBundleDisplayName $DISPLAY_NAME" "$APP_BUNDLE/Info.plist"
  /usr/libexec/PlistBuddy -c "Set :CFBundleName $DISPLAY_NAME" "$APP_BUNDLE/Info.plist"

  echo ">> copying privacy manifest"
  cp "$APP_DIR/ios/PrivacyInfo.xcprivacy" "$APP_BUNDLE/PrivacyInfo.xcprivacy"

  plutil -lint "$APP_BUNDLE/Info.plist" >/dev/null
  echo ">> branded iOS bundle ready: $APP_BUNDLE (sign/archive to ship)"
  exit 0
fi

# Android ------------------------------------------------------------------
PROJ="$OUT/app"
RES="$PROJ/app/src/main/res"
[[ -d "$RES" ]] || { echo "no generated res at $RES" >&2; exit 1; }

echo ">> injecting launcher icons"
for d in mdpi hdpi xhdpi xxhdpi xxxhdpi; do
  rm -f "$RES/mipmap-$d/ic_launcher.webp"
  cp "$ICON_DIR/android/mipmap-$d/ic_launcher.png" "$RES/mipmap-$d/ic_launcher.png"
  cp "$ICON_DIR/android/mipmap-$d/ic_launcher_foreground.png" "$RES/mipmap-$d/ic_launcher_foreground.png"
done
cp "$ICON_DIR/android/mipmap-anydpi-v26/ic_launcher.xml" "$RES/mipmap-anydpi-v26/ic_launcher.xml"

echo ">> setting adaptive-icon background color + app name"
# Drop the brand-purple background color as its own values file (the generated
# colors.xml has no ic_launcher_background, so there is no conflict to merge).
cp "$ICON_DIR/android/values/colors.xml" "$RES/values/ic_launcher_background.xml"
printf '<resources>\n    <string name="app_name">%s</string>\n</resources>\n' "$DISPLAY_NAME" > "$RES/values/strings.xml"

GRADLE_TASK="assembleDebug"
[[ "$PROFILE" == "release" ]] && GRADLE_TASK="assembleRelease"
echo ">> gradle $GRADLE_TASK (branded APK)"
( cd "$PROJ" && ./gradlew "$GRADLE_TASK" )

echo ">> branded Android APK(s):"
find "$PROJ/app/build/outputs/apk" -name "*.apk" 2>/dev/null || true
