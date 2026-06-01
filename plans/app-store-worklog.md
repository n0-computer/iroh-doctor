# Worklog — app store release (autonomous session)

Started: 2026-06-01, continued while Rae is out.
Branch: rae/doctor-app. No push, no PR, no destructive ops. Leaving ready.

Decisions taken (from Rae): distribution = public dev-tool, both stores;
icon/name wiring = **build wrapper + override files**; "do them all".

## Plan for this block
1. Android `[android]` config — min/target/compile SDK (supported config).
2. iOS `PrivacyInfo.xcprivacy` — Apple privacy manifest content.
3. Build wrapper — inject icon + display name (+ privacy manifest) into dx's
   generated projects, since dx 0.7.9 regenerates them and has no icon/name hook:
   - iOS: after `dx build --ios`, compile the AppIcon.appiconset with `actool`,
     merge icon keys + fix `CFBundleDisplayName` into Info.plist, drop in the
     privacy manifest.
   - Android: after `dx build --android`, overwrite `res/mipmap-*` + adaptive
     XML + `strings.xml` app_name, then run `gradlew` directly.
4. Verify what's possible without accounts/signing (Android debug APK is fully
   testable; iOS .app contents verifiable but not installable unsigned).

## Progress

- **Android SDK config** — added `[android] min_sdk=24, target_sdk=35,
  compile_sdk=35` to Dioxus.toml. Verified: generated `build.gradle.kts` now
  shows 35/24/35, Android build green. (Play requires API 35 for new apps; dx
  defaulted to 34.)
- **iOS privacy manifest** — wrote `app/ios/PrivacyInfo.xcprivacy` (no tracking,
  no collection by default, FileTimestamp reason 3B52.1 for the diagnostics
  export's mtime reads). Flagged: re-verify no linked lib uses other
  required-reason APIs before submission.
- **iOS icon catalog** — restructured into
  `app/assets/icon/ios/Assets.xcassets/AppIcon.appiconset` so `actool` can
  compile it.
- **Build wrapper** — `app/scripts/bundle-mobile.sh`:
  - iOS: `dx build --ios` → `actool` compiles the icon into the `.app` →
    PlistBuddy merges icon keys + sets `CFBundleDisplayName`/`CFBundleName` →
    copies in the privacy manifest → `plutil -lint`.
  - Android: `dx build --android` → overwrite mipmaps + adaptive XML + app_name +
    background color → `gradlew assembleDebug` (branded APK, debug-signed).
  - **VERIFIED both platforms.** Android: debug APK reports
    `application-label:'iroh doctor'`, icon = our adaptive pulse mark (viewed the
    extracted PNG — not the default robot), `targetSdkVersion 35`,
    `BUILD SUCCESSFUL`. iOS: `.app` has `Assets.car` with the AppIcon renditions,
    `CFBundleIcons` wired, `CFBundleDisplayName=iroh doctor`, `PrivacyInfo.xcprivacy`
    present, `plutil -lint` OK.
  - Bugs fixed while building it: empty-array under `set -u` (bash 3.2), and
    Android env had to be exported before `dx build --android` (not just before
    gradle).

## Outcome

The icon + display-name wiring — the last hard blocker — is solved and verified
on both platforms via the wrapper. Remaining work is account-/device-/brand-
gated (see plans/app-store-release-design.md). Release artifacts (signed IPA /
AAB) still need Apple/Google accounts and signing identities.

## How to build a branded bundle
```sh
cd app
scripts/bundle-mobile.sh android   # or: android --release
scripts/bundle-mobile.sh ios       # or: ios --release  (then sign/archive)
```
