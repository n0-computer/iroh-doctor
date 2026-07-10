# Plan: QR share + deep-link connect

## Checklist

- [x] Write plan
- [x] Step 1: QR display (qrcode dep, `deeplink::connect_url`, Header toggle, CSS)
- [x] Step 2: peer-id input accepts a connect URL (`deeplink::parse_connect_url`)
- [x] Step 3: deep-link connect, cold start (`[deep_links]`, iOS + Android capture)
- [x] Step 3v: scheme verified in generated Android manifest + iOS Info.plist
- [x] Step 4: Android warm-start glue (Rust + bundle-mobile.sh; APK build verified)
- [x] Final review (4-reviewer staff pass; survivors fixed in 75cd4c9, 67009e0)

## Goal

Two devices exchange endpoint ids without copy-pasting a 64-hex string. Any
device shows its id as a QR code; a phone scans it with the system camera and
iroh-doctor opens with the peer id prefilled on the Connect tab.

Design: `docs/superpowers/specs/2026-07-10-qr-deep-link-connect-design.md`.

## Context

The Connect page (`app/src/components/connect_page.rs`) has a `Header` showing
the device's own endpoint id with a Copy button, and a `ConnectBar` with the
peer-id text input (validated by `EndpointId::from_str`), Paste, and Connect.
`peer_id_input` and `current_tab` are both signals owned by `App`
(`app/src/main.rs`), so a deep-link handler at that level can set both.

Platform glue already follows two patterns: a JS bridge via `document::eval`
(clipboard) and native per-target code (`android.rs` JNI via `with_env`,
UIPasteboard via objc2 on iOS). The app is a Dioxus 0.7.9 / tao 0.34.8 / wry
0.53.5 webview app using `anyhow` for errors.

Research (worklog 2026-07-10, verified against upstream source) established:
`[deep_links]` in Dioxus.toml registers the scheme on both platforms; iOS
capture is `Event::Opened` via `use_wry_event_handler`; Android cold-start
capture is `getIntent().getData()` over the existing JNI helper (the
`ndk_context` context is the Activity); only Android warm-start needs native
glue.

## Approach

### Step 1: QR display (feat)

- Add `qrcode = { version = "0.14", default-features = false, features =
  ["svg"] }` to `app/Cargo.toml` (all targets; drops the default `image`
  feature to keep the dep light).
- New `app/src/deeplink.rs` with `pub fn connect_url(id: &str) -> String`
  returning `irohdoctor://connect?id=<id>`. Declare `mod deeplink;` in main.rs.
- In `Header` (connect_page.rs): a `use_signal(|| false)` toggle and a "QR"
  button next to Copy. When on, render `qrcode::QrCode::new(connect_url(&id))`
  as an SVG string (`.render::<svg::Color>()...`) into a `div` via
  `dangerous_inner_html`. Content is the device's own id, so the SVG is trusted.
  Disable the button while the id is empty.
- `assets/styling/main.css`: a `.qr-panel` block (centered, sized ~200px, light
  background so the code scans in dark mode).
- Tests (deeplink.rs): `connect_url` shape; QR encodes without error and yields
  non-empty SVG for a sample 64-hex id.
- Verify: build + run the desktop app, toggle the QR, confirm it renders and a
  phone camera decodes it to the URL.

### Step 2: input accepts a connect URL (feat)

- `deeplink::parse_connect_url(input: &str) -> Option<String>`: if `input`
  parses as an `irohdoctor://connect?...` URL with an `id` query param, return
  that id; otherwise `None`. Keep it dependency-light (manual prefix/split or
  the `url` crate only if already in the tree; check first).
- In `ConnectBar`, resolve the id once: `parse_connect_url(trimmed).unwrap_or
  (trimmed)` before the `EndpointId::from_str` validation and before sending
  `NodeCommand::Connect`, so pasting/typing either a bare id or a full URL
  works. Single parse path shared with the deep-link handler.
- Tests: bare id passes through; full URL yields the id; malformed URL and
  wrong-scheme return None; the resolved value still fails `EndpointId::from_str`
  when the id is malformed.

### Step 3: deep-link connect, cold start (feat)

- `Dioxus.toml`: `[deep_links] schemes = ["irohdoctor"]`.
- iOS (and desktop, harmless): in `deeplink.rs`, a `use_deep_link` hook that
  calls `use_wry_event_handler`, matches `Event::Opened { urls }`, runs
  `parse_connect_url`, and on a hit sets `peer_id_input` + `current_tab =
  Connect`. Gate to `any(feature = "mobile", feature = "desktop")`.
- Android: `android::launch_deep_link() -> Option<String>` reading
  `getIntent().getData().toString()` via `with_env`; a `use_effect`/`use_hook`
  in `App` (android-only) reads it once at startup and applies it through the
  same funnel.
- Wire both into `App` via one `deeplink::use_connect_links(peer_id_input,
  current_tab)` entry point.

### Step 3v: verify scheme registration

- `dx build --ios` and `dx build --android` (or via `bundle-mobile.sh`), then
  grep the generated `Info.plist` for `CFBundleURLSchemes`/`irohdoctor` and the
  generated `AndroidManifest.xml` for the `<data android:scheme="irohdoctor">`
  intent-filter. If absent, apply the fallback (iOS `[ios.raw] info_plist`;
  Android manifest patch in the wrapper) and record it in learnings.md.

### Step 4 (optional): Android warm-start glue (feat)

Only if v1 must handle a scan while the app is already backgrounded on Android.

- `launchMode="singleTask"` + an `onNewIntent`/`setIntent` override + an
  `external fun newDeepLink` in a patched `MainActivity.kt`; a matching
  `#[no_mangle] extern "C"` in Rust pushing into a global the UI drains; all
  patched post-build in `bundle-mobile.sh` (dx regenerates these files each
  build). Keep the wrapper idempotent (see learnings.md).

## Risks and open questions

- **`[deep_links]` may be inert in dx 0.7.9** (history of ignored keys). Step 3v
  gates this; fallbacks are known.
- **iOS cold-start event ordering** is unproven from source. If a launched-from-
  cold URL is dropped, buffer it in a global drained on mount. Cheap to add;
  deferred until a device shows the drop.
- **Android warm-start** is genuinely unsupported upstream (wry #1563). Handled
  as the optional Step 4; cold start covers app-not-running.
- **Native paths are not unit-testable** without a device/emulator. Logic lives
  in `connect_url`/`parse_connect_url` (unit-tested); the JNI/tao glue is thin
  plumbing verified by manual on-device test.
- **`url` crate availability**: check the tree before using it in Step 2; tao
  pulls in `url`, but the app crate should not rely on a transitive dep. Prefer
  a small manual parse unless `url` is already a direct app dependency.

## Commit strategy

One commit per step, each compiling and passing checks on its own: `cargo
clippy --workspace --all-targets --all-features`, `cargo test`, and `cargo fmt`
(plain `cargo fmt`, not `cargo make format`, which reformats the whole repo;
see MEMORY). Confirm which `cargo make` targets exist before relying on them.

1. `feat(app): show endpoint id as a scannable QR code`
2. `feat(app): accept a connect URL in the peer id input`
3. `feat(app): connect via irohdoctor:// deep links`
4. `feat(app): handle deep links while running on Android` (optional)

Tests live in the same commit as the code they exercise. `plans/` is committed
in this project, so the plan and worklog are committed; do not bundle them with
code commits.
