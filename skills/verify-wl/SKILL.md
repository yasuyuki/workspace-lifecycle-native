---
name: verify-wl
description: Verify wl task lifecycle, slot-cache retention, or required-check gates through the real wl CLI using disposable repositories and retained evidence.
license: MIT
---

# Verify workspace-lifecycle-native (wl)

For topic and target required-check gates, failure, or retry, use the
[required-checks recipe](features/required-checks.md). It takes a built binary
and substitutes a loopback API for GitHub.

Build `wl` from the selected `workspace-lifecycle-native` checkout into an isolated
scratch directory and drive the compiled `wl` executable as a user/agent does:
`begin`, `hold`, `finish` refusal while held, `release-hold`, commit with
declared `slot_cache` and undeclared ignored files, `finish --retire`, polling
`status` and `status [TASK] --finish ID` until retirement, confirming the retired
run stays readable by `FinishId` after notice drain while `status <task>`,
mismatched `status <other> --finish ID`, and unknown `--finish` ID fail, slot
reuse on a second `begin`, explicit `retire`, and task-name reuse isolation on
re-beginning the retired name. Validation and push preflight use local synthetic
scripts; no GitHub API or WSL offload is invoked. See [the feature map](features/README.md)
for proven scope and `not-run` surfaces.

Requirements: Python 3.10+, Rust/Cargo with locked dependencies available (or
network for first Cargo build), and Git.

From the repository root:

```sh
python skills/verify-wl/scripts/verify.py --repo . --evidence-root /absolute/evidence/parent --agent "<agent and model>"
```

On Windows, use your Python 3 executable and keep the command on one line. Pass
`--doctor-only` to record checkout identity (`revision`, `dirty`, `inputs_sha256`,
tool versions) without building or driving `wl`. Pass `--help` for all options.

## Workflow

1. **Doctor.** Checks `cargo`, `git`, and `wl/Cargo.toml` (`name = "wl"`).
   Records Git `revision` and `dirty` state (`unknown` outside Git), SHA-256 of
   `Cargo.toml`, `Cargo.lock`, `wl/Cargo.toml`, every `wl/src/*.rs`, and
   `verify.py`, plus `git`, `cargo`, `rustc`, `python`, and OS platform. Missing
   prerequisites return `blocked` (exit 2).
2. **Launch.** Copies the checkout (excluding `.git` and `target`) into a new
   scratch folder, runs `cargo build --locked --offline -p wl` into a scratch
   target directory (falling back to `cargo build --locked -p wl` if crates are
   not cached offline), verifies the built `wl` executable exists, and checks
   `wl help` lists every subcommand.
3. **Drive.** Creates a disposable bare remote `origin.git` (branch `main`) and
   clone `product` with `.gitignore` and `.lifecycle.toml` (`slot_cache` plus a
   Python validation script), plus an isolated `WL_CONFIG` pointing to a
   synthetic `preflight.py`, isolated `HOME`/`USERPROFILE`/`LOCALAPPDATA`, and
   an empty `GIT_CONFIG_GLOBAL`. Runs the fixed steps in
   [task finish lifecycle](features/task-finish-lifecycle.md) and checks every
   literal expectation.
4. **Evidence.** Writes `result.json` before Drive and after every command and
   assertion. Copies `preflight.py`, `preflight-log.txt`, `validation-ran.txt`,
   `.git/wl/logs/alpha-1.log`, `final-status.json`, and
   `finish-status-after-retire.json` into the evidence directory outside scratch.
5. **Cleanup.** Waits for background slot preparation (`background: false`) so
   detached workers release handles, removes only this run's scratch directory
   (retrying read-only Git files), and verifies `cleanup.ok` and
   `evidence_retained`.

Exit codes: `0` = `pass`, `1` = `fail`, `2` = `blocked`, `3` = `not-run`. Before
reporting a pass, read `result.json` and check `status`, `assertions`,
`cleanup.ok`, `evidence_retained`, and `not_run`. Failed cleanup or missing
retained artifacts forces `status: "fail"` even when all feature assertions pass.

`python skills/verify-wl/scripts/test_verify.py` checks detector behavior on
disposable copies (pass on real checkout, fail on faulty copies skipping
`clear_ignored` or `write_retired_run`, `blocked` on invalid repo, `not-run` on
`--doctor-only`).
