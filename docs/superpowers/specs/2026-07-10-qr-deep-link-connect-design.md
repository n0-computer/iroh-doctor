# QR share + deep-link connect

**Status:** design approved 2026-07-10
**Scope:** `app/` (Dioxus desktop + mobile). No changes to `core/` or `cli/`.

## Goal

Let two devices exchange endpoint ids without copy-pasting a 64-hex string:

1. Any device (desktop or mobile) can **display its endpoint id as a QR code**.
2. A mobile device can **scan** another device's QR with the phone's **system
   camera**, which opens iroh-doctor with the peer id prefilled, ready to connect.

This covers both desktop→mobile and mobile→mobile exchange. Scanning is a
system-camera + deep-link flow, so the app writes **no in-app camera code**.

## Non-goals

- In-app camera / getUserMedia / native camera capture. Explicitly rejected in
  favor of the system camera + deep link.
- Universal links / associated domains. Custom URL scheme only (see below).
- Auto-dial on link open. The link **prefills**; the user taps Connect.
- Desktop scanning. Desktop only displays.

## Two independent pieces

The work splits into two pieces that ship in order. Piece 1 is trivial and
cross-platform; Piece 2 is the platform-glue-heavy scanner path.

---

## Piece 1 — QR display (all platforms)

**Dependency:** add `qrcode` (pure Rust, has a built-in SVG renderer) to
`app/Cargo.toml` for all targets.

**UI:** in `components/connect_page.rs`, the `Header` (which shows "My id:" +
Copy) gains a **"Show QR"** toggle. When on, it renders the endpoint id's QR as
an inline SVG below the id.

- The SVG string comes from `qrcode` and is injected with
  `dangerous_inner_html`. The content is the device's own id — trusted input,
  so "dangerous" html is acceptable here. (One `demo()`/unit check that the
  encoder produces non-empty SVG for a sample id guards the glue.)
- The QR encodes the **deep-link URL** (`irohdoctor://connect?id=<hex>`), not
  the bare id — so a mobile scanning it deep-links (Piece 2). A desktop with no
  scanner still just displays; that's fine.
- Rendered size ~200px so it scans reliably. Toggle state is local component
  state; nothing persisted.

**Platform coverage:** identical on desktop / iOS / Android. No JS, no
permissions. This is the entire "show my id as a QR" feature and ships first.

## Piece 2 — deep-link connect (mobile scanner path)

### QR content / URL scheme

- Scheme: **custom** `irohdoctor://connect?id=<hex>`.
- Custom over universal links because it needs no hosted
  `apple-app-site-association` / `assetlinks.json`, no owned domain, and no
  associated-domains entitlement. Both devices in a face-to-face exchange
  already have the app installed, so the "app not installed" degradation of a
  custom scheme doesn't matter here.
- The bundle id is `com.number0.irohdoctor` (Dioxus.toml). The scheme string is
  `irohdoctor`.

### Registering the scheme

dx 0.7.9 has a first-class manifest hook for this. A single entry in
`Dioxus.toml` registers the scheme on both platforms:

```toml
[deep_links]
schemes = ["irohdoctor"]
```

The dx CLI merges this into the iOS `CFBundleURLTypes` (via the plist template)
and into an `<intent-filter>` (VIEW / DEFAULT / BROWSABLE) on the Android
`MainActivity` (via the manifest template). No `bundle-mobile.sh` patch is
needed, and the config survives the wrapper: `bundle-mobile.sh` never touches
`CFBundleURLTypes` or `AndroidManifest.xml`. Because the scheme lands in the dx
template, it is present in plain `dx build` / `dx serve` too, so deep links can
be tested in ordinary dev builds.

**Verification is mandatory.** dx 0.7.9 has silently ignored config keys before
(`[application] name`, `[ios] deployment_target`; see `plans/learnings.md`).
After the first build, grep the generated `Info.plist` and `AndroidManifest.xml`
for the scheme. Fallbacks if `[deep_links]` proves inert: iOS via `[ios.raw]
info_plist` raw XML; Android via a post-build manifest patch in the wrapper.

### Capturing the incoming URL → Rust

A new `deeplink` module holds the platform-independent parse
(`connect_url(id)` / `parse_connect_url(url)`) and the per-platform capture,
mirroring how `clipboard` / `android` isolate platform glue. A captured id sets
the existing `peer_id_input` signal and switches the app to the Connect tab.

- **iOS:** tao already forwards custom-scheme opens as
  `tao::event::Event::Opened { urls }`; Dioxus surfaces raw tao events through
  `use_wry_event_handler`. The handler parses the URL and sets the signals. No
  objc2 delegate work. Warm start (app running) works live. Cold start relies on
  tao replaying its pre-launch event queue to the handler registered at startup;
  this is the one point not proven from source, mitigated only if device testing
  shows a dropped launch event (buffer the URL in a global drained on mount).
- **Android (cold start):** the `ndk_context` context is the `WryActivity`
  itself, so `getIntent().getData()` works over the existing `android::with_env`
  JNI helper, read once at startup. This is the primary scan-then-launch path.
- **Android (warm start):** tao does not forward `onNewIntent` (wry #1563 is
  open), so a scan while the app is already running needs native glue:
  `launchMode="singleTask"`, an `onNewIntent`/`setIntent` override in
  `MainActivity.kt`, and a JNI `extern` callback into Rust, all patched
  post-build in `bundle-mobile.sh`. This is the only hard part and is
  deferrable: cold start already covers the app-not-running case.

### Behavior on open

Prefill `peer_id_input` with the parsed id and switch to the Connect tab. **No
auto-dial** — the user reviews and taps Connect. Auto-connect is a later
one-line change (fire `NodeCommand::Connect`) if desired.

### Input robustness (freebie)

Teach the Connect input's parse to accept either a bare 64-hex id **or** a full
`irohdoctor://connect?id=<hex>` URL (extract and validate the `id` param). This
means a text-only QR scanner + Paste still works, and the deep-link handler and
the manual input share one parse path. Covered by a unit test over both forms
plus a malformed case.

## Build order

Ordered so each step compiles, is independently useful, and defers the one hard
part to the end.

1. **QR display everywhere** (Piece 1). Ships value immediately, zero platform
   risk. Introduces `deeplink::connect_url`.
2. **Accept a connect URL in the peer-id input** (the robustness freebie). Adds
   `deeplink::parse_connect_url` and wires it into the input's parse, so a URL
   pasted or scanned-as-text connects. Pure logic, unit-tested, no native code.
3. **Deep-link connect** (Piece 2, cold start). `[deep_links]` config + iOS
   `use_wry_event_handler` + Android launch-intent read, funneled into
   `peer_id_input` + the Connect tab. Verify the scheme lands in the generated
   manifests.
4. **Android warm-start glue** (optional). The `onNewIntent` / `singleTask` /
   JNI-callback path. Deferrable; decide at plan approval whether v1 includes it.

## Files touched

- `app/Cargo.toml` — add `qrcode` (svg-only features).
- `app/Dioxus.toml` — `[deep_links] schemes = ["irohdoctor"]`.
- `app/src/deeplink.rs` (new) — `connect_url` / `parse_connect_url`; the iOS
  `use_wry_event_handler` hook; the Android startup read.
- `app/src/components/connect_page.rs` — "Show QR" toggle + SVG render; the
  input parse accepts the URL form.
- `app/src/android.rs` — JNI read of the launch intent URL (+ warm-start
  callback in step 4).
- `app/src/main.rs` — register the deep-link hook; funnel the id into
  `peer_id_input` and switch to the Connect tab.
- `app/assets/styling/main.css` — QR container styling.
- `app/scripts/bundle-mobile.sh` — only in step 4 (warm-start Kotlin/manifest
  patch); not needed for registration.

## Testing

- Unit: URL parse accepts bare id + URL form, rejects malformed (in the shared
  parse path).
- Unit/demo: `qrcode` produces non-empty SVG for a sample id.
- Manual: QR displays on desktop; bundle mobile, scan desktop QR with a phone's
  system camera, confirm the app opens with the id prefilled on the Connect tab
  (Android first, then iOS).
