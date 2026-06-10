# Plan: overnight 2026-06-09, all unblocked work

- [x] write plan
- [x] T0 toolchain: dx 0.7.9 + Android SDK 35 / build-tools 35 / NDK r28c installed and working
- [x] B5 untrack cli/log.txt (6856505)
- [x] B4 e2e verify the live monitor (cli accept <-> cli connect): direct path, live latency, throughput, ttfdb all rendered
- [x] B2 align the app's live latency onto the probe ping loop (8c6be8b)
- [x] B3 collect per-destination-port variation so NAT can return Easy (4950c36 + 2e78de4 validation fix)
- [x] B1 app CI job: macOS app job, clippy + tests (ca1b6a1)
- [x] A1 trust-model framing: in-app first-run copy (a90eb11)
- [x] A2 harden error + empty states for a stranger's first launch (a90eb11)
- [x] A3 background behavior: foreground-only decided + documented (dc89c2f)
- [x] A4 Android permissions: INTERNET only, multicast/wifi-state omitted with evidence (dc89c2f)
- [x] A5 iOS minimum deployment target 13.0 + build-number wiring (dc89c2f, 6dc4653)
- [x] A6 document the version/build-number strategy (dc89c2f)
- [x] A7 Android release artifacts: branded release APK + AAB, verified twice (6dc4653, 2a945e5)
- [x] A8 splash / launch screen: iOS storyboard + Android 12 splash theme (a1cf12e)
- [x] A9 Play feature graphic (1024x500) + favicon replaced with the mark (a1cf12e)
- [x] A10 App Privacy nutrition-label + Data Safety answers (fe3c77b)
- [x] A11 draft terms of use (fe3c77b)
- [~] stretch: Android emulator screenshots - DROPPED. Disk-bound on this
      Mac mini (SDK + target dir fill the disk); explicitly a stretch and
      not required by the closing checklist. See worklog.
- [x] final review: staff reviews round 1 (4 reviewers) + round 2 (2
      reviewers, safe to keep), review-of-reviews, closing checklist all
      in plans/worklog-2026-06-09.md

All items complete. Goal met. See plans/worklog-2026-06-09.md for the
closing checklist with evidence and the morning summary.

## Goal

Close every item from plans/app-store-release-design.md and
plans/pr-notes.md that does not require paid accounts, signing
identities, a physical device, or Xcode (absent on this machine). At
the end the branch should hold reviewable commits for all code and
config work, store-ready docs and assets, and a worklog Rae can read
first thing in the morning.

## Context

The repo is a three-crate workspace (cli, app, core) on rae/doctor-app,
clean, 8616 lines of Rust. Two prior autonomous sessions closed the
core extraction and the mobile branding wrapper. What remains splits
into the release plan's unblocked tail (Phases 1-4 oddments, Android
release artifacts) and the six pr-notes follow-ups. This machine is new
to mobile work: dx and the Android SDK pieces are installing now; iOS
native builds are impossible here (no Xcode).

## Approach

Order is chosen so pure-Rust work runs while toolchain downloads
finish, and so risky items (B2, B3, A4) get full cycles early in the
night.

1. **B5 untrack cli/log.txt** (one-liner, `git rm --cached`).
2. **B4 e2e monitor verify**: run `iroh-doctor accept` and
   `iroh-doctor connect <id>` as real local processes, capture output,
   record evidence in the worklog. No code expected; if it breaks, full
   debug cycle.
3. **B2 latency alignment**: research app/src/node/monitor.rs vs
   core/src/probe.rs ping loop; move the app's live latency source to
   the probe ping loop so cli and app read the same number. Risk:
   the app may rely on path-level RTT for the path table; keep that
   for paths, switch only the headline latency series.
4. **B3 NAT port variation**: core's classify_nat_type already takes
   port-variation input that nothing collects. Add collection (probe
   from extra sockets / vary destination ports per the STUN-style
   technique the report supports), feed it through cli + app.
   Riskiest item; full research phase before code.
5. **B1 app CI**: extend .github/workflows/ci.yaml (or a new
   app-ci.yaml) with a macOS job that installs dx and builds the app
   (desktop feature headless check at minimum; mobile as cache-heavy
   optional). Cannot run CI here; validate YAML + document.
6. **A-stream code**: A1 first-run trust copy, A2 error/empty states,
   A4 Android permissions + MulticastLock (needs research: dx 0.7.9
   manifest injection + acquiring the lock from Rust via jni at app
   start on Android).
7. **A-stream config/docs**: A3 foreground-only decision note, A5 iOS
   deployment target + CFBundleVersion wiring (config only, verify
   steps documented), A6 version mapping doc, A10 nutrition label +
   Data Safety answers, A11 terms draft.
8. **A-stream assets**: A8 splash, A9 feature graphic + favicon.
   Constraint: no rsvg/imagemagick here; check app/assets/icon/README
   for how the PNGs were rendered, else render SVG via a headless
   webview/Python/cargo resvg install (user-local, allowed).
9. **A7 Android release artifacts**: once T0 lands, run
   scripts/bundle-mobile.sh android --release with JAVA_HOME pointing
   at homebrew OpenJDK, then `dx bundle --platform android
   --package-types aab`; verify label/icon/targetSdk on the outputs.
10. **Stretch**: emulator screenshots only if everything above closes.

Worktree subagents run independent items in parallel (B2 vs B1 vs
docs/assets) per the parallel-subagent mandate; B3 stays with the
coordinator after research. Staff reviews at the B-stream boundary and
at end of night on the full diff, round 2 on the post-fix diff.

## Risks and open questions

- dx 0.7.9 on rustc 1.98 nightly: prior verified pipeline used 1.95;
  a build failure here is an environment regression to document, not
  necessarily our bug.
- NDK 28 vs the doc's NDK 30: dx may pin expectations; if the Android
  build fails on NDK, try the newest NDK sdkmanager offers.
- JDK 19 (homebrew) vs AGP for SDK 35: AGP 8.x wants JDK 17+; 19
  should pass, but JBR 21 was the verified combo.
- B3 design unknown: how much of the port-variation probe the current
  net-report data already exposes vs needing new probe traffic.
- MulticastLock needs Java-side or JNI work inside a dx-generated
  project that regenerates every build; may need the build wrapper to
  inject a MainActivity override or use dx's android_main_activity
  hook.
- Asset rendering tooling absent; may install resvg (user-local).

## Commit strategy

One commit per checklist item, conventional prefixes, each compiling
and passing the suite: `chore:` (B5), `fix:`/`feat:` (B2, B3, A2, A4),
`ci:` (B1), `feat:`/`docs:` per A item as code vs prose. Worklog and
plan updates ride along with related commits (plans/ is committed in
this repo). No pushes.
