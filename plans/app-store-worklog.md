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

## 2026-06-10 — publish push (session with Rae)

Accounts confirmed: Apple = personal paid membership (krmckelv@gmail.com,
team 84T7UAWDW5) → iOS path unblocked. Play Console = created, identity
verification PENDING → Play uploads blocked until it clears.

Done this session:
- Display name fixed durably for iOS: `[ios.plist]` CFBundleDisplayName/
  CFBundleName = "iroh doctor" in Dioxus.toml — verified to override dx 0.7.9's
  crate-name template even on plain `dx build --ios` (the design doc's
  hypothesized fix, now confirmed). Android still needs the wrapper.
- Release artifacts built + verified with BUILD_NUMBER=1: iOS branded .app
  (CFBundleVersion=1) and Android AAB + APK. See plans/release-runbook.md.
- Play upload keystore generated: ~/.android-keystores/iroh-doctor-upload.jks
  (alias `upload`; password in adjacent README — Rae: move to password manager).
- Store screenshots captured at 1320×2868 (6.9", iPhone 16 Pro Max simulator)
  in app/assets/store/screenshots/: first-run note, clean Connect, and a live
  probe (direct path, latency sparkline, throughput) driven by the CLI
  (`iroh-doctor connect <sim app id>`). Tab screenshots (Diagnostics/Gossip/
  Endpoints) need simulated taps = macOS accessibility permission, or 2 min of
  manual capture: `xcrun simctl io booted screenshot out.png`.
- Wrote plans/release-runbook.md: exact remaining steps both stores, signing/
  IPA commands, build-number ledger.

Open blockers (Rae-only): deploy privacy policy (uncommitted in
../iroh.computer), merge n0-computer/svc#887, ASC app record + distribution
cert + profile + upload, Play verification wait, merge rae/doctor-app.

## 2026-06-10 (later) — iOS build 1 uploaded to App Store Connect

ASC API key + App Store profile in place (see runbook "Credentials" section).
altool validation exposed four dx 0.7.9 store-compliance defects, all fixed
(wrapper now pins --target aarch64-apple-ios; package-ios-ipa.sh stamps DT*
keys, CFBundlePackageType=APPL, single-value CFBundleSupportedPlatforms;
MinimumOSVersion aligned to the binary's real 17.0). VERIFY SUCCEEDED, then
upload succeeded: build 1 (0.1.0), delivery 52ee7635-a13e-4305-8b41-ad05e9df89fc.
Next: TestFlight smoke test on the iPhone once processing finishes, listing
entry (copy in plans/app-store-listing-copy.md), privacy nutrition label
(answers in plans/app-store-data-safety-forms.md), submit.

## 2026-06-11 — iOS build 2 uploaded (clipboard fix)

Build 1 on TestFlight showed the Copy button writing an empty clipboard when
the iOS build runs on an Apple silicon Mac: WebKit's
navigator.clipboard.writeText resolves ok but only the private
com.apple.WebKit.custom-pasteboard-data type crosses the UIPasteboard ->
NSPasteboard bridge; the text/plain payload is dropped. Fixed by writing
through UIPasteboard directly on iOS (commit 0ddd764). Build 2 (0.1.0)
packaged with the unchanged bundle-mobile.sh + package-ios-ipa.sh pipeline,
VERIFY SUCCEEDED, upload succeeded: delivery
f1c7549f-2b03-424d-a139-a77d0da20bfa. Verify on the Mac TestFlight install
once processing finishes: click Copy, paste into another app.
