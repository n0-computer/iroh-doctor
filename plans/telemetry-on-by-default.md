# Plan: telemetry on by default, with disclosure + opt-out

- [x] write plan
- [x] core: `SecretSource::AppDefault` + resolution + tests
- [x] app: `telemetry_pref` persistence (disabled marker) + tests
- [x] node: `disabled` state + `SetTelemetryEnabled` command; rewire `start_services_client`
- [x] app main: wire the disabled flag into `run_node` and `services_configured`
- [x] UI: on/off toggle in Diagnostics, keep the custom-key field, update status strings
- [x] first-run: fold a telemetry sentence into the trust note
- [x] store declarations: PrivacyInfo.xcprivacy, privacy policy, listing copy, data-safety forms
- [x] learnings: reverse the "App services / telemetry" invariant
- [x] final review (adversarial self + 4-reviewer staff review; fixes amended into a668a3e, see worklog)

Spec: `docs/superpowers/specs/2026-06-10-telemetry-on-by-default-design.md`.

## Goal

Make app telemetry on by default using the bundled iroh-services key, with a
first-run disclosure and a persistent in-app off toggle. Update the privacy
docs and store declarations to match the new default rather than constraining
the code to the old "off by default" language.

## Context

The 2026-06-09 fix (`acfbf14`) made app telemetry opt-in to match the privacy
docs: `core::services::resolve_api_secret(SavedOverride(s))` returns `None` when
the saved key is empty, so a fresh install pushes nothing. We are reversing the
product decision. iroh doctor is a diagnostics tool; anonymous connection
telemetry is part of its purpose.

Current mechanism, all of which this plan touches:

- `core/src/services.rs`: `SecretSource { SavedOverride(&str), BundledDefault }`
  and `resolve_api_secret`. The env var `IROH_SERVICES_API_SECRET` wins (empty =
  opt out). The app passes `SavedOverride`; the cli passes `BundledDefault`.
- `app/src/identity.rs`: `api_secret.txt` holds the custom key; empty/missing
  reads as off today.
- `app/src/node/mod.rs`: `run_node` tracks a mutable `api_secret_override`;
  `start_services_client` resolves it and builds or skips the client;
  `NodeCommand::SaveApiSecret` drops and rebuilds the client.
- `app/src/main.rs`: `services_configured()` gates the services probes;
  `TelemetryState` flows over a `tokio::sync::watch` channel.
- `app/src/components/diagnostics.rs`: the iroh-services section is a key-paste
  field plus a telemetry-status line; empty key = "off".
- `app/src/components/first_run_note.rs` + `app/src/first_run.rs`: a dismissable
  trust note backed by a `trust_note_dismissed` marker file.
- `app/ios/PrivacyInfo.xcprivacy`: declares `NSPrivacyCollectedDataTypes` empty.
- `plans/learnings.md` "App services / telemetry": documents the old invariant
  ("the app must never use `BundledDefault`"), which this work reverses.

## Approach

Telemetry resolution becomes a tri-state expressed as two independent prefs: an
enabled flag (absent = on) and the existing custom key (empty = use bundled).

### Step 1 - core: `SecretSource::AppDefault` (commit 1)

Replace the `SavedOverride` variant with `AppDefault { disabled: bool, custom:
&str }`. `resolve_api_secret` precedence:

1. `IROH_SERVICES_API_SECRET` set: non-empty used as-is, empty -> `None`.
2. `AppDefault { disabled: true, .. }` -> `None`.
3. `AppDefault { custom }` non-empty -> trimmed custom key.
4. `AppDefault { disabled: false, custom: "" }` -> bundled `DEFAULT_API_SECRET`.
5. `BundledDefault` -> bundled key (cli, unchanged).

Extend the existing precedence test for the new variant (default-on, disabled,
custom key, env override, env empty-opt-out). Update the `SecretSource` and
`DEFAULT_API_SECRET` doc comments: the app now does reach the bundled key by
default. This is a breaking change to the core API (`!` suffix).

### Step 2 - app: telemetry-enabled persistence (commit 1)

Add a `telemetry_disabled` marker file in the app config dir, mirroring
`first_run.rs`: presence = off, absence = on (default). Provide a reader and a
`set_telemetry_disabled(bool)` writer with a roundtrip test. Put it in
`app/src/identity.rs` (alongside the other config-dir state) or a small
`telemetry_pref.rs`; identity.rs already owns config-dir state, so extend it
unless it grows unwieldy.

### Step 3 - node: disabled state + toggle command (commit 1)

`run_node` gains an initial `disabled: bool` alongside the existing override and
tracks it mutably. `start_services_client` takes `(disabled, custom)` and builds
`AppDefault { disabled, custom }`. Add `NodeCommand::SetTelemetryEnabled { enabled }`
that updates the flag and rebuilds the client the same way `SaveApiSecret` does
(drop the old client so its push task stops, then rebuild). Add `info!` tracing
on the toggle so a 3 AM reader sees telemetry flipping.

### Step 4 - app main: wiring (commit 1)

`main.rs` reads the disabled marker at startup and passes it into `run_node`.
`services_configured()` resolves `AppDefault { disabled, custom }` instead of
`SavedOverride`. The initial `TelemetryState` stays `Off` until the client
reports `Active` (no change to the channel).

Also update `plans/learnings.md`: the "App services / telemetry" section
documents the now-reversed invariant. Rewrite it to describe the new default
(on by default via the bundled key, off via the marker or the env opt-out) in
the same commit that makes the reversal.

### Step 5 - UI: toggle + strings (commit 2)

In `diagnostics.rs`, add a Telemetry on/off toggle at the top of the
iroh-services section, bound to the disabled marker, defaulting on. Toggling
persists the marker and sends `SetTelemetryEnabled`. Keep the custom-key field
below, relabeled as an advanced option. Rewrite the footer note and
`telemetry_line` so "off - paste a key to enable" becomes on-by-default copy.

### Step 6 - first-run disclosure (commit 2)

Add one sentence to the trust note in `first_run_note.rs`: "iroh doctor sends
anonymous connection diagnostics to help improve iroh. You can turn this off in
Diagnostics." One dismissal still covers the whole note.

### Step 7 - store declarations (commit 2)

- `PrivacyInfo.xcprivacy`: add `NSPrivacyCollectedDataTypes` for Device ID and
  Performance Data (collected, not linked to identity, not used for tracking;
  purposes App Functionality + Analytics). `NSPrivacyTracking` stays false.
- `plans/app-store-privacy-policy-draft.md`: rewrite the telemetry section to
  on-by-default with an opt-out.
- `plans/app-store-listing-copy.md`: flip the data-safety bullet (~line 77).
- `plans/app-store-data-safety-forms.md`: move the shipped-behavior pointer from
  Scenario B to Scenario C; keep Play's optional flag, justified by the toggle.

## Risks and open questions

- **Migration.** Existing installs with an empty `api_secret.txt` and no marker
  flip off -> on, and will not re-see the first-run note (already dismissed).
  Accepted: pre-release, tiny base, and on-by-default is the intent.
- **Live stop.** Turning off must actually stop metric pushes. `SaveApiSecret`
  already proves dropping the client stops its tasks; `SetTelemetryEnabled`
  reuses that path. Verify by reading `start_services_client` returns `None`
  when disabled.
- **Play "optional" flag.** Keeping it depends on the toggle being a genuine,
  discoverable opt-out. The toggle in step 5 satisfies that.
- **clippy dead-code.** Adding `AppDefault` without using it would warn, so the
  core change and the app migration land in the same commit (commit 1), not
  separately.

## Commit strategy

One commit. The adversarial plan review found that splitting collection (on by
default) from its declaration (`PrivacyInfo.xcprivacy`) and its opt-out (the
toggle, which the data-safety form and privacy policy claim exists) produces an
intermediate commit that is individually incorrect on the compliance axis: code
that collects with a manifest saying it does not, or a declared opt-out that
does not yet exist. These three are coupled and must land together.

`feat!: turn app telemetry on by default with a disclosure and opt-out`, all
steps above in one commit. Breaking: `SecretSource` and the resolution semantics
change, and the observable default flips off -> on. PR notes need a Breaking
changes section. The diff is larger than the usual target (~12 small files) but
it is one cohesive, individually-correct change. The only piece that ships
nowhere and could trail in its own commit is `app-store-listing-copy.md`
(external marketing copy); folded in here for simplicity.
