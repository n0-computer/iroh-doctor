# iroh-doctor-app

A Dioxus 0.7 desktop tool for debugging live iroh connections and
exercising the iroh-blobs, iroh-gossip, and iroh-docs protocols
against another peer. The original Pong game lives on as one tab.

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
- **Data**: Blobs and Docs sit together on one tab.
  - Blobs: generate a salted-zero buffer of 1 KiB to 1 GiB into an
    in-memory blob store, copy the hash to another peer, pull blobs
    by hash and endpoint id, see elapsed time and throughput per blob.
  - Docs: create a new iroh-docs document or import one from a write
    ticket, share the active doc as a write ticket, write KV entries
    under the default author, watch local inserts, remote inserts,
    sync-finished, and content-ready events stream in.
- **Gossip**: join an iroh-gossip topic (paste a 64-hex topic id, or
  type any string and the app hashes it deterministically with
  BLAKE3 so both peers converge); see neighbors; broadcast UTF-8
  messages; watch incoming messages with neighbor-change events.
- **Pong**: the original two-player paddle game over a custom pong
  ALPN.

## Multi-protocol surface

`peer.rs` binds one `iroh::Endpoint` that advertises six ALPNs:
the pong ALPN (`iroh-helloiroh-pong/0`), `iroh-blobs::ALPN`
(`/iroh-bytes/4`), `iroh-gossip::ALPN` (`/iroh-gossip/1`),
`iroh-docs::ALPN` (`/iroh-sync/1`), the probe ALPN
(`iroh-pong-probe/0`), and the iroh-doctor protocol (`n0/doctor/1`)
so `iroh-doctor connect` works against this app. The accept loop
dispatches per ALPN: pong adopts a single replaceable session via
`conn_slot`; the others spawn the matching `ProtocolHandler::accept`
(or `Gossip::handle_connection`, `peer_probe::handle_connection`, or
`doctor::handle_connection`) per connection so any one transfer
cannot wedge the accept of another.

`MemStore` is ephemeral and the peer task does not call
`shutdown().await` on exit, so closing the app discards all
generated and downloaded blobs and any in-memory doc state. That
is intentional for a debug session.

## Trust model

iroh-doctor-app serves blobs, gossip, and docs to any peer that can dial
our endpoint id. Endpoint ids are not secret, but the content
addressing means a remote needs the 32-byte hash to pull anything
from the blobs store, and the doc protocol still rejects writes
without the right capability. Treat this app as a peer-to-peer
debug tool to hand to a known collaborator over a side channel,
not as a service to leave exposed.

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
  peer.rs                 - iroh endpoint, accept loop, command pump
  game.rs                 - Pong game state machine
  identity.rs             - on-disk secret key + api secret override
  endpoints.rs            - saved-endpoints store (the Endpoints tab)
  nat.rs                  - NAT classifier (Easy/Medium/Hard/Unknown)
  peer_probe.rs           - probe ALPN responder
  doctor.rs               - iroh-doctor connect/accept responder
  portmap_probe.rs        - UPnP/PCP/NAT-PMP probe wrapper
  relay_probe.rs          - per-relay connect+ping probe
  diagnostics_export.rs   - diagnostics zip bundle
  wire.rs                 - Pong wire format
  components/
    mod.rs                - shared helpers (short_id)
    diagnostics.rs        - Diagnostics tab (paths, RTT, events, report)
    blobs.rs              - Blobs section
    gossip.rs             - Gossip tab
    docs.rs               - Docs section
    endpoints.rs          - Endpoints tab
    error_dialog.rs       - global error modal
    pong_scene.rs         - Pong tab
assets/
  styling/main.css        - all styles
  favicon.ico
```

## Pins

```toml
iroh           = "=1.0.0-rc.1"
iroh-base      = "=1.0.0-rc.1"
iroh-relay     = "=1.0.0-rc.1"
iroh-services  = "=1.0.0-rc.1"
iroh-blobs     = "=0.102.0"
iroh-gossip    = "=0.100.0"
iroh-docs      = "=0.100.0"
portmapper     = "0.18"
```

The `=` pins are deliberate: this branch validates against a
specific iroh rc and the protocol crates that match it. Loosen
the pins after iroh leaves rc.
