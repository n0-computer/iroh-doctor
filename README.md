# iroh-doctor

Tools for diagnosing and testing iroh in your network configuration. This
repository is a Cargo workspace with three crates:

- [`cli/`](cli/) - `iroh-doctor`, the command-line tool. See
  [cli/README.md](cli/README.md) for usage.
- [`app/`](app/) - `iroh-doctor-app`, a cross-platform GUI built with
  Dioxus that surfaces the same diagnostics and runs the probe protocol
  against a peer.
- [`core/`](core/) - `iroh-doctor-core`, the shared probe protocol, monitor
  helpers, and NAT classification used by both the CLI and the app.
