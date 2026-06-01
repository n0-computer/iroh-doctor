# Plan: unify the doctor CLI and the Dioxus app into one workspace

- [x] write plan
- [x] commit 1: convert repo to a Cargo workspace, move the CLI into `cli/` (f46df15)
- [x] commit 2: add the Dioxus app at `app/` as `iroh-doctor-app`, with rename + config-dir migration (f074ab7)
- [x] verify both crates build and the workspace tests pass (110 passed, 0 failed, 0 ignored)
- [x] self-review (re-read every changed file, opposing stance)
- [x] staff review on the full diff, then review round 2 on the post-fix diff
- [x] final review

Done. Not pushed; see plans/pr-notes.md. Follow-ups (core crate, app CI,
iOS provisioning) tracked in the worklog and pr-notes.

## Goal

Bring the Dioxus app (currently the `iroh-pong` / `iroh-dr-app` repo) into
the `iroh-doctor` repo as a second workspace member so the GUI and the CLI
live together and the manual "parity with iroh-pong" syncing ends. This PR
does the move app-first: each crate keeps its own copy of the shared
protocol code, and extracting a shared `core` crate is a follow-up PR.

## Context

`iroh-doctor` is a single-package crate at the repo root (branch
`rae/feat-iroh-rc.0-and-probe`, on iroh `1.0.0-rc.1`). It already grew
`probe`, `report`, and `port_map` commands kept in hand-maintained parity
with the app. The app is a Dioxus crate named `iroh-pong` with its own
`peer_probe.rs` (probe ALPN + protocol), a `doctor.rs` responder for the
doctor test protocol, and a diagnostics UI that mirrors the CLI's probes.

Unifying the repos removes the parity burden. We start by relocating both
crates into a workspace without trying to deduplicate yet.

## Approach

### Commit 1 (refactor, no behavior change): workspace + `cli/`

Move the package down into `cli/` and make the root a workspace manifest.

- `git mv` the package-specific entries into `cli/`: `Cargo.toml`, `src/`,
  `README.md`, `log.txt`.
- Keep at the repo root: `Cargo.lock` (workspace lock), `.github/`,
  `.config/` (nextest), `deny.toml`, `cliff.toml`, `release.toml`,
  `Makefile.toml`, `.typos.toml`, `.gitattributes`, `.gitignore`, licenses,
  `code_of_conduct.md`, `CONTRIBUTING.md`.
- Write a new root `Cargo.toml` with `[workspace] members = ["cli"]`,
  `resolver = "2"`. Add `"app"` to members in commit 2, not here, so this
  commit builds on its own.
- Add a short root `README.md` describing the workspace (cli + app) and
  pointing at each crate.
- `Makefile.toml` needs no change: its `format`/`format-check` tasks use
  `cargo fmt --all`, which already covers all members.
- Verify: `cargo check --workspace --all-features`, `cargo build -p
  iroh-doctor`, and the CI test invocation pass exactly as before.

### Commit 2 (additive): the app at `app/`

Plain-copy the app from the `iroh-pong` repo's `rae/doctor-app` branch into
`app/`, excluding `target/`, `.git/`, `Cargo.lock`, `plans/`, `AGENTS.md`,
`*.log`, `.DS_Store`, `.claude/`. Copy `src/`, `assets/`, `Cargo.toml`,
`Dioxus.toml`, `.cargo/config.toml`, `clippy.toml`, `README.md`,
`.gitignore`.

Apply the rename (`iroh-pong` -> `iroh-doctor-app`,
`com.number0.iroh-pong` -> `com.number0.iroh-doctor-app`):

- `app/Cargo.toml`: `[package].name = "iroh-doctor-app"`.
- `Dioxus.toml`: bundle `identifier` and web `title`.
- `src/main.rs`: env-filter directive `iroh_pong=debug` ->
  `iroh_doctor_app=debug`; os_log subsystem string; the comment referencing
  the subsystem; the rolling log file name; the save-dialog title; the
  generated zip filename.
- `src/diagnostics_export.rs`: the bundle header string.
- `README.md`: title, config-path example, env example.
- Leave the wire ALPNs (`wire.rs`, `peer_probe.rs`, `doctor.rs`) unchanged:
  they are on-the-wire protocol identifiers.
- Add `"app"` to the root workspace `members`.
- Hoist the iOS `.cargo/config.toml` (`IPHONEOS_DEPLOYMENT_TARGET`) to the
  workspace root so the setting applies to the app target. Confirm it does
  not disturb the CLI build.

Config-dir migration (so the user's identity and saved endpoints survive
the `iroh-pong` -> `iroh-doctor-app` directory rename):

- `identity.rs`: `config_dir()` -> `iroh-doctor-app`. Add a legacy read of
  the old `iroh-pong` dir for `secret_key.bin` and `api_secret.txt`; on a
  legacy hit, migrate by writing to the new path so the endpoint id is
  preserved.
- `endpoints.rs`: primary `endpoints_path()` -> `iroh-doctor-app/
  endpoints.json`. The legacy fallback list keeps pointing at the OLD
  `iroh-pong` dir: `iroh-pong/endpoints.json` then `iroh-pong/devices.json`.
  (The research report suggested renaming the legacy path to
  `iroh-doctor-app`; that is wrong, it would drop the old data.)

Verify: `cargo check -p iroh-doctor-app`, `cargo build -p iroh-doctor-app`
(desktop), `cargo check --workspace`, and `cargo test --workspace`.

## Risks and open questions

- iOS build: verifying the `dx`/iOS build headless is likely not possible
  here (needs the dx CLI, a simulator or device, and signing for the new
  bundle id). Plan: verify the desktop build and `cargo check` for the app
  crate, and document the iOS build + new-bundle-id provisioning as a step
  the user runs on-device. This is a real external dependency, not a skip.
- Dependency resolution: cli and app pin overlapping iroh-family crates
  (`iroh =1.0.0-rc.1`, `iroh-services`, `iroh-relay`, `iroh-base`). They do
  not share code yet, so the single workspace `Cargo.lock` can carry
  whatever versions each needs. Watch for a `=`-pin conflict during the
  first workspace resolve.
- `resolver = "2"` changes feature unification across the workspace. With
  only two unrelated binaries this should be inert, but re-run the cli
  tests after adding the workspace to confirm no feature drift.
- `log.txt` is a 101 KB tracked diagnostic dump. Moving it with `git mv`
  into `cli/` is non-destructive and preserves history; removing it from
  tracking is a separate decision left to the user.

## Commit strategy

Two commits, each independently building and testing:

1. `refactor: convert to a cargo workspace with the cli under cli/`
2. `feat: add the dioxus app as the iroh-doctor-app workspace member`

No pushing and no PR creation in this session (overnight safety). Leave the
branch `rae/doctor-app` committed and ready for the user to push.
