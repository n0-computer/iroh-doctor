# Telemetry on by default, with disclosure + opt-out

**Date:** 2026-06-10
**Status:** approved, ready for implementation plan

## Motivation

iroh doctor is a diagnostics tool: collecting anonymous connection telemetry is
a deliberate part of its purpose, not a side feature. The 2026-06-09 fix made
app telemetry opt-in (off until the user pastes an iroh-services key) to match
the "off by default" promise then in the privacy docs. We are reversing that
product decision: telemetry should be **on by default** using the bundled
iroh-services key, with a clear first-run disclosure and an in-app off toggle.
The privacy docs and store declarations are updated to match, rather than the
code being constrained to match the old language.

This corresponds to **"Scenario C"** already drafted in
`plans/app-store-data-safety-forms.md` (bundled default, on at startup),
softened by adding a genuine in-app opt-out toggle.

## Decisions (from brainstorming)

- **Consent UX:** first-run notice + persistent off toggle (not silent;
  not a forced first-run choice).
- **Toggle UI:** an on/off toggle as the primary control, *keeping* the
  custom-key paste field as an advanced option.
- **Play "optional" flag:** keep it, justified by the in-app opt-out toggle
  (collection is user-controllable). Apple has no optional flag regardless.
- **Migration:** accept that existing installs flip off→on and won't re-see
  the first-run note (pre-release, tiny base). Do *not* bump the marker.

## 1. State model (core: `core/src/services.rs`)

Telemetry resolution becomes a tri-state expressed as two independent prefs:

- **enabled flag** — new app pref; *absent = on* (default), explicit *off*
  when the user toggles off.
- **custom key** — `api_secret.txt` as today; empty = use the bundled key,
  non-empty = use the user's key.

Extend `SecretSource`:

```rust
pub enum SecretSource<'a> {
    /// App: on by default with the bundled key, unless disabled by the user
    /// or overridden with a custom key.
    AppDefault { disabled: bool, custom: &'a str },
    /// CLI: bundled default out of the box (unchanged).
    BundledDefault,
}
```

`resolve_api_secret` precedence:

1. `IROH_SERVICES_API_SECRET` env var wins when set (non-empty → use as-is;
   empty → `None`, i.e. opt out — preserved for devs/CI).
2. `AppDefault { disabled: true, .. }` → `None`.
3. `AppDefault { custom }` non-empty → trimmed custom key.
4. `AppDefault { disabled: false, custom: "" }` → bundled `DEFAULT_API_SECRET`.
5. `BundledDefault` → bundled `DEFAULT_API_SECRET` (CLI, unchanged).

The `SavedOverride` variant is removed; all app callers move to `AppDefault`.
`device_name` and `build_client` are unchanged.

## 2. Persistence (app: `app/src/`)

A new `telemetry_disabled` marker file in the app config dir, mirroring the
existing `first_run.rs` trust-note marker pattern:

- presence of the file = telemetry off;
- absence = default-on.

Lives alongside `secret_key.bin` / `api_secret.txt` / `trust_note_dismissed`.
`api_secret.txt` keeps its current meaning and legacy-dir migration fallback.
Provide `telemetry_disabled()` reader and `set_telemetry_disabled(bool)` writer
with a roundtrip test mirroring `first_run`.

## 3. UI (`app/src/components/diagnostics.rs`)

- Add a **Telemetry on/off toggle** at the top of the iroh-services section,
  defaulting on, bound to the new marker.
- Toggling sends a node command that **starts/stops the services client live**:
  turning off drops the client so metric pushes stop; turning on rebuilds it.
- **Keep the custom-key field** below the toggle, relabeled as an advanced
  option (e.g. "Use your own iroh-services key").
- Rewrite the footer note and `telemetry_line` strings from
  "off — paste a key to enable" to reflect on-by-default
  (e.g. "Sending anonymous connection diagnostics. Toggle off to stop.").

Wiring touchpoints: `app/src/main.rs` (`services_configured`, initial
`TelemetryState`), `app/src/node/mod.rs` (`start_services_client` signature
takes `disabled` + `custom`; the `SaveApiSecret` command path gains/parallels a
telemetry-toggle command that rebuilds or drops the client).

## 4. First-run notice (`app/src/first_run.rs` + its view)

Fold a one-line telemetry disclosure into the **existing first-run trust note**
(a single dismissable note, not a second modal):

> "iroh doctor sends anonymous connection diagnostics to help improve iroh.
> You can turn this off in Diagnostics."

One dismissal covers both the trust note and the telemetry disclosure.

## 5. Docs + store declarations

- **`plans/app-store-privacy-policy-draft.md`** — rewrite the
  "Optional telemetry (off by default)" section to: on by default, anonymous
  connection diagnostics to iroh-services, with an in-app opt-out.
- **`plans/app-store-listing-copy.md`** (data-safety bullet, ~line 77) — flip
  from "Optional, off-by-default telemetry … if the user supplies a key" to
  on-by-default with opt-out.
- **`plans/app-store-data-safety-forms.md`** — move the "shipped behavior"
  pointer from Scenario B to **Scenario C** (already drafted in the doc).
  Keep Play's "optional" flag, justified by the opt-out toggle. Note Apple has
  no optional flag, so Device ID + Performance Data are simply "Collected,
  not linked to identity, not used for tracking."
- **`app/ios/PrivacyInfo.xcprivacy`** — add `NSPrivacyCollectedDataTypes`
  entries:
  - Device ID — collected, not linked to identity, not used for tracking,
    purposes App Functionality + Analytics.
  - Performance Data (Diagnostics) — same flags/purposes.
  - `NSPrivacyTracking` stays `false`; `NSPrivacyTrackingDomains` stays empty.

## 6. Testing

- **Core:** extend the `resolve_api_secret` precedence test for the
  `AppDefault` cases — default-on, disabled, non-empty custom key, env override,
  and env empty-opt-out.
- **App:** marker roundtrip test mirroring `first_run` (default reads as
  enabled; set-disabled persists; idempotent).

## Out of scope

- Third-party analytics SDKs (Firebase/PostHog/etc.).
- Changing what data iroh-services collects; this only changes the default
  on/off state and the surrounding disclosure/controls.
- Store-console (App Store Connect / Play Console) install analytics, which are
  server-side and need no code.
