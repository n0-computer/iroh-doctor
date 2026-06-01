# Design: shared `core` crate + a live `iroh-doctor connect`

> Status: implemented on rae/doctor-app (commits 5e2c100..1c41ac9),
> reviewed (two rounds), 110 workspace tests green. Not pushed. See
> plans/worklog.md and plans/pr-notes.md.


## Goal

Extract the genuinely shared code between the `cli` and `app` crates into a
new `core` crate so the two stop drifting out of hand-maintained parity,
and rebuild `iroh-doctor connect` as a live connection monitor that matches
the app's connect view, powered by the shared probe protocol.

## Context

The workspace has `cli/` (the iroh-doctor CLI) and `app/` (the
iroh-doctor-app Dioxus GUI). They carry parallel implementations of the
same ideas:

- The doctor connection-test wire protocol (`TestStreamRequest`, the
  `n0/doctor/1` ALPN) is byte-identical in both.
- NAT classification: the `NatType` enum is identical; the `classify`
  logic is parallel but divergent (the CLI's takes an
  `ExtendedNetworkReport` with per-destination-port variation; the app's
  takes a plain `iroh::NetReport`). The app comment already says it is
  "matching iroh-doctor's classifier."
- The peer-probe protocol (`Frame`, the `iroh-pong-probe/0` ALPN, codec,
  responder) lives only in the app today; the CLI has no counterpart.

Today `iroh-doctor connect` dials, logs connection changes, and runs the
doctor test's passive side. The app's connect button instead shows a live
monitor: connection state (relay/direct), the path table, time-to-first-
direct-byte, and latency over time.

## Approach

### The `core` crate (`iroh-doctor-core`)

A small library depending only on the shared essentials: `iroh`, `serde`,
`postcard`, `anyhow`, `tokio`, `tracing`, `n0-future`. No `dioxus`,
`indicatif`, or `clap`, so neither GUI nor CLI concerns leak in.

```
core/src/
  lib.rs       pub mod nat; pub mod doctor; pub mod probe;
  nat.rs       NatType + ExtendedNetworkReport + classify() + classify_base()
  doctor.rs    TestStreamRequest + the n0/doctor/1 ALPN (wire types only)
  probe.rs     Frame + iroh-pong-probe/0 ALPN + frame codec
               + handle_connection (passive responder: echo pings, drain upload)
               + a client: continuous ping loop (RTT samples) and an
                 upload-throughput measurement
```

- **nat**: the CLI's richer `classify(&ExtendedNetworkReport) -> NatType`
  becomes canonical, with `classify_base(&iroh::NetReport) -> NatType` for
  the app's simpler case. `ExtendedNetworkReport` moves here from
  `cli/src/swarm/net_report_ext.rs`. Both crates' NAT tests merge here and
  must stay green; any divergence is a real parity bug to resolve.
- **doctor**: wire types only. The two responders stay in their crates:
  the CLI's is woven into its indicatif progress UI, the app's is timeout-
  only, and unifying them would mean reworking the CLI's GUI-coupled
  `passive_side`, which is out of scope.
- **probe**: the app's `peer_probe.rs` moves here wholesale and gains a
  client. The protocol keeps both behaviors: latency (Ping/Pong) and
  throughput (UploadStart/drain/UploadDone). The responder is passive
  (echo pings, drain the upload, ack). The client offers a continuous ping
  loop that yields one RTT sample per round, plus a one-shot upload
  measurement.

### `iroh-doctor connect` as a live monitor

`connect` dials by endpoint id (keeping the existing `--relay`/`--addr`
hints) and renders a continuously-updating terminal view until Ctrl-C:

- connection state (relay / direct / custom) and TTFDB, sampled from
  `connection.paths()`;
- the path table (addr, selected, per-path RTT);
- latency over time from `core::probe`'s ping loop;
- throughput from `core::probe`'s upload measurement (the peer's passive
  responder drains it).

Terminal rendering (indicatif / console) stays in the CLI; `core` supplies
the data. This replaces the current `passive_side`-on-connect behavior. The
doctor echo/send/drain test remains reachable through `accept`/the existing
test path; it is just not what `connect` does now.

### Rewiring

- `cli`: delete its duplicate `NatType`, `classify`, `ExtendedNetworkReport`,
  `TestStreamRequest`, and the ALPN const; import them from `core`. Build
  the new `connect` monitor on `core::probe` plus `connection.paths()`.
- `app`: delete `nat.rs` and `peer_probe.rs` and the doctor wire enum;
  import from `core`. The accept loop registers `core::probe::ALPN` and
  dispatches to `core::probe::handle_connection`.

## Risks and open questions

- The app's own live latency comes from QUIC path RTT, not the probe ping
  loop. After this change the CLI's latency-over-time (probe pings) and the
  app's (path RTT) are both "latency over time" but from different sources.
  Making them numerically identical means switching the app's live latency
  onto the probe ping loop, a larger app-UI change deliberately left as a
  follow-up.
- NAT reconciliation: adopting the CLI's canonical `classify` must not
  change the app's results for its inputs. The merged tests are the guard;
  a difference is a parity bug to fix, not paper over.
- The probe client is new code (the app had removed its client earlier).
  It needs its own tests (RTT sample shape, upload throughput math) and a
  bounded, cancel-safe ping loop so Ctrl-C exits cleanly.

## Commit strategy

Each commit builds and tests on its own:

1. `feat: add the iroh-doctor-core crate` - nat + doctor wire + probe
   (responder, client, codec) with moved and merged tests; wired into the
   workspace, not yet consumed.
2. `refactor: use iroh-doctor-core in the app` - drop the app's duplicates,
   point at core, keep behavior identical.
3. `refactor: use iroh-doctor-core in the cli` - drop the CLI's duplicates,
   point at core, no behavior change yet.
4. `feat: make iroh-doctor connect a live connection monitor` - rebuild
   connect on core::probe plus connection path sampling.

No push or PR in this work; leave the branch committed and ready.
