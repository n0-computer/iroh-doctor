# DRAFT — store listing copy (App Store + Google Play)

> Draft for review. Positioning (agreed): **public, but a developer /
> network-diagnostics utility** — not a consumer app. Copy is written from the
> app's real features (see `app/README.md`). Tune voice with the n0 brand team;
> verify character limits against current store rules before submitting.

## Shared

- **App name (display)**: iroh doctor
- **Developer / seller**: Number Zero
- **Category**: Developer Tools (App Store) / Tools (Google Play)
- **Primary audience**: developers and operators working with peer-to-peer /
  iroh networking.
- **Support URL**: https://www.iroh.computer
- **Privacy policy URL**: https://www.iroh.computer/legal
- **Price**: Free

## Apple App Store

**Subtitle** (≤30 chars):
`Peer-to-peer network doctor`

**Promotional text** (≤170 chars, updatable without review):
`Diagnose direct connections and your local network: live latency, paths,
throughput, NAT type, relay timing, and an iroh-gossip playground.`

**Keywords** (≤100 chars, comma-separated, no spaces):
`iroh,p2p,network,diagnostics,latency,NAT,relay,QUIC,connection,gossip,debug,devtool`

**Description**:
```
iroh doctor is a peer-to-peer network diagnostics tool for developers. Connect
to another peer by node id and watch the connection in real time, or inspect
your local network environment on its own.

CONNECT
• Live round-trip latency with a 30-second sparkline
• The set of QUIC paths in use (direct IP, relay, or custom) with per-path RTT
• Time-to-first-direct-byte and periodic throughput
• A connection-event log of recent state transitions

DIAGNOSTICS
• Local network report with a NAT classification (Easy / Medium / Hard)
• IPv4 and IPv6 visibility, mapping variation, and captive-portal status
• Direct port-mapping probe (UPnP / PCP / NAT-PMP)
• Per-relay latency: TLS connect time plus a relay protocol ping
• Side-by-side iroh-services view to spot disagreements

GOSSIP
• Join an iroh-gossip topic, see neighbors, and broadcast messages

iroh doctor talks to other peers using the same public iroh infrastructure any
iroh app uses. It does not require an account, and by default it sends no data
to us. It is a hands-on debugging tool: connect only to peers you trust.
```

**What's New** (first release):
`First release of iroh doctor: live connection diagnostics, a local network
report, and an iroh-gossip playground.`

**Export compliance**: uses only standard encryption (TLS/QUIC) → exempt
(`ITSAppUsesNonExemptEncryption = false`, already set).

## Google Play

**Short description** (≤80 chars):
`Peer-to-peer network diagnostics: latency, paths, NAT type, relays, gossip.`

**Full description** (≤4000 chars): reuse the App Store description above.

**Content rating**: complete the IARC questionnaire — expected "Everyone"
(a utility with user-to-user messaging via gossip; disclose the messaging).

**Data safety** (matches the privacy policy):
- No data collected by default.
- Optional, off-by-default telemetry to Iroh Services if the user supplies a key.
- Connection metadata (IP, node id) transits public iroh relay/discovery as an
  inherent part of establishing peer connections.
- Data is not sold; no third-party ads/analytics SDKs.

## Open items before submission
- [ ] n0-brand voice pass on the description + subtitle.
- [ ] Confirm the messaging (gossip) disclosure wording for both content ratings.
- [ ] Screenshots (Phase 2) — captions can reuse the CONNECT/DIAGNOSTICS/GOSSIP
      section headers.
- [ ] Decide whether to mention "iroh" trademark usage per n0 guidelines.
