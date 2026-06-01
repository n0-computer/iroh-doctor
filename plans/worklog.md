# Worklog: shared core crate + live `iroh-doctor connect`

Started: 2026-05-27T15:36-07:00
Wrapped: 2026-05-27T16:10-07:00
Branch: rae/doctor-app (stacked on the earlier unification commits)
Mode: overnight. No push, no PR, no destructive ops. Left ready to push.

## Outcome: goal met

Per plans/core-extraction-design.md (approved). Six commits on top of the
unification work, each building and tested on its own:

- 5e2c100  feat: add the iroh-doctor-core crate
- 925a3bb  refactor: use iroh-doctor-core in the app
- 8010dac  refactor: use iroh-doctor-core in the cli
- 5e4958b  feat: make iroh-doctor connect a live connection monitor
- 5fbd381  fix: bound the probe upload drain and cover continuous serving
- 1c41ac9  fix: correct accept's connect instructions and a stale comment

The new `core` crate (`iroh-doctor-core`) holds the code the cli and app
genuinely share; both now depend on it. `iroh-doctor connect` is a live
monitor matching the app's connect view.

## What landed

- core/: `nat` (NatType + ExtendedNetworkReport + classify_nat_type +
  classify_base_report), `doctor` (TestStreamRequest + n0/doctor/1 ALPN
  wire types), `probe` (Frame + iroh-pong-probe/0 ALPN + generic codec +
  the passive responder + a ProbeClient that pings and uploads +
  throughput_mbps).
- The probe responder was reworked from one-shot into continuous: it
  echoes pings uncapped and keeps serving after an upload, bounded by a
  per-frame idle timeout (incl. on the upload drain) and the upload-size
  cap.
- app/: dropped nat.rs and peer_probe.rs and the doctor wire enum; uses
  core. Behavior unchanged (verified: classify_base_report == old classify
  on every input).
- cli/: dropped nat_classifier.rs, swarm/net_report_ext.rs, and the doctor
  wire enum/ALPN; uses core. `connect` defaults to a live monitor over the
  probe protocol (state + paths + latency over time + throughput + TTFDB);
  `connect --test` keeps the old doctor passive test; `accept` now serves
  both ALPNs so it can be monitored.

## Verification (evidence, final tree)

- `cargo test --workspace`: 110 passed, 0 failed, 0 ignored (11 cli + 80
  app + 19 core).
- `cargo clippy --workspace --exclude iroh-doctor-app --all-targets
  --all-features`: clean. `cargo clippy -p iroh-doctor-app --all-targets`:
  clean. `cargo fmt --all --check`: clean.

## Review

Round 1: two staff reviewers (core correctness; cli monitor + rewires).
Round 2: one reviewer on the post-fix diff.

Applied:
- Bounded the probe upload drain with the idle timeout (the continuous
  rework had dropped the overall handler ceiling, leaving a stall path
  that could pin the cli responder, which has no semaphore).
- Added a test proving the responder serves past the old ping cap and
  after an upload.
- Fixed `accept`'s printed pairing commands to `connect --test` and
  clarified the `connect` help, since the default flipped to the monitor.

Held after opposing-stance review:
- NAT reconciliation: confirmed `classify_nat_type` is identical to the
  cli original for all inputs and to the app's as-called; the only change
  is latent (the app could return Easy once it collects port variation),
  and that is already documented in the module doc. No code change.
- TTFDB watcher task lingers until the connection drops: benign (the
  stream ends when the connection closes). MSRV of `is_multiple_of` (1.87)
  is under the CI MSRV (1.89). No action.

## Closing checklist

- [x] Wall-clock: ~34 minutes; under 6 hours, but the stated goal is fully
  met (the "whichever is sooner" clause). Push and PR are intentionally
  left to the user (overnight safety).
- [x] Every test claimed passing was verified on a fresh full
  `cargo test --workspace` just now: 110 passed.
- [x] No `todo!()` in scope. The only TODO is the deliberate wire-version
  note in probe.rs.
- [x] Every staff-review finding applied or argued (see Review above).
- [x] Staff review round 2 ran on the post-fix diff.
- [x] No `#[ignore]`/skipped tests: the full run reports 0 ignored.
- [x] No forbidden phrase without action: remaining "follow-up" items
  (aligning the app's live-latency source onto the probe ping loop;
  collecting per-port variation to let NAT return Easy) are genuine future
  scope, recorded in pr-notes, not deferred work that could be done now.

## Branch note

rae/doctor-app now contains both the earlier unification (2 commits:
f46df15, f074ab7) and this core extraction + monitor (6 commits, 5e2c100
through 1c41ac9). See plans/pr-notes.md for how to push and whether to
split into two PRs.
