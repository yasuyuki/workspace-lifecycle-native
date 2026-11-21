# Task finish lifecycle

A user or agent begins a task with `wl begin`, holds and unholds it (`wl hold`,
`wl release-hold`), commits work in the allocated slot alongside declared
`slot_cache` and undeclared ignored files, runs `wl finish --retire`, polls `wl
status` and `wl status --finish <id>` until the detached worker validates,
pushes, integrates into `main`, and retires the slot, verifies `status --finish <id>`
keeps reporting `stage: "retired"` after notice drain while `status <task>`,
mismatched `status <other> --finish <id>`, and missing `--finish` IDs return errors,
begins a second task that reuses the same slot (retaining `slot_cache`) and
explicitly retires it with `wl retire`, then re-begins and retires the first task name
to confirm prior run isolation.

Contract sources:

- `README.md` (`%LOCALAPPDATA%\wl\config.json` preflight contract)
- `CONTEXT.md` (Finish acceptance, Worker, Integration worktree, Needs-action, Slot, Slot cache)
- `docs/design.md` (`WL_CONFIG`, command behavior, finish stages, per-run finish status, slot reuse and `clear_ignored`)
- `wl/src/commands.rs` (`begin`, `status`, `hold`, `release_hold`, `retire`, `retire_task`, `clear_ignored`, `prepare_slot`, `lock_task`, `claim_slot`, `switch_slot`, `reclaim_orphan_claims`)
- `wl/src/finish.rs` (`finish`, `create_run`, `write_retired_run`, `clear_finish_files`, `finalize_retired_finish_locked`, `finish_report`, `finish_status`, `work`, `housekeeping`, `running`, `Worker::step`, `push_topic`, `wait_for_turn`, `leave_queue`, `queue_entry_task`, `needs_queue`, `lock_integration`, `prepare`, `push_target`, `notify`)
- `wl/src/main.rs`, `wl/src/config.rs`, `wl/src/model.rs`, `wl/src/store.rs`
- `wl/tests/cli.rs` and `wl/tests/finish.rs`

## Prerequisites

Python 3.10+, Rust/Cargo (locked dependencies in offline Cargo cache or network
available to fetch them), and Git. Everything else is built inside the helper's
scratch directory: copied source, built `wl` binary, bare `origin.git` on
`main`, clone `product`, `.lifecycle.toml`, synthetic `preflight.py`, and
isolated `WL_CONFIG` / `HOME` / `GIT_CONFIG_GLOBAL`.

## Operations and fixed expectations

| Step | Command and inputs | Expected observation |
| --- | --- | --- |
| Build | `cargo build --locked -p wl` on copied checkout | Built `wl` binary exists; `wl help` exits 0 and prints `begin`, `status`, `hold`, `release-hold`, `finish`, `retire`. |
| Begin | `wl --repo product begin alpha --request verify/lifecycle` | Exit 0, `ok: true`, `stage: "ready"`, `branch: "alpha"`, `worktree` under `product.slots`, `product` remains on `main`. |
| Hold & blocked finish | `wl --repo product hold alpha --reason "verify hold"`, then `wl --repo product finish alpha` | `hold` exits 0; `status alpha` reports `hold: "verify hold"`; `finish` exits 2 with `ok: false` and `"held"` in `error`; `origin/main` unchanged. |
| Release hold | `wl --repo product release-hold alpha --decision verify/release` | Exit 0, `released: "verify hold"`; `status alpha` reports `hold: null`. |
| Finish with `--retire` | Commit `work.txt` in `alpha` slot, create ignored `cache_keep/keep.txt` (`slot_cache`) and `scratch_drop/drop.txt`, run `wl --repo <slot> finish alpha --retire` | Exit 0, `ok: true`, `accepted: true`, non-empty `id`, `retire: true`, `attempt: 1`, `commit == HEAD`, `target: "main"`. |
| Background completion & retirement | Poll `wl --repo product status` and `wl --repo product status --finish <id>` until `alpha` leaves `tasks` and `retired` notice is received | `status --finish <id>` tracks `alpha` cleanly; one `retired` notice with `id == <id>` is delivered once (next `status` has no `notices`); `origin/main == HEAD` and `origin/alpha == HEAD`; validation script ran in `integrate-main` on `HEAD`; preflight ran for topic and target; local branch `alpha` deleted; `cache_keep/keep.txt` exists; `scratch_drop` removed; `slots.free >= 1`. |
| Post-retirement run status & guards | Run `wl --repo product status alpha`, `status --finish <id>`, `status alpha --finish <id>`, `status beta --finish <id>`, `status --finish missing-id` | `status alpha` exits 2 (`ok: false`); `status --finish <id>` and `status alpha --finish <id>` exit 0 with `ok: true`, `finish.id == <id>`, `task: "alpha"`, `attempt: 1`, `commit == HEAD`, `merged == HEAD`, `target: "main"`, `retire: true`, `stage: "retired"`, `detail: null`; `status beta --finish <id>` and `status --finish missing-id` exit 2 (`ok: false`). |
| Slot reuse & task-name reuse isolation | `wl --repo product begin beta --request verify/reuse`, `wl --repo product retire beta`, then `begin`/`status`/`retire alpha`, then re-read `status --finish <id>` | `begin beta` reuses `alpha`'s slot `worktree` path (`stage: "ready"`) with `cache_keep/keep.txt` intact; `retire beta` exits 0 with `retired: true`; fresh `begin alpha` starts with `finish: null`; `retire alpha` succeeds; original `status --finish <id>` still reports `stage: "retired"` on `alpha`'s integrated `HEAD`. |

## Substituted boundaries and limits

- Push preflight is a synthetic Python script logging calls and returning `push`,
  not the owner's private `workspace_lifecycle.push` policy check.
- Remote is a local bare repository (`origin.git`), not GitHub over HTTPS/SSH.
- `.github/required-checks.json` is absent (`ungated` path); required-check
  polling and GitHub API token lookup are not exercised.
- Validation uses a local Python command with default `where = "windows"` (runs
  locally); WSL offload (`where = "any"`) is not exercised.
- Only the host OS executing `scripts/verify.py` is proven; other features in
  [the feature map](README.md) remain `not-run`.
