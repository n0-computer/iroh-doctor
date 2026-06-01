# Plan: release iroh-doctor-app on the App Store and Google Play

- [x] write plan (this doc)
- [x] user approved plan ("just go yes") + go-as-far-as-possible execution
- [x] Phase 0: unify bundle id, set marketing name, dx mobile spike (see below)
- [x] Phase 3 (iOS, partial): local-network + export-compliance plist keys, verified
- [ ] remaining Phase 0–4 + as much of 5–6 as possible without paid accounts / Android hardware

Status: design approved (Approach A). Distribution posture: **public, positioned
as a developer / network-diagnostics utility**, both stores. Deliverable: this
plan, then execute as far as possible, flagging each credential / account /
device blocker.

## Spike results (verified 2026-06-01, dx 0.7.9)

The dx 0.7 mobile pipeline is real and works for this app:

- **`dx build --ios` succeeds end-to-end (exit 0).** The whole iroh stack
  compiles for `aarch64-apple-ios` (683 crates) and dx emits a working
  `IrohDoctorApp.app` with an `Info.plist`. First build ~30 s; re-bundling after
  a Dioxus.toml-only change is ~2 s (no recompile).
- **Config injection points (from the dx-generated templates):**
  - `[ios.plist]` — key→value map merged into `Info.plist` (bool → `<false/>`,
    string → `<string>`); `[ios.raw.info_plist]` for raw XML.
  - `[android.permissions]` and `[android.raw.manifest]` — Android manifest.
  - `[permissions]` — cross-platform high-level toggles (camera, microphone,
    notifications, photos, bluetooth, background) that expand to both iOS usage
    strings and Android permissions. No `local_network` high-level key, so the
    iOS local-network string is set directly via `[ios.plist]`.
  - `[ios]` also exposes `ios_info_plist`, `macos_entitlements`,
    `android_main_activity`, `android_min_sdk_version`, widget extensions.
- **Verified injection**: `[ios.plist]` `NSLocalNetworkUsageDescription` +
  `ITSAppUsesNonExemptEncryption = false` now appear in the generated Info.plist
  with correct types; `CFBundleIdentifier = com.number0.irohdoctor`;
  `plutil -lint` → OK.
- **Icon is NOT auto-wired** — the iOS `.app` ships only `favicon.ico` copied as
  a plain asset, no `AppIcon`/asset catalog. Wiring the app icon is real Phase-2
  work (mechanism TBD: likely `[bundle] icon` + `[ios.raw.info_plist]`
  `CFBundleIcons`, or dropping an asset catalog the dx project references).
- **Display name is auto-derived**: dx PascalCases the package name to
  `IrohDoctorApp` for `CFBundleDisplayName`; `[bundle] name` did not override it.
  Setting the public display name needs a confirmed key (small follow-up — try
  `[ios.raw.info_plist]` `CFBundleDisplayName`, or rename the package).
- **`dx build --android` succeeds (exit 0) after the rfd fix.** Env needed:
  `JAVA_HOME` = Android Studio's JBR (OpenJDK 21), `ANDROID_HOME`/`ANDROID_SDK_ROOT`
  = `~/Library/Android/sdk`, `ANDROID_NDK_HOME` = `.../ndk/30.0.14904198`. dx
  emits a Gradle project under `target/dx/.../android/app`; `dx bundle --platform
  android --package-types aab` produces the Play artifact. **These env vars must
  go in the release runbook** (Phase 7) — they are not set in the shell by default.
- **rfd does not compile for Android.** `Cargo.toml` gated rfd out only for iOS,
  so Android pulled the `xdg-portal` backend and failed (12 errors). Fixed by
  gating rfd to `cfg(not(any(ios, android)))` and routing Android through the
  same sandbox-write `save_diagnostics_zip` branch as iOS. Re-verified: Android
  builds clean.
- **App icon drafted**: `app/assets/icon/icon-master.svg` (+ rendered 1024 PNG) —
  a white ECG/pulse line on the iroh-purple (#7c7cff) gradient, echoing the app's
  RTT sparkline. Pending Rae's sign-off before generating the full size set.

## Goal

Ship `iroh-doctor-app` (Dioxus 0.7) to the iOS App Store and Google Play as a
public developer tool. Rae is the primary developer; teammates may need to build
and push later, so the release process must be documented and reproducible
(local builds first, CI as a Phase 7 follow-up).

## Approach (A): iOS device-first, then mirror to Android

Prove the app runs *well* on Rae's real iPhone first, nail the mobile UX and the
iOS-specific gotchas (local-network entitlement, privacy manifest, export
compliance), reach TestFlight — then replicate the proven shape on Android
(emulator-tested) and go public on both. Shared assets (icon, copy, privacy
policy) are made once, early. Rationale: there is an iPhone but no Android
device, iOS review is the stricter gate, and de-risking mobile UX once benefits
both platforms.

Rejected alternatives: **B (both platforms in lockstep)** — debugging two
unproven pipelines at once with no Android hardware; **C (listing/branding
first)** — risks polishing a listing for an app whose phone UX isn't proven yet.

## Current state (verified 2026-06-01)

- Dioxus 0.7.1 crate `iroh-doctor-app` v0.1.0; `dx` 0.7.9, `rustc` 1.95.
- Features: `default = ["desktop"]`, plus `web`, `mobile`. iOS-specific code
  already exists (`tracing-oslog`, `rfd` excluded on iOS with a sandbox-write
  fallback) — iOS has been started but never shipped.
- Bundle id today: `com.number0.iroh-doctor-app` (`app/Dioxus.toml`).
- Assets: only `assets/favicon.ico` and `assets/styling/main.css`. No app icon,
  splash, screenshots, or store metadata.
- Telemetry: `TelemetryState` defaults `Off`; opt-in iroh-services API key path.
  Logs are written locally (rolling file + oslog on iOS).
- Branding source `../iroh.computer`: has **wordmark** SVGs
  (`public/img/logo/iroh-wordmark-*.svg`) and an iOS-starter screenshot set, but
  **no square app mark**.

## Findings that shape the plan

1. **Android applicationId is invalid today.** `com.number0.iroh-doctor-app`
   contains hyphens; Android package segments cannot. Need a unified id, e.g.
   `com.number0.irohdoctor`, and align iOS to match.
2. **iOS local-network permission is mandatory; Bonjour is not.** Verified that
   `bind_endpoint` uses `presets::N0` = DNS/pkarr lookup + relay, with **no mDNS**
   (and the app adds none). So `NSLocalNetworkUsageDescription` IS required —
   iroh's direct LAN hole-punching triggers the iOS 14+ local-network prompt, and
   without the string iOS silently blocks local connections (relay-only fallback,
   which guts a diagnostics tool) — but **`NSBonjourServices` is not needed**
   unless mDNS discovery is added later. *(Done: string added via `[ios.plist]`.)*
3. **Android multicast for mDNS** needs `CHANGE_WIFI_MULTICAST_STATE` +
   `ACCESS_WIFI_STATE` and a held `WifiManager.MulticastLock`, or local discovery
   won't work on device.
4. **Apple Privacy Manifest** (`PrivacyInfo.xcprivacy`) is required for App Store
   submission and must declare required-reason APIs + data collection.
5. **Export compliance**: the app uses encryption (QUIC/TLS via rustls). Needs an
   `ITSAppUsesNonExemptEncryption` declaration; standard TLS typically qualifies
   for the exemption but must be self-classified.
6. **Public name**: `iroh-doctor-app` is a poor store display name. Pick a
   marketing name (e.g. "iroh doctor") distinct from the bundle name.
7. **Trust-model copy**: the README calls this "a peer-to-peer debug tool to hand
   to a known collaborator … not a service to leave exposed." For a *public*
   listing this framing must be reconciled in the store description and likely an
   in-app note, so reviewers and users understand the tool's intent.
8. **dx 0.7 mobile mechanics unknown to us**: exactly how `dx bundle` injects
   icons, Info.plist keys, manifest permissions, and signing for ios/android in
   0.7.9 needs a short spike before relying on it (it may be Dioxus.toml config,
   or editing the generated Xcode/Gradle project).

## Phases

### Phase 0 — Decisions & accounts

- [x] Set **marketing name** "iroh doctor" in `[bundle] name` (note: this drives
      desktop/web, NOT the iOS `CFBundleDisplayName` — see spike follow-up).
- [x] Unify **bundle id / applicationId** to `com.number0.irohdoctor` in
      `app/Dioxus.toml`; verified in the generated iOS `CFBundleIdentifier`.
- [ ] Decide **org vs personal** for both accounts. Bundle prefix implies the
      `number0` org. *(Blocker — Rae: org Apple enrollment needs a D-U-N-S
      number; allow days/weeks of lead time.)*
- [ ] **Enroll Apple Developer Program** ($99/yr). *(Blocker — Rae: payment + ID.)*
- [ ] **Create Google Play Console account** ($25 one-time). *(Blocker — Rae:
      payment + ID verification, which Google now requires up front.)*
- [ ] Decide **version/build-number strategy**: Cargo `version` → iOS
      `CFBundleShortVersionString` + monotonic `CFBundleVersion`; Android
      `versionName` + monotonic integer `versionCode`. Document the mapping.
- [x] **Spike**: dx 0.7.9 mobile bundle mechanics — done for **both** iOS and
      Android (see "Spike results"). Android required an rfd fix (below) and the
      local SDK/NDK env; both `dx build --ios` and `dx build --android` now
      succeed. Icon drafted; display-name wiring still open.

### Phase 1 — Mobile readiness (iPhone first)

- [ ] Build + run on the **physical iPhone** via `dx serve --platform ios`
      (device, not just Simulator); confirm it launches and an endpoint comes up.
- [ ] Fix **mobile layout**: tab UI, safe-area insets (notch/home indicator),
      scroll, touch-target sizes, input fields (paste endpoint id), no desktop
      window-chrome assumptions.
- [ ] Verify the **local-network permission prompt** appears and discovery works
      once granted.
- [ ] Reconcile the **trust-model framing** for public users (in-app copy /
      first-run note as needed).
- [ ] Harden **error + empty states** for a stranger's first launch (no peer,
      offline, permission denied, diagnostics export on iOS sandbox).
- [ ] Decide **background behavior** (foreground-only is fine for a debug tool;
      avoids background-mode entitlements).
- [ ] Confirm release builds work: `dx bundle --platform ios --release`.

### Phase 2 — Branding & assets

- [~] Design a **square app mark** (iroh.computer only has wordmarks). DRAFTED:
      `app/assets/icon/icon-master.svg` — white ECG/pulse line on iroh-purple
      gradient, 1024 master rendered. Awaiting Rae's sign-off / n0-brand check.
- [ ] Generate the **iOS asset catalog** icon set (all required sizes) +
      Android **adaptive icon** (foreground/background layers, all densities).
- [ ] **Splash / launch screen** consistent with the mark.
- [ ] **Screenshots** on device for required sizes: iPhone 6.7"/6.5"/5.5" as
      currently required; Android phone (+ optional tablet).
- [ ] Play **feature graphic** (1024×500) and any promo art.
- [ ] Replace the placeholder `favicon.ico` with the new mark for web/desktop too.

### Phase 3 — Platform config & compliance

iOS:
- [x] `Info.plist`: `NSLocalNetworkUsageDescription` via `[ios.plist]` (verified).
      `NSBonjourServices` not needed (no mDNS in `presets::N0`).
- [x] `ITSAppUsesNonExemptEncryption = false` via `[ios.plist]` (verified;
      standard TLS/QUIC is export-exempt).
- [ ] Add `PrivacyInfo.xcprivacy` (required-reason APIs + data-collection types).
- [ ] Wire the **app icon** (not auto-bundled today) and the public
      **display name** (`CFBundleDisplayName` currently auto-derives to
      "IrohDoctorApp").
- [ ] Set **minimum iOS deployment target**; confirm device arch (`aarch64-apple-ios`).
- [ ] App category, display name, bundle version wiring.

Android:
- [ ] Set valid `applicationId`; `targetSdkVersion` to Play's current minimum for
      new apps *(verify the required API level at submission time — it rises
      yearly)*; `minSdkVersion`.
- [ ] Manifest permissions: `INTERNET`, `ACCESS_NETWORK_STATE`,
      `ACCESS_WIFI_STATE`, `CHANGE_WIFI_MULTICAST_STATE`; acquire a
      `MulticastLock` at runtime for mDNS.
- [ ] Confirm Rust ABIs built: `arm64-v8a` (required), optional `x86_64` for
      emulator; NDK version pinned.
- [ ] Output **AAB** (App Bundle), not APK.

### Phase 4 — Legal

- [~] Write a **privacy policy** (required by both stores even with no
      collection): local logs, telemetry-off-by-default, opt-in iroh-services.
      DRAFTED: `plans/app-store-privacy-policy-draft.md` — needs legal review +
      a couple of confirmed details (telemetry fields, contact, retention).
- [ ] **Host** it under `https://www.iroh.computer/legal` (confirmed location).
      *(Blocker — Rae / web team: publish the page at that URL.)*
- [ ] Optional **terms of use**.
- [ ] Prepare **App Privacy "nutrition label"** (iOS) answers.
- [ ] Prepare **Data Safety** form answers (Android).

### Phase 5 — iOS submission

- [ ] Create the **App ID + provisioning** (distribution) in the Apple portal.
      *(Blocker — Rae: requires the paid account.)*
- [ ] Configure **signing** in the dx-generated Xcode project / Dioxus.toml.
- [ ] **Archive** a release build; upload to **App Store Connect**.
- [ ] Smoke-test via **TestFlight** on the iPhone (and any teammates).
- [ ] Fill the **App Store Connect listing**: name, subtitle, description
      (dev-tool framing), keywords, category, support URL, privacy URL,
      screenshots, export-compliance, age rating.
- [ ] **Submit for review**; handle rejections (local-network rationale is the
      most likely follow-up). *(Blocker — Rae: hits "Submit".)*

### Phase 6 — Android submission

- [ ] Generate an **upload keystore**; enroll in **Play App Signing**.
      *(Blocker — Rae: keystore custody + Play account.)*
- [ ] Build a signed **release AAB**.
- [ ] Create the app in **Play Console**; **Internal testing** track first
      (Rae can test even without an Android phone via emulator / a borrowed
      device — note: physical-device coverage is a gap to close).
- [ ] Complete **Data Safety**, **Content Rating (IARC)**, target-audience,
      store listing (icon, feature graphic, screenshots, description), privacy URL.
- [ ] Promote to **Production**; **submit for review**. *(Blocker — Rae: submits.)*

### Phase 7 — CI follow-up (post-launch)

- [ ] Automate builds/signing/upload (fastlane + GitHub Actions) so teammates can
      release; store signing secrets securely (App Store Connect API key, Android
      keystore + service account).
- [ ] Document the release runbook in the repo.

## Blockers that require Rae (only-human steps)

- Apple Developer Program enrollment + payment (+ D-U-N-S if org).
- Google Play Console account + payment + identity verification.
- Custody of signing keys (iOS distribution cert, Android upload keystore).
- Physical-device testing: iPhone available; **Android device is a gap**.
- Publishing the privacy-policy URL.
- Pressing "Submit for review" on each store.

## Out of scope (for now)

- macOS/Windows/Linux desktop store distribution (separate effort).
- Web deployment.
- Background networking / push notifications.
- Localization beyond English.
