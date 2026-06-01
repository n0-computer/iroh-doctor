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

1. App CI: a `dx`-based, per-platform job (the app is excluded from the
   current headless matrix).
2. iOS build + provisioning for the new `com.number0.iroh-doctor-app`
   bundle id, verified on-device.
3. Consider untracking `cli/log.txt` (a pre-existing 101 KB diagnostic
   dump the move carried along).
4. The cli `connect` monitor's latency-over-time uses the probe ping loop,
   while the app's live latency uses QUIC path RTT. Aligning the app onto
   the probe ping loop would make the two numerically identical.
5. The shared NAT classifier can return `Easy` once per-destination-port
   variation is actually collected; nothing collects it yet.
6. Verify the live monitor against a real peer (the app, or another cli
   `accept`): `iroh-doctor connect <peer>`. It is unit-tested (probe over
   an in-memory duplex) but not exercised end-to-end here.
