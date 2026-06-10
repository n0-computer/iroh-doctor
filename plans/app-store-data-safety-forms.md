# DRAFT - App Privacy and Data Safety form answers (Apple + Google Play)

> Ready-to-enter answers for the Apple App Privacy "nutrition label" and the
> Google Play Data Safety form. Written against the code on `rae/doctor-app`
> as of 2026-06-09, updated after the opt-in telemetry fix below.

## Premise: app telemetry is opt-in (fixed 2026-06-10)

Until the overnight 2026-06-09 session the code contradicted the "telemetry
off by default" promise in the privacy-policy draft, the listing copy, and
the release design: `resolve_api_secret` fell back to the bundled
`DEFAULT_API_SECRET` when the saved override was empty (the fresh-install
state), `start_services_client` built the iroh-services client at app
startup, registering a device name derived from the node id, and the client
pushed endpoint metrics every 60 seconds (the iroh-services 1.0.0-rc.1
builder default that `build_client` never disables). There was no in-app
opt-out: saving an empty key resolved back to the bundled default.

This is fixed in `core/src/services.rs`: an app-side empty override now
resolves to `None`, so iroh-services stays off until the user pastes a key
(or sets `IROH_SERVICES_API_SECRET`). The cli keeps its out-of-the-box
bundled default; it is a foreground dev tool and not a store deliverable.
The forms below describe this shipped behavior (scenario B). Re-verify the
fresh-install behavior on a device before entering the answers.

## Ship configurations and their outcomes

- **A. Services client compiled out of mobile builds.** Apple: "Data Not
  Collected" overall. Play: "No data collected or shared". The cleanest
  label, but loses the side-by-side iroh-services diagnostics view.
- **B. Opt-in only: no bundled default in the app, telemetry starts only
  after the user pastes their own iroh-services key (IMPLEMENTED).** Apple:
  declare Diagnostics and Identifiers as collected (not linked, no
  tracking). Play: declare the same two types with the "optional" flag.
  Reasoning below.
- **C. Bundled default, on at startup (the pre-fix behavior).** Must declare
  collection, cannot mark it optional on Play, and the listing copy and
  privacy policy are false as written. Do not ship this.

Why scenario B is not "Data Not Collected": Apple requires declaring all
data the app collects, including from features only some users enable. The
optional-disclosure carve-out (data the user actively submits through a
form each time) does not apply, because once enabled the telemetry pushes
continuously in the background. Google Play likewise requires declaring
optional collection, but its form has a per-type "optional" toggle, which
fits this case exactly.

## Apple App Privacy (App Store Connect), scenario B

Tracking question: **No, we do not use data for tracking.** No ad networks,
no data brokers, no linking with third-party data.

| Apple category | Answer | Linked to user | Tracking | Justification |
| --- | --- | --- | --- | --- |
| Contact Info | Not collected | - | - | No accounts, no name/email/phone anywhere. |
| Health & Fitness | Not collected | - | - | Not applicable. |
| Financial Info | Not collected | - | - | Free app, no payments. |
| Location | Not collected | - | - | No location APIs. IP addresses transit relays only to service connections (see edge cases). |
| Sensitive Info | Not collected | - | - | Not applicable. |
| Contacts | Not collected | - | - | No contacts access. |
| User Content | Not collected | - | - | Gossip messages go peer-to-peer, encrypted in transit; we never receive or store them (see edge cases). |
| Browsing History | Not collected | - | - | Not applicable. |
| Search History | Not collected | - | - | Not applicable. |
| Identifiers > Device ID | **Collected** | No | No | Opt-in telemetry registers an app-generated device name derived from the public node id. Per-install app-generated IDs count as Device ID. |
| Purchases | Not collected | - | - | No purchases. |
| Usage Data | Not collected | - | - | No analytics SDK, no product-interaction tracking. |
| Diagnostics > Performance Data | **Collected** | No | No | Opt-in telemetry pushes iroh endpoint metrics (connection and relay performance counters) to iroh-services. |
| Diagnostics > Crash Data | Not collected | - | - | No crash reporter. Update if one is added. |
| Surroundings | Not collected | - | - | Not applicable. |
| Body | Not collected | - | - | Not applicable. |
| Other Data | Not collected | - | - | Nothing else leaves the device to us. |

For both collected types: purpose = App Functionality (and Analytics if the
console requires a second purpose for metrics), linked to identity = No
(no account; the node id is per-install and not tied to a person by us),
used for tracking = No.

Under scenario A, every row is "Not collected" and the overall label is
**Data Not Collected**.

## Edge cases and conclusions (both stores)

- **Opt-in telemetry.** Conclusion: must be declared if the capability
  ships in the binary, even though it is off by default. Apple has no
  "optional" flag; Play does, so mark it optional there. The only way to a
  clean "Data Not Collected" label is to compile the path out (scenario A).
- **Diagnostics export.** Conclusion: not collection. The bundle is built
  on device and saved to a user-chosen location (desktop file dialog) or
  the app's documents directory (mobile). It reaches us only if the user
  sends it through a channel of their choosing; the app has no upload path.
- **Relay and DNS transit.** Conclusion: not collection. Establishing
  connections necessarily exposes IP addresses and public node ids to n0
  relay and discovery infrastructure, but that data is processed in real
  time to service the connection, which both stores exclude from the
  collection definition (Apple: real-time servicing exclusion; Play:
  ephemeral processing). It is disclosed in the privacy policy instead.
- **Gossip messages.** Conclusion: not collection. Messages travel
  peer-to-peer over QUIC-encrypted iroh connections; relays forward
  ciphertext and we never receive or store message content.
- **Locally stored data.** Conclusion: not collection. The node secret key,
  saved peer endpoints (node ids the user enters), the optional API key,
  and rolling log files live only in app-private storage and never leave
  the device on their own.

## Google Play Data Safety form, scenario B

- **Does your app collect or share any of the required user data types?**
  Yes (scenario A: No, and the rest of the form disappears).
- **Is all of the user data collected by your app encrypted in transit?**
  Yes. All traffic is QUIC/TLS (rustls); telemetry rides the iroh
  connection to the services backend.
- **Do you provide a way for users to request that their data is deleted?**
  The app has no accounts, so Play's account-deletion requirement does not
  apply. Answer the deletion question per the current console wording; if a
  free-text mechanism is required, point to the privacy-policy contact for
  removal of telemetry records.

Data types declared:

| Play data type | Collected | Shared | Ephemeral | Required or optional | Purpose |
| --- | --- | --- | --- | --- | --- |
| Device or other IDs | Yes | No | No | Optional (user enables telemetry by entering a key) | App functionality, Analytics |
| App info and performance > Diagnostics | Yes | No | No | Optional (same gate) | App functionality, Analytics |

Every other Play data type (Location, Personal info, Financial info, Health,
Messages, Photos and videos, Audio, Files and docs, Calendar, Contacts, App
activity, Web browsing, Crash logs): not collected, with the same reasoning
as the Apple table. Messages deserves one note for the IARC/content-rating
side: the gossip tab is user-to-user communication, but message content is
not collected by us, so it appears in the content rating questionnaire, not
in Data Safety.

"Shared" is No throughout: nothing goes to third parties; iroh-services is
operated by number 0, the developer.

## Answers may need updating if

- The telemetry default changes again, or a bundled key ships in app
  builds (the pre-fix code did both; see the premise section).
- A crash reporter or analytics SDK is added (declare Crash Data / Usage
  Data, and revisit `PrivacyInfo.xcprivacy`).
- The diagnostics bundle gains an upload-to-n0 path instead of local save
  (becomes collection, likely User Content or Other Diagnostic Data).
- iroh-services telemetry fields expand beyond endpoint metrics (re-check
  category mapping and the privacy policy's telemetry section).
- Accounts or sign-in are added (Play deletion requirement kicks in;
  "linked to user" answers change).
- Gossip messages are stored server-side or relayed through anything that
  retains them.
- The stores change their forms (both have done so; re-read the current
  questions at submission time).

## Verified against code

- Telemetry resolution: `core/src/services.rs` (`resolve_api_secret`: env
  var wins, app-side empty override resolves to `None` so telemetry stays
  off, cli-side `None` falls back to `DEFAULT_API_SECRET`; `build_client`
  leaves the 60 s metrics interval enabled once a client exists;
  `device_name`); `app/src/node/mod.rs` (`start_services_client`, called
  at startup with the saved override); `app/src/main.rs` (passes
  `identity::load_api_secret_override()`, empty on first run);
  `app/src/identity.rs` (API key stored locally as `api_secret.txt`).
- `TelemetryState` enum and initial `Off` value: `app/src/node/mod.rs`,
  `app/src/main.rs` (signal and watch channel start at `Off`).
- Logs are local only: `app/src/main.rs` (`log_dir`, `init_logging`: daily
  rolling file under the app config dir, plus `tracing-oslog` into the iOS
  unified log; no network log sink).
- Diagnostics export is user-initiated and local: `app/src/main.rs`
  (`save_diagnostics_zip`: rfd save dialog on desktop, app documents dir on
  mobile); `app/src/diagnostics_export.rs` (bundle contents, including
  `endpoints.json` and recent log files, so the privacy policy's note that
  an exported bundle contains logs and saved endpoints is accurate).
- Saved peer node ids stored locally: `app/src/endpoints.rs`
  (`endpoints.json` under the app config dir, written on every mutation).
- Node secret key generated and stored locally: `app/src/identity.rs`
  (`secret_key.bin`).
- Network exposure during operation: `app/src/node/mod.rs` (`bind_endpoint`
  applies `presets::N0`: n0 relay servers plus DNS-based discovery, no
  mDNS), so node ids and IP addresses transit n0 infrastructure as part of
  connecting.
- Trust model: `app/src/node/mod.rs` accept loop serves the probe ALPN with
  no authentication, capped at `MAX_CONCURRENT_PROBE_SERVERS = 4`; anyone
  with the node id can connect while the app runs.
