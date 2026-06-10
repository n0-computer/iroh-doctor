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
# iOS  : compiles assets/icon/ios/Assets.xcassets (app icon + launch mark) with
#        actool into the .app, compiles the launch storyboard with ibtool,
#        merges the icon keys + display name into Info.plist, sets the build
#        number when BUILD_NUMBER is set, and copies in the privacy manifest.
#        The .app is then ready to sign/archive.
# Android: overwrites the launcher mipmaps + adaptive icon + app_name in the
#        generated project, adds the Android 12+ splash theme, sets versionCode
#        when BUILD_NUMBER is set, then runs Gradle: assemble* always, and
#        bundleRelease (the Play AAB) on a release build.
#
# Env (auto-defaulted; export to override): for Android, JAVA_HOME,
# ANDROID_HOME, ANDROID_NDK_HOME. BUILD_NUMBER (a positive integer) sets the
# store build number on both platforms; see the version strategy in
# plans/app-store-release-design.md.
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
SPLASH_DIR="$APP_DIR/assets/splash"
OUT="$WORKSPACE/target/dx/iroh-doctor-app/$PROFILE/$PLATFORM"
DISPLAY_NAME="iroh doctor"
# Store build number (CFBundleVersion / versionCode). dx stamps the crate
# version into the marketing fields but hardcodes versionCode=1 and reuses
# the crate version as CFBundleVersion, both of which the stores reject on
# re-upload. Set BUILD_NUMBER to the next monotonic integer for every store
# upload (see plans/app-store-release-design.md, version strategy). Unset =
# leave dx's defaults, fine for local debug builds.
BUILD_NUMBER="${BUILD_NUMBER:-}"
if [[ -n "$BUILD_NUMBER" && ! "$BUILD_NUMBER" =~ ^[1-9][0-9]*$ ]]; then
  echo "BUILD_NUMBER must be a positive integer, got '$BUILD_NUMBER'" >&2
  exit 2
fi

# Android toolchain env (needed by `dx build --android` too, so set it first).
# Auto-defaulted to a standard macOS Android Studio install; export to override.
if [[ "$PLATFORM" == "android" ]]; then
  export JAVA_HOME="${JAVA_HOME:-/Applications/Android Studio.app/Contents/jbr/Contents/Home}"
  export ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
  export ANDROID_SDK_ROOT="${ANDROID_SDK_ROOT:-$ANDROID_HOME}"
  export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$(ls -d "$ANDROID_HOME"/ndk/* 2>/dev/null | sort | tail -1)}"
  export PATH="$JAVA_HOME/bin:$PATH"
fi

# Make the wrapper idempotent across re-runs. dx regenerates its own
# `ic_launcher.webp` on every build but does not remove the `ic_launcher.png`
# this wrapper injected on a previous run, so a second build would have both
# and dx's apk assembly fails with "Duplicate resources". Remove our prior
# PNG injections before `dx build` so dx assembles against only its `.webp`;
# the wrapper re-injects the PNGs (and removes the `.webp`) afterwards.
if [[ "$PLATFORM" == "android" ]]; then
  PREV_RES="$OUT/app/app/src/main/res"
  if [[ -d "$PREV_RES" ]]; then
    for d in mdpi hdpi xhdpi xxhdpi xxxhdpi; do
      rm -f "$PREV_RES/mipmap-$d/ic_launcher.png" \
            "$PREV_RES/mipmap-$d/ic_launcher_foreground.png"
    done
  fi
fi

echo ">> dx build --$PLATFORM $PROFILE"
# iOS: plain `dx build --ios` targets the SIMULATOR (LC_BUILD_VERSION
# platform 7, no LC_ENCRYPTION_INFO — App Store validation rejects it with
# error 90125). Pin the device triple so wrapper builds are always store-able.
DX_TARGET=""
[[ "$PLATFORM" == "ios" ]] && DX_TARGET="--target aarch64-apple-ios"
# shellcheck disable=SC2086  # $DX_RELEASE/$DX_TARGET word-split intentionally
( cd "$APP_DIR" && dx build "--$PLATFORM" $DX_RELEASE $DX_TARGET )

if [[ "$PLATFORM" == "ios" ]]; then
  APP_BUNDLE="$OUT/IrohDoctorApp.app"
  [[ -d "$APP_BUNDLE" ]] || { echo "no .app at $APP_BUNDLE" >&2; exit 1; }
  TMP="$(mktemp -d)"

  echo ">> actool: compiling app icon into the .app"
  xcrun actool "$ICON_DIR/ios/Assets.xcassets" \
    --compile "$APP_BUNDLE" \
    --app-icon AppIcon \
    --platform iphoneos \
    --minimum-deployment-target 17.0 \
    --output-partial-info-plist "$TMP/icon-info.plist" \
    --errors --warnings >/dev/null

  echo ">> merging icon keys + display name into Info.plist"
  /usr/libexec/PlistBuddy -c "Merge $TMP/icon-info.plist" "$APP_BUNDLE/Info.plist"
  /usr/libexec/PlistBuddy -c "Set :CFBundleDisplayName $DISPLAY_NAME" "$APP_BUNDLE/Info.plist"
  /usr/libexec/PlistBuddy -c "Set :CFBundleName $DISPLAY_NAME" "$APP_BUNDLE/Info.plist"
  if [[ -n "$BUILD_NUMBER" ]]; then
    echo ">> setting CFBundleVersion = $BUILD_NUMBER"
    /usr/libexec/PlistBuddy -c "Set :CFBundleVersion $BUILD_NUMBER" "$APP_BUNDLE/Info.plist"
  fi

  echo ">> ibtool: compiling launch screen storyboard"
  # dx 0.7.9's Info.plist template sets UILaunchStoryboardName=LaunchScreen but
  # generates no storyboard, so without this the launch screen is blank.
  xcrun ibtool "$SPLASH_DIR/ios/LaunchScreen.storyboard" \
    --compile "$APP_BUNDLE/LaunchScreen.storyboardc" \
    --errors --warnings >/dev/null
  # Defensive: make sure the plist points at the storyboard we just compiled.
  /usr/libexec/PlistBuddy -c "Set :UILaunchStoryboardName LaunchScreen" "$APP_BUNDLE/Info.plist" 2>/dev/null \
    || /usr/libexec/PlistBuddy -c "Add :UILaunchStoryboardName string LaunchScreen" "$APP_BUNDLE/Info.plist"

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

echo ">> adding Android 12+ splash theme (brand background; icon comes from the launcher icon)"
# The generated res/ has no values-v31/, so this override is conflict-free.
mkdir -p "$RES/values-v31"
cp "$SPLASH_DIR/android/values-v31/styles.xml" "$RES/values-v31/styles.xml"

if [[ -n "$BUILD_NUMBER" ]]; then
  echo ">> setting versionCode = $BUILD_NUMBER"
  # dx 0.7.9 hardcodes versionCode = 1 in its template; rewrite the
  # generated (and regenerated-every-build) gradle file.
  sed -i '' "s/versionCode = 1/versionCode = $BUILD_NUMBER/" "$PROJ/app/build.gradle.kts"
  grep -q "versionCode = $BUILD_NUMBER" "$PROJ/app/build.gradle.kts" \
    || { echo "versionCode rewrite failed; dx template changed?" >&2; exit 1; }
fi

# One gradle invocation: the tasks share the configuration phase and the
# compiled outputs, so a release build does not pay gradle startup twice.
GRADLE_TASKS=(assembleDebug)
if [[ "$PROFILE" == "release" ]]; then
  # bundleRelease emits the Play AAB (unsigned until Play App Signing is set up).
  GRADLE_TASKS=(assembleRelease bundleRelease)
fi
echo ">> gradle ${GRADLE_TASKS[*]} (branded APK)"
( cd "$PROJ" && ./gradlew "${GRADLE_TASKS[@]}" )

echo ">> branded Android artifacts:"
find "$PROJ/app/build/outputs" \( -name "*.apk" -o -name "*.aab" \) 2>/dev/null || true
