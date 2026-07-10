# Learnings

Project-specific lessons: surprising behaviors and subtle invariants an
agent should know before touching this code. Append as you discover.

## dx 0.7.9 mobile gotchas

- **The bundle wrapper is not idempotent unless it pre-cleans.** dx
  regenerates `ic_launcher.webp` on every `dx build --android` but does
  NOT remove the `ic_launcher.png` that `scripts/bundle-mobile.sh`
  injected on a prior run. A second build then has both and dx's own apk
  assembly fails with "Duplicate resources". The wrapper removes its
  prior PNG injections before `dx build` to stay idempotent; keep that
  if you touch the icon-injection logic.
- **dx hardcodes `UILaunchStoryboardName = LaunchScreen` in its iOS
  plist template but generates no storyboard**, so an unmodified build
  launches to a blank screen. The wrapper compiles a storyboard with
  `ibtool`. Verified by reading dx's `assets/ios/ios.plist.hbs`.
- **versionCode is hardcoded to 1** and CFBundleVersion reuses the crate
  version, both rejected by the stores on re-upload. The wrapper stamps
  `BUILD_NUMBER` post-build (PlistBuddy / sed). There is no dx config
  hook for either in 0.7.9.
- dx regenerates the whole native project from templates each build, so
  editing anything under `target/dx/.../{ios,android}` by hand does not
  survive. All branding goes through the wrapper.
- **`[deep_links] schemes = [...]` in `Dioxus.toml` IS honored by dx 0.7.9**
  (unlike `[application] name` and `[ios] deployment_target`). Verified in the
  generated `AndroidManifest.xml` (a VIEW/DEFAULT/BROWSABLE `<intent-filter>`
  with `<data android:scheme=...>` on MainActivity) and the iOS `Info.plist`
  (`CFBundleURLTypes` -> `CFBundleURLSchemes`). It survives `bundle-mobile.sh`,
  which touches neither. So cold-start deep links need no post-build patch.
- **tao does not forward Android `onNewIntent`** (wry #1563), so a warm-start
  deep link (app already running) needs glue: `launchMode="singleTask"` on the
  activity plus an `onNewIntent`/`setIntent` override in `MainActivity.kt` that
  calls a JNI `external fun` into Rust. `bundle-mobile.sh` patches both after
  `dx build` (dx regenerates them each build). iOS needs none of this: tao
  delivers custom-scheme opens as `Event::Opened`, live and on cold start.
- **To compile-check the android target on the host** (no full `dx build`):
  the rust targets are installed, but a C dep (`ring`) needs the NDK clang. Set
  `ANDROID_NDK_HOME`, then `CC_aarch64_linux_android`,
  `AR_aarch64_linux_android` (llvm-ar), and
  `CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER` to the NDK toolchain
  (`.../ndk/<ver>/toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android24-clang`),
  then `cargo check -p iroh-doctor-app --no-default-features --features mobile
  --target aarch64-linux-android`. This exercises the `target_os = "android"`
  code paths that plain host builds skip.

## NAT classification

- **`classify_nat_type` returns `Easy` only with per-destination-port
  variation data, which iroh's net_report cannot collect** (relays serve
  QAD on one UDP port, so net_report only measures the destination-
  *address* axis). The port axis needs one host reachable on two ports:
  `core::port_variation` ships a QAD helper server plus a same-socket
  probe for exactly this. A multi-relay approach cannot work.
- **The probe must validate that its packets crossed the NAT.** It only
  counts an observation whose observed external IP matches the base
  report's public IP. Without this a loopback or same-LAN helper reports
  a private address, matches trivially across ports, and fakes `Easy`.
  If you change `compute_port_variation`, keep the expected-address
  filter.
- The family combine is **optimistic** (easier family wins): P2P
  succeeds if either IPv4 or IPv6 can holepunch. Do not mix the
  destination-address axis of one family with the port axis of another.

## App services / telemetry

- Telemetry is on by default in the app as of 2026-06-10: iroh doctor is a
  diagnostics tool and collects anonymous connection metrics out of the box,
  with an in-app opt-out. (This reverses the 2026-06-09 opt-in decision; the
  privacy docs and store declarations were updated to match.)
- `core::services::resolve_api_secret` takes a `SecretSource` enum on purpose.
  `BundledDefault` (cli) always uses the embedded key. `AppDefault { disabled,
  custom }` (app) uses the bundled key by default and resolves to `None` only
  when the user disabled telemetry or set `IROH_SERVICES_API_SECRET=""`; a
  non-empty `custom` overrides the bundled key. The env var wins over both.
- The app's opt-out is a `telemetry_disabled` marker file (`telemetry_pref.rs`),
  absent = on, mirroring the `first_run` trust-note marker. Toggling it sends
  `NodeCommand::SetTelemetryEnabled`, which drops and rebuilds the services
  client so pushes stop immediately rather than at the next restart.

## App node concurrency

- The paths sampler and the dial monitor share the latency graph. A
  dial plots probe ping round-trips; an incoming probe plots path RTT.
  The `dial_active` AtomicBool gates which source feeds the graph. It is
  pump-managed (set true on Connect after aborting the prior monitor,
  cleared on Disconnect and at the monitor's natural end), which is
  load-bearing: putting `store(true)` before the abort, or clearing via
  a drop guard, reintroduces a clobber race.
- `conn_slot` is keyed on peer id, not connection identity, so a
  reciprocal dial (you dial a peer who is also probing you) can clear
  the wrong connection on cleanup. Pre-existing; not yet fixed.
