# PR notes: doctor + app unification

## Branch contents (two stacked pieces)

`rae/doctor-app` now holds two logical pieces, both committed locally and
NOT pushed:

1. The unification (commits f46df15, f074ab7): workspace + the app moved in
   as `iroh-doctor-app`. Described below.
2. The shared `core` crate + live `iroh-doctor connect` (commits 5e2c100
   through 1c41ac9): the genuinely shared code (NAT classifier, doctor wire
   types, peer probe protocol) extracted into `iroh-doctor-core`, both
   crates rewired onto it, and `connect` rebuilt as a live monitor. See
   plans/core-extraction-design.md and the worklog.

You can land this as one PR (all 8 commits against
`rae/feat-iroh-rc.0-and-probe`) or split piece 2 onto its own branch off
piece 1 for two reviews. The commits are ordered and each builds + tests on
its own, so either works.

To publish and open a single PR against `rae/feat-iroh-rc.0-and-probe`:

```sh
cd ~/dev/iroh-doctor
git push -u origin rae/doctor-app
gh pr create --base rae/feat-iroh-rc.0-and-probe --head rae/doctor-app \
  --title "Unify the doctor CLI and the Dioxus app into one workspace" \
  --body-file plans/pr-body.md
```

## Proposed PR title

Unify the doctor CLI and the Dioxus app into one workspace

## Proposed PR description

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

## Reviewer call-outs

- Wire ALPNs (`iroh-helloiroh-pong/0`, `iroh-pong-probe/0`) are kept
  literal for interop; only crate/identity/path/log names were renamed.
- The config-dir migration is the one behavior change worth a close read:
  `app/src/identity.rs` and `app/src/endpoints.rs`.

## Known follow-ups

All but one closed in the 2026-06-09 overnight session (see
plans/worklog-2026-06-09.md):

1. ~~App CI~~ - done: a macOS `app` job (clippy + tests, default desktop
   feature) in ci.yaml. Per-platform dx bundles remain future work.
2. iOS build + provisioning for the `com.number0.irohdoctor` bundle id,
   verified on-device. *(Still open: needs the iPhone, a signing
   identity, and a machine with Xcode.)*
3. ~~Untrack `cli/log.txt`~~ - done, plus a `.gitignore` entry.
4. ~~Align the app's live latency onto the probe ping loop~~ - done: a
   dial plots probe ping round-trips (matching `connect`), an incoming
   probe keeps path RTT (matching `accept`).
5. ~~NAT classifier `Easy`~~ - done: `core::port_variation` (QAD helper
   server + same-socket probe), `iroh-doctor nat-helper`, and
   `diagnostics --nat-probe host:p1,host:p2`. The app does not collect
   it yet; helper-address UX is a product decision.
6. ~~Verify the live monitor end to end~~ - done against a real peer on
   the real network: direct path selected, live latency series,
   throughput, ttfdb all rendered (evidence in the worklog).
