# DRAFT — iroh doctor privacy policy

> Draft for legal review, **not yet published**. Intended home:
> `https://www.iroh.computer/legal`. Written from the app's actual behavior as of
> 2026-06-01 (commit on `rae/doctor-app`). Verify every claim against the shipped
> build before publishing, and have counsel confirm wording for App Store /
> Google Play. Replace bracketed placeholders.

---

**iroh doctor — Privacy Policy**

_Last updated: [DATE]_

iroh doctor is a peer-to-peer network-diagnostics tool published by number 0,
Inc. ("number 0", "we", "us"). It helps you measure the quality of a direct
connection to another peer and inspect your local network environment. This
policy explains what the app does and does not do with data.

### Summary

- We do **not** require an account, and we do **not** collect personal
  information for advertising or analytics.
- By default, the app sends **no diagnostic or usage data** to us.
- The app is a networking tool: to connect you to a peer it uses the same public
  iroh infrastructure (relay and discovery servers) any iroh application uses.
  Establishing connections necessarily exposes network information such as IP
  addresses and public node identifiers to that infrastructure and to the peer
  you connect to.

### Information stored on your device

- **Node identity.** On first launch the app generates a cryptographic key pair
  and stores the secret key in the app's private storage on your device. It is
  used to identify your node to peers. It never leaves the device. Deleting the
  app removes it.
- **Saved endpoints.** Peer identifiers you choose to save are stored locally.
- **Logs.** The app writes diagnostic logs to its private storage on your device
  (and, on iOS, to the system log) to help with troubleshooting. These stay on
  the device unless you choose to export and share them.
- **Diagnostics bundle.** When you export a diagnostics bundle, it is created on
  your device and saved to a location you choose (or to the app's private
  storage on mobile). It is shared only if you share it.

### Information shared over the network

To connect you to another peer, iroh doctor uses public iroh infrastructure
operated by number 0:

- **Relay and discovery servers.** The app uses n0's relay servers and DNS-based
  discovery to help establish peer-to-peer connections. As a normal part of
  this, your public node identifier and network addresses (including IP
  addresses) may be visible to that infrastructure and to the peer you connect
  to. This is inherent to how the tool works.
- **Peers.** When you connect to a peer, you and that peer exchange connection
  measurements (such as round-trip latency and throughput) and any messages you
  send over the gossip feature. Only connect to peers you trust.

We do not use this connection information to build advertising or marketing
profiles.

### Optional telemetry (off by default)

The app can integrate with iroh-services, a service operated by number 0.
**This is disabled by default.** It is enabled only if you supply an
iroh-services API key and turn it on. When enabled, diagnostic and operational
telemetry is sent to iroh-services to help analyze connectivity. You can leave
it off, and you can stop it at any time by removing the key. [Confirm exact
telemetry fields and link to the iroh-services privacy terms before publishing.]

### Third parties

- **number 0 infrastructure** (relay, discovery, and — only if you enable it —
  iroh-services), as described above.
- We do **not** use third-party advertising or analytics SDKs.

### Children

iroh doctor is a developer/network tool and is not directed to children under
13. We do not knowingly collect personal information from children.

### Data retention and deletion

Data stored on your device remains until you delete it or uninstall the app.
We do not maintain user accounts, so there is no server-side profile to delete.
[State retention for any telemetry collected via iroh-services when enabled.]

### Changes

We may update this policy. Material changes will be posted at this URL with an
updated "Last updated" date.

### Contact

[support email / URL — e.g. the iroh.computer contact or a dedicated address].
