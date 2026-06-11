# iroh-doctor-app

A Dioxus 0.7 desktop tool for debugging live iroh connections and
exercising the iroh-gossip protocol against another peer. Connecting to
a peer runs the same latency and throughput monitor as
`iroh-doctor connect`.

## Tabs

- **Connect**: dial a peer by endpoint id and monitor the live connection.
  - Connect/disconnect is a distinct step: once connecting or connected the
    id input gives way to a Disconnect (or Cancel) button.
  - High-level connection state, live RTT with an SVG sparkline of the last
    30 s, the set of QUIC paths (IP, relay, or custom) with per-path RTT,
    time-to-first-direct-byte, periodic throughput, and a connection-event
    log of the last 50 state transitions.
- **Diagnostics**: the local network environment, independent of any peer.
  - Local network report: a one-shot `endpoint.net_report()` probe with
    a NAT classification (Easy / Medium / Hard / Unknown), IPv4 and IPv6
    visibility, mapping variation, captive-portal status, and the
    preferred relay.
  - Direct port-map probe: UPnP/PCP/NAT-PMP availability.
  - Relay latency: per-relay TLS connect time plus a one-shot relay
    protocol ping, sorted by ping with failures at the bottom.
  - Services diagnostics: the iroh-services-backed view, kept side-by-side
    so a disagreement with the direct probes is visible.
  - iroh-services API key and telemetry status.
- **Gossip**: join an iroh-gossip topic (paste a 64-hex topic id, or
  type any string and the app hashes it deterministically with
  BLAKE3 so both peers converge); see neighbors; broadcast UTF-8
  messages; watch incoming messages with neighbor-change events.

Connecting dials the probe protocol and runs the active monitor: it pings
for latency, uploads periodically for throughput, and drives the Connect
graph. It shares the `iroh_doctor_core::monitor::run` composition with
`iroh-doctor connect`, so the cli and the app report a connection
identically.

## Multi-protocol surface

`node/` binds one `iroh::Endpoint` that advertises two ALPNs:
`iroh-gossip::ALPN` (`/iroh-gossip/1`) and the probe ALPN
(`iroh-pong-probe/0`) so `iroh-doctor connect` works against this app. The
accept loop dispatches per ALPN: gossip spawns `Gossip::handle_connection`
and the probe spawns `probe::handle_connection_with`, each per connection
so one cannot wedge the accept of another. An incoming probe is also
surfaced through `conn_slot` so the Connect view shows it like an
outgoing dial.

## Trust model

iroh-doctor-app serves the gossip and probe protocols to any peer that
can dial our endpoint id. Endpoint ids are not secret. Treat this app as
a peer-to-peer debug tool to hand to a known collaborator over a side
channel, not as a service to leave exposed.

## Build and run

Requires Rust 1.91+ and the Dioxus CLI:

```sh
curl -sSL https://dioxus.dev/install.sh | sh
```

Then from this directory:

```sh
dx serve --platform desktop
```

The first run drops a 32-byte secret key under
`~/Library/Application Support/iroh-doctor-app/secret_key.bin` (macOS)
or the platform-equivalent config directory. Delete that file to
reroll your endpoint id.

To see info-level traces from iroh and the app:

```sh
RUST_LOG=info,iroh_doctor_app=debug dx serve --platform desktop
```

(The binary calls `tracing_subscriber::fmt().with_env_filter(...).init()`
with the same fallback filter when `RUST_LOG` is unset.)

### Android

Install the SDK (with platform-tools + emulator), an NDK, and a JDK
(Android Studio bundles a suitable one as `jbr`). Export, adjusting the
NDK version and `JAVA_HOME` to your install:

```sh
export ANDROID_SDK_ROOT=$ANDROID_HOME
export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/28.0.12674087
export NDK_HOME=$ANDROID_NDK_HOME
export JAVA_HOME=/opt/android-studio/jbr
export PATH=$ANDROID_HOME/platform-tools:$ANDROID_HOME/cmdline-tools/latest/bin:$ANDROID_HOME/emulator:$PATH
```

Then:

```sh
dx serve --platform android
```

`dx` shells out to `adb` without `-s`, so if you have more than one
device attached (e.g. a physical phone *and* an emulator) it fails with
`adb: more than one device/emulator`. `adb` honours `ANDROID_SERIAL`, so
pick the target by serial (from `adb devices`):

```sh
ANDROID_SERIAL=<serial> dx serve --platform android
```

Unlike desktop/iOS, Android has no XDG home and `dirs::config_dir()`
returns `None`, so the app reads its private files dir off the Android
`Context` over JNI (see `identity::config_base`). State lives under that
dir's `iroh-doctor-app/` rather than a user-visible config path.

#### Logs

An Android app's stdout/stderr goes to `/dev/null`, so the fmt stdout
layer is invisible there. The tracing subscriber instead writes to logcat
under the tag `iroh-doctor-app` (see `init_logging`). `dx serve` streams
logcat, so the logs show up in the terminal you ran it from. To watch them
directly:

```sh
adb logcat -s iroh-doctor-app
```

(add `ANDROID_SERIAL=<serial>` if more than one device is attached). The
default filter is `info,iroh_doctor_app=debug`.

## Project layout

```
src/
  main.rs                 - Dioxus app and tab routing
  node/
    mod.rs                - iroh endpoint, command pump, services telemetry
    accept.rs             - accept loop (per-ALPN dispatch)
    monitor.rs            - active connection monitor + path snapshots
    gossip.rs             - gossip join + topic-id parsing
  identity.rs             - on-disk secret key + api secret override
  endpoints.rs            - saved-endpoints store (the Endpoints tab)
  portmap_probe.rs        - UPnP/PCP/NAT-PMP probe wrapper
  relay_probe.rs          - per-relay connect+ping probe
  diagnostics_export.rs   - diagnostics zip bundle
  components/
    mod.rs                - shared helpers (short_id)
    connection.rs         - Connect view (state, RTT, paths, events)
    diagnostics.rs        - Diagnostics view (net report, relays, port-map, services)
    diag_state.rs         - DiagState + the trigger_* probe dispatchers
    gossip.rs             - Gossip tab
    endpoints.rs          - Endpoints tab
    error_dialog.rs       - global error modal
assets/
  styling/main.css        - all styles
  favicon.ico
```

## Pins

```toml
iroh           = "=1.0.0-rc.1"
iroh-services  = "=1.0.0-rc.1"
iroh-blobs     = "=0.102.0"
iroh-gossip    = "=0.100.0"
```

The `=` pins are deliberate: this branch validates against a
specific iroh rc and the protocol crates that match it. Loosen
the pins after iroh leaves rc.
