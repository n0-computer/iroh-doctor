# iroh-doctor

Tools for diagnosing and testing iroh in your network configuration. This
repository is a Cargo workspace with two crates:

- [`cli/`](cli/) - `iroh-doctor`, the command-line tool. See
  [cli/README.md](cli/README.md) for usage.
- [`app/`](app/) - `iroh-doctor-app`, a cross-platform GUI built with
  Dioxus that surfaces the same diagnostics and runs the probe and doctor
  protocols against a peer.

The CLI and the app currently keep their own copies of the shared probe
and doctor protocols. Extracting that shared code into a `core` crate is
planned as a follow-up.
