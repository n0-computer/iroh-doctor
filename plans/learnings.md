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

- `core::services::resolve_api_secret` takes a `SecretSource` enum on
  purpose: `BundledDefault` (cli) falls back to the embedded key,
  `SavedOverride("")` (app, no key) resolves to `None` so telemetry
  stays off. The app must never use `BundledDefault`; that would push
  metrics on first launch and break the privacy-policy promise. This was
  a real shipped bug before 2026-06-09.

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
