This converts the repo into a Cargo workspace and brings the Dioxus GUI
(the former iroh-pong app) in as a second member, so the CLI and the app
stop drifting out of hand-maintained parity. The CLI moves to `cli/`
unchanged; the app lands at `app/` as `iroh-doctor-app`. App-first: each
crate keeps its own copy of the probe and doctor protocols for now, and a
shared `core` crate is the next PR.

The app's config directory moves from `iroh-pong` to `iroh-doctor-app`;
the secret key and saved endpoints fall back to and migrate from the old
directory, so a user's endpoint id and saved peers survive the rename.

The GUI app is excluded from the workspace-wide CI jobs and the release
build is scoped to the CLI: the app's dioxus renderer features are
mutually exclusive and it needs platform GUI libraries the CI runners
lack. Standing up app CI (a macOS runner with `dx`, or system GUI deps) is
a follow-up.

Wire ALPNs (`iroh-helloiroh-pong/0`, `iroh-pong-probe/0`) are kept literal
for interop; only crate, identity, path, and log names changed. The config
directory migration in `app/src/identity.rs` and `app/src/endpoints.rs` is
the one behavior change worth a close read.
