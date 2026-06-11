# Release runbook — iroh doctor (App Store + Google Play)

Status as of 2026-06-10. Accounts: Apple Developer Program = **personal,
enrolled** (krmckelv@gmail.com, team `3N5967R866`); Play Console = **created,
identity verification pending** (blocks all Play uploads until it clears).

## Credentials on this machine

- App Store Connect API key `9S6KRR582J`:
  `~/.appstoreconnect/private_keys/AuthKey_9S6KRR582J.p8` (altool finds keys
  there automatically; pair with the Issuer ID from the ASC API page).
- App Store provisioning profile:
  `~/Library/MobileDevice/Provisioning Profiles/iroh_doctor_appstore.mobileprovision`
  (team `3N5967R866`, app id `com.number0.irohdoctor`).
- Apple Distribution certificate: login keychain ("Apple Distribution:
  Karissa McKelvey (3N5967R866)"). Export a .p12 backup (Keychain Access →
  My Certificates → right-click → Export) into a password manager — losing
  the private key means re-issuing the cert.
- Play upload keystore: `~/.android-keystores/iroh-doctor-upload.jks` (see
  Android section).

## Artifacts already built (this machine)

Built with `BUILD_NUMBER=1`, marketing version 0.1.0:

- iOS: `target/dx/iroh-doctor-app/release/ios/IrohDoctorApp.app` — branded
  (Assets.car icon, "iroh doctor" display name), `PrivacyInfo.xcprivacy`,
  `CFBundleVersion=1`. Unsigned: needs distribution signing + IPA packaging.
- Android: `target/dx/iroh-doctor-app/release/android/app/app/build/outputs/`
  - `bundle/release/app-release.aab` (Play upload artifact, unsigned)
  - `apk/release/app-release-unsigned.apk`
- Screenshots: `app/assets/store/screenshots/` (6.9" / 1320×2868, taken on the
  iPhone 16 Pro Max simulator). Reusable for Play (within 320–3840 px).
- Play feature graphic: `app/assets/store/feature-graphic.png` (1024×500).
- Upload keystore: `~/.android-keystores/iroh-doctor-upload.jks` (alias
  `upload`, RSA-4096, 25 y). Password in the README next to it — **move it to a
  password manager and delete that file.** With Play App Signing the upload key
  is resettable, so loss is recoverable.

To rebuild either artifact: `BUILD_NUMBER=<n> app/scripts/bundle-mobile.sh
<ios|android> --release` (bump `<n>` for every store upload; see the version
strategy in plans/app-store-release-design.md).

## Pre-publish blockers (do before either submission)

1. **Privacy policy is not live.** The "iroh doctor (App) Privacy Policy"
   section is drafted in `../iroh.computer/src/app/legal/page.jsx`
   (**uncommitted** in that repo). Review, commit, deploy; the listing URL is
   `https://www.iroh.computer/legal#iroh-doctor`.
2. **Merge n0-computer/svc#887** (telemetry "not used for advertising"
   carve-out) so the linked iroh-services policy backs the app's claims.
3. **Merge `rae/doctor-app`** (or at least make sure what you ship is what gets
   reviewed) — the branch with telemetry/copy/branding changes is local-only.

## iOS — path to TestFlight / App Store (unblocked now)

One-time setup, in order:

1. **App Store Connect record**: appstoreconnect.apple.com → My Apps → "+" →
   New App: platform iOS, name "iroh doctor", primary language EN, bundle ID
   `com.number0.irohdoctor` (register it at developer.apple.com → Identifiers
   first if the dropdown doesn't offer it; capabilities: none beyond defaults),
   SKU e.g. `iroh-doctor-001`.
2. **Distribution certificate**: Xcode → Settings → Accounts →
   krmckelv@gmail.com → Manage Certificates → "+" → Apple Distribution.
3. **Provisioning profile**: developer.apple.com → Profiles → "+" →
   App Store Connect distribution, App ID `com.number0.irohdoctor`, the new
   distribution cert. Download `iroh_doctor_appstore.mobileprovision`.
4. **Sign + package the IPA**: `app/scripts/package-ios-ipa.sh` (signs the
   release .app with the Apple Distribution cert using entitlements extracted
   from the provisioning profile, emits `target/iroh-doctor-<n>.ipa`).
5. **Upload**:
   `xcrun altool --upload-app -f target/iroh-doctor-<n>.ipa -t ios --apiKey
   9S6KRR582J --apiIssuer <ISSUER_ID>` (Issuer ID is on the ASC API page).
   Validate first with `--validate-app` (same flags). GUI fallback: the
   **Transporter** app.
6. **TestFlight**: the build appears in App Store Connect → TestFlight after
   processing (~15 min). Add yourself as an internal tester, install on the
   iPhone, verify: launch, icon, display name, local-network prompt, a real
   probe between phone and laptop, diagnostics export.
7. **Listing**: copy is staged in `plans/app-store-listing-copy.md`; App
   Privacy answers in `plans/app-store-data-safety-forms.md`; screenshots in
   `app/assets/store/screenshots/`. Privacy policy URL from blocker 1.
   Export compliance is pre-answered by `ITSAppUsesNonExemptEncryption=false`.
8. **Submit for review.** Most likely follow-up question: the local-network
   usage rationale (answer: QUIC hole-punching to user-designated peers,
   no mDNS, no scanning).

## Android — path to Play (blocked on account verification)

Once identity verification clears:

1. **Create the app** in Play Console (name "iroh doctor", app, free).
   Accept **Play App Signing** (Google holds the signing key; our keystore is
   only the upload key).
2. **Sign the AAB** with the upload keystore:

   ```sh
   AAB=target/dx/iroh-doctor-app/release/android/app/app/build/outputs/bundle/release/app-release.aab
   jarsigner -keystore ~/.android-keystores/iroh-doctor-upload.jks \
     -signedjar /tmp/iroh-doctor-1.aab "$AAB" upload
   ```
   (jarsigner prompts for the store password; it lives in the keystore README
   until moved to a password manager.)
3. **Internal testing track** first: upload the AAB, add your Google account
   as a tester, install via the opt-in link on an emulator (Play Store image)
   or a borrowed device. *Physical-device coverage is a known gap.*
4. **Console forms**: Data Safety (answers in
   `plans/app-store-data-safety-forms.md`), IARC content rating, target
   audience (18+/general — it's a dev tool, no kids' appeal), store listing
   (description from `plans/app-store-listing-copy.md`, icon auto-derived from
   the AAB, feature graphic + ≥2 phone screenshots from `app/assets/store/`).
   Privacy policy URL from blocker 1.
5. **Promote to Production** and submit for review (first review on a new
   account can take up to a week).

## CI releases (Phase 7) — what becomes a GitHub Actions secret

The standard pattern so teammates can release without Rae's laptop: keep all
signing material in GitHub Actions **secrets** (repo or org level, Settings →
Secrets and variables → Actions); the workflow reconstructs the keychain /
keystore at run time. Never commit any of these to the repo.

| Secret | Contents |
|---|---|
| `ASC_API_KEY_P8` | base64 of `AuthKey_9S6KRR582J.p8` |
| `ASC_API_KEY_ID` | `9S6KRR582J` |
| `ASC_API_ISSUER_ID` | the Issuer ID UUID |
| `IOS_DIST_CERT_P12` | base64 of the Apple Distribution cert+key export |
| `IOS_DIST_CERT_PASSWORD` | password chosen at .p12 export |
| `IOS_PROVISIONING_PROFILE` | base64 of `iroh_doctor_appstore.mobileprovision` |
| `ANDROID_UPLOAD_KEYSTORE` | base64 of `iroh-doctor-upload.jks` |
| `ANDROID_KEYSTORE_PASSWORD` | the keystore password |
| `PLAY_SERVICE_ACCOUNT_JSON` | Play Console → Setup → API access service account key (create after verification) |

Workflow shape (macOS runner): import the .p12 into a temporary keychain
(`security create-keychain` / `security import`), drop the profile into
`~/Library/MobileDevice/Provisioning Profiles/`, write the .p8 to
`~/.appstoreconnect/private_keys/`, then run the same three scripts used
locally (`bundle-mobile.sh ios --release`, `package-ios-ipa.sh`, `altool
--upload-app`). Android: write the keystore, `bundle-mobile.sh android
--release`, `jarsigner`, upload with the Play Developer API (service
account). fastlane wraps most of this if preferred, but the scripts already
do the heavy lifting. The API key + cert are org-wide powers — scope secrets
to this repo, and rotate the ASC key if it ever leaks (revoke in ASC, issue a
new one).

## Build-number ledger

| # | Date | Version | Platforms | Notes |
|---|------|---------|-----------|-------|
| 1 | 2026-06-10 | 0.1.0 | ios+android | iOS: uploaded to ASC (delivery 52ee7635). Android AAB built, awaiting Play verification |
| 2 | 2026-06-11 | 0.1.0 | ios | Clipboard fix (UIPasteboard on iOS, commit 0ddd764). Uploaded to ASC (delivery f1c7549f) |

Next upload: `BUILD_NUMBER=3` (never reuse, never decrease; shared counter
across both stores).
