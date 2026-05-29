# iroh-doctor-app

A Dioxus 0.7 desktop tool for debugging live iroh connections and
exercising the iroh-gossip protocol against another peer. Connecting to
a peer runs the same latency and throughput monitor as
`iroh-doctor connect`.

## Tabs

- **Diagnostics**: the live state of one or more iroh connections,
  plus every diagnostic an `iroh-doctor` user would reach for, in
  one tab.
  - Live RTT, an SVG sparkline of the last 30 s of samples, and the
    set of QUIC paths (IP, relay, or custom) with their per-path RTT.
  - Connection-event log: the last 50 state transitions with relative
    timestamps.
  - Local network report: a one-shot `endpoint.net_report()` probe with
    a NAT classification (Easy / Medium / Hard / Unknown), IPv4 and IPv6
    visibility, mapping variation, captive-portal status, and the
    preferred relay.
  - Direct port-map probe: UPnP/PCP/NAT-PMP availability via the
    `portmapper` crate, independent of iroh-services.
  - Relay latency: per-relay TLS connect time plus a one-shot relay
    protocol ping, sorted by ping with failures at the bottom.
  - Services diagnostics: the legacy iroh-services-backed view, kept
    side-by-side so a disagreement with the direct probes is visible.
  - iroh-services API key and telemetry status.
- **Gossip**: join an iroh-gossip topic (paste a 64-hex topic id, or
  type any string and the app hashes it deterministically with
  BLAKE3 so both peers converge); see neighbors; broadcast UTF-8
  messages; watch incoming messages with neighbor-change events.

Entering a peer's endpoint id and hitting Connect dials the probe
protocol and runs the active monitor: it pings for latency, uploads
periodically for throughput, and drives the Diagnostics graph. This is
the same `iroh_doctor_core::probe::run_client` loop `iroh-doctor
connect` uses, so the cli and the app report a connection identically.

## Multi-protocol surface

`peer.rs` binds one `iroh::Endpoint` that advertises two ALPNs:
`iroh-gossip::ALPN` (`/iroh-gossip/1`) and the probe ALPN
(`iroh-pong-probe/0`) so `iroh-doctor connect` works against this app. The
accept loop dispatches per ALPN: gossip spawns `Gossip::handle_connection`
and the probe spawns `probe::handle_connection_with`, each per connection
so one cannot wedge the accept of another. An incoming probe is also
surfaced through `conn_slot` so the Diagnostics view shows it like an
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

## Project layout

```
src/
  main.rs                 - Dioxus app and tab routing
  peer.rs                 - iroh endpoint, accept loop, command pump, monitor
  identity.rs             - on-disk secret key + api secret override
  endpoints.rs            - saved-endpoints store (the Endpoints tab)
  portmap_probe.rs        - UPnP/PCP/NAT-PMP probe wrapper
  relay_probe.rs          - per-relay connect+ping probe
  diagnostics_export.rs   - diagnostics zip bundle
  components/
    mod.rs                - shared helpers (short_id)
    diagnostics.rs        - Diagnostics tab (paths, RTT, events, report)
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
