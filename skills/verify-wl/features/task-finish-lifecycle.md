# Task finish lifecycle

A user or agent begins a task with `wl begin`, holds and unholds it (`wl hold`,
`wl release-hold`), commits work in the allocated slot alongside declared
`slot_cache` and undeclared ignored files, runs `wl finish --retire`, polls `wl
status` until the detached worker validates, pushes, integrates into `main`, and
retires the slot, then begins a second task that reuses the same slot and
explicitly retires it with `wl retire`.

Contract sources:

- `README.md` (`WL_CONFIG` / `config.json` preflight contract)
- `CONTEXT.md` (Finish acceptance, Worker, Integration worktree, Needs-action, Slot, Slot cache)
- `docs/design.md` (command behavior, finish stages, slot reuse and `clear_ignored`)
- `wl/src/commands.rs` (`begin`, `hold`, `release_hold`, `retire`, `status`, `prepare_slot`)
- `wl/src/finish.rs` (`finish`, `work`, `Worker::step`, `prepare`, `push_target`, `notify`)
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
| Build | `cargo build --locked -p wl` on copied checkout | Built `wl` binary exists; `wl help` exits 2 or 0 and prints `begin`, `status`, `hold`, `release-hold`, `finish`, `retire`. |
| Begin | `wl --repo product begin alpha --request verify/lifecycle` | Exit 0, `ok: true`, `stage: "ready"`, `branch: "alpha"`, `worktree` under `product.slots`, `product` remains on `main`. |
| Hold & blocked finish | `wl --repo product hold alpha --reason "verify hold"`, then `wl --repo product finish alpha` | `hold` exits 0; `status alpha` reports `hold: "verify hold"`; `finish` exits 2 with `ok: false` and `"held"` in `error`; `origin/main` unchanged. |
| Release hold | `wl --repo product release-hold alpha --decision verify/release` | Exit 0, `released: "verify hold"`; `status alpha` reports `hold: null`. |
| Finish with `--retire` | Commit `work.txt` in `alpha` slot, create ignored `cache_keep/keep.txt` (`slot_cache`) and `scratch_drop/drop.txt`, run `wl --repo <slot> finish alpha --retire` | Exit 0, `ok: true`, `accepted: true`, `attempt: 1`, `commit == HEAD`, `target: "main"`. |
| Background completion & retirement | Poll `wl --repo product status` until `alpha` leaves `tasks` and `retired` notice is received | One `retired` notice is delivered once (next `status` has no `notices`); `origin/main == HEAD` and `origin/alpha == HEAD`; validation script ran in `integrate-main` on `HEAD`; preflight ran for topic and target; local branch `alpha` deleted; `cache_keep/keep.txt` exists; `scratch_drop` removed; `slots.free >= 1`. |
| Slot reuse & explicit retire | `wl --repo product begin beta --request verify/reuse`, then `wl --repo product retire beta` | `begin beta` reuses `alpha`'s slot `worktree` path (`stage: "ready"`); `retire beta` exits 0 with `retired: true`. |

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
