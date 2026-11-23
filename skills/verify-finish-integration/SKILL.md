---
name: verify-finish-integration
description: Model-check that `wl finish` and its detached worker push only validated and CI-checked commits to the integration target, enforce holds and retirement safety, and preserve exclusive access to the shared integration worktree and exact queue ownership. Use when a change touches `wl/src/finish.rs`, `wl/src/store.rs` (`integration_lock`), or `wl/src/commands.rs` (`retire_task`).
license: MIT
---

# Verify finish and target integration (`wl`)

**Rules.**

- `ExclusiveIntegrationWorktree`: at most one task worker reads or mutates the shared integration worktree `integrate-<target>` at any moment.
- `OnlyCheckedWorkReachesTarget`: every task commit pushed to `origin/<target>` passed `.lifecycle.toml` validation on its own uncorrupted merged tree and passed topic required checks when `.github/required-checks.json` is non-empty.
- `IntegratedIsOnRemote`: a task recorded as `integrated` (or `retired` after integration) has its accepted commit on `origin/<target>`.
- `IntegratedChecksPassed`: when `.github/required-checks.json` is non-empty, a task recorded as `integrated` passed required checks on the exact commit pushed to `origin/<target>`.
- `NoUnpublishedOrDirtyRetire`: `finish --retire` never retires a task that has uncommitted/untracked files, an active `HoldRecord`, post-accept commits, or commits missing from `origin/<target>`.
- `NoTargetPushWhileHeld`: no task pushes to `origin/<target>` while it has an active `HoldRecord`.
- `QueueEntryMatchesOwner`: a task worker's `leave_queue()` never removes another running task's entry from `queue/<target>/`.
- `RetiredRunMatchesExecution`: once `finish --retire` completes retirement (even after crashing post-task-deletion once `housekeeping()` / `begin()` / `retire()` runs `finalize_retired_finish_locked()`), its durable `runs/<id>.json` records `stage: "retired"` matching that exact run's commit and target, and no live `accept.json` / `progress.json` remains to taint later runs.
- `RunIsolationAcrossRebegin`: retiring `task` and re-beginning the same task name preserves every prior terminal `runs/<id>.json` (`retired` / `needs-action`) keyed by `FinishId` without altering or misattributing earlier executions.

**Harm when they break.** Two workers overwrite each other's `integrate-<target>` checkout mid-validation so unchecked code lands on `main`, a running task's queue entry is deleted early, unverified commits reach `main`, held tasks integrate, dirty/unpushed user work is erased during retirement, callers monitoring `finish --retire` lose its outcome or mistake an unrelated or failed execution for a retired run, or leftover finish files corrupt a later same-named task.

## Current status at `3a46d797c79915a6b13d0d09df0cc1b357e0b2d7`

All six configurations (`single-task-gated`, `single-task-ungated`, `single-task-retire-reuse`, `two-tasks-crash-queue`, `two-tasks-accept-race`, `two-tasks-suffix`) pass all nine invariants (`EXPECTED` is `None` for every config). In addition to the earlier two-task findings (`49d2d43`), commit `0149b85` preserves per-run `runs/<id>.json` outcomes keyed by `FinishId` (`wl/src/finish.rs:281-310`), writes `Stage::Retired` via `write_retired_run()` after `retire_task()` deletes `tasks/<task>.json` (`wl/src/commands.rs:356-359`), and finalizes crashed `finish --retire` executions via `finalize_retired_finish_locked()` (`wl/src/finish.rs:155-171`) in `housekeeping()`, `begin()`, and absent-task `retire_task()`.

## When to use

Run this check when a change touches covered code in `wl/src/finish.rs`, `wl/src/store.rs`, `wl/src/model.rs` (`FinishId` / `FinishRun`), or `wl/src/commands.rs`. To see what changed since the model was last confirmed:

```sh
sync=$(git log -1 --format=%H -- skills/verify-finish-integration)
git diff "$sync" -- wl/src/finish.rs wl/src/store.rs wl/src/model.rs wl/src/commands.rs
```

Empty output (or changes only inside `hold`, `release_hold`, `prepare_slot`, or `reclaim_*` outside `begin`, `retire_task`, `Store::*lock*`, and `FinishRun` / `runs/`) means the model still describes the covered functions.

## Coverage

Sync commit: `3a46d797c79915a6b13d0d09df0cc1b357e0b2d7` (verified after commit `0149b85`; `2df5c4a` added `retire --abandon`, which `finish --retire` does not use; `3a46d79` moved the push decision into `Worker::preflight()` in process, which still returns the approved remote and destination or stops, so only line numbers moved).

| Code at `3a46d79` (`wl/src/finish.rs` unless named) | Model action |
| --- | --- |
| `finish()` validation checks, `create_run()` (`runs/<id>.json`), `Store::write_json` of `accept.json` (lines 24-89, 104-121) | `FinishWriteAccept` |
| `finish()` `Store::create_new` of `queue/<target>/<ts>-<task>` (line 90) | `FinishEnqueue` (split when `PreemptAccept`) |
| `housekeeping()` restart + `work()` `try_lock_exclusive` & progress load (lines 213-225, 313-333) | `WorkerStart` |
| `Worker::step()` `Stage::Accepted` -> `push_topic()` (lines 374-380, 435-440) | `WorkerPushTopic` |
| `wait_for_turn()`, `queue_entry_task()`, `needs_queue()` (lines 191-203, 452-473) | `WorkerWaitTurn` |
| `prepare()` `lock_integration()` (line 509; `store.rs:32, 139`) | `WorkerPrepareAcquire` |
| `prepare()` fetch, ancestor short-circuit, fast-forward/merge in `integrate-<target>` (lines 510-532) | `WorkerPrepareMerge` |
| `prepare()` `.lifecycle.toml` validation run under lock (lines 535-554) | `WorkerPrepareValidate` |
| `push_target()` topic `wait_for_checks(commit)` (line 593) | `WorkerCheckTopicCI` |
| `push_target()` `lock_integration()` (line 597) | `WorkerPushAcquire` |
| `push_target()` `HoldRecord`, fetch, remote tip compare, `preflight`, push, and the refetch after a refused push (lines 594-623) | `WorkerPushTarget` |
| `Worker::step()` `Stage::TargetPushed` -> `leave_queue()` & `wait_for_checks(merged)` (lines 392-399) | `WorkerTargetChecks` |
| `Worker::step()` `Stage::Integrated` -> `commands.rs` `retire_task()` safety checks & task delete (lines 400-403; `commands.rs:301-388`) | `WorkerRetireRemoveTask` |
| `retire_task()` post-delete `write_retired_run()` (`commands.rs:389-390`; `finish.rs:128-146`) | `WorkerRetireWriteRun` |
| `retire_task()` `clear_finish_files()` (`commands.rs:391`; `finish.rs:148-153`) | `WorkerRetireCleanup` |
| `housekeeping()` / `begin()` / absent `retire_task()` -> `finalize_retired_finish_locked()` (`finish.rs:155-171, 217-221`; `commands.rs:188-190, 309-316`) | `HousekeepingFinalizeRetire` |
| `commands.rs` `begin()` re-creating a retired task (`commands.rs:170-237`) | `UserRebegin` |
| `work()` `write_run()` & `Store::write_durable(&progress_path, &progress)` (lines 341-346) | `WorkerWriteStage` |
| `work()` post-durable `worker.leave_queue()` (lines 348-350, 475-481) | `WorkerLeaveQueue` |
| Worker process termination (`worker_lock` & `integration_lock` drop, disk files stay) | `WorkerKill` |

## Run

Requirements: Python 3.10+, Java 11+, and `tla2tools.jar` from TLA+ v1.7.4 (`TLC2 Version 2.19 of 08 August 2024`, SHA-256 `936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`).

From the repository root:

```sh
python -B skills/verify-finish-integration/scripts/check.py --mutants --java JAVA --tla2tools TLA2TOOLS_JAR
```

`--java` and `--tla2tools` fall back to `JAVA` and `TLA2TOOLS`. Exit `0` means every config matches `EXPECTED` and every mutant breaks its expected invariant; `1` means a mismatch; `2` means TLC could not run.

## Read a result

Read every result with `/statechart`. Give it `model/FinishIntegration.tla` and the failing `.cfg` (or `model/FinishChart.tla` with `model/chart.cfg` for the single-task overview chart), plus the view `model/chart.mjs`. `/statechart` shows each invariant violation as its shortest step-by-step path in plain language.

If a run disagrees with `EXPECTED`:

1. **Invariant violation**: replay the `/statechart` path step-by-step against real `wl` before altering the model or muting the failure.
2. **Intentional contract change**: confirm against code and tests, update `EXPECTED`, the Coverage table, and the status note above, and advance the sync note only after re-running `--mutants`.
3. **Mutant no longer caught**: either the guard/lock in `wl/src/` regressed or `MUTANTS` text in `scripts/check.py` drifted out of sync with `FinishIntegration.tla`.

To confirm the check catches a real code change on a scratch copy, remove `self.lock_integration()?` in `wl/src/finish.rs` (`prepare()` line 509) and flip `WorkerPrepareAcquire` (`IF TRUE` -> `IF FALSE`) in `model/FinishIntegration.tla` to see `two-tasks-accept-race` fail `ExclusiveIntegrationWorktree`, or skip `write_retired_run` (`commands.rs:357`) and flip `WorkerRetireWriteRun` (`IF TRUE` -> `IF FALSE`) to see `single-task-retire-reuse` fail `RetiredRunMatchesExecution`, then drop the edits and re-verify exit `0`.

## What the model leaves out

- **Bounds**: 1 task with up to 1 fix commit, 3 finish attempts, and 1 external push (`single-task-gated` / `single-task-ungated`), 1 task with up to 1 re-begin after retirement (`single-task-retire-reuse`); 2 tasks with 1 commit and 1 finish attempt (`two-tasks-*`).
- **Not modeled**: explicit `wl retire`, `--target` selection (one shared target branch is modeled), `--base`, WSL offload transport details (`tar -m` / `wl-offload`), `gh auth token` / HTTP polling retries, `wl hold` arriving concurrently *between* `push_target`'s `HoldRecord` check (`finish.rs:594`) and `git push` (`finish.rs:613`), `reset_integration_worktree` failures on ignored data, a target push the remote refuses (the [required checks skill](../verify-required-checks/SKILL.md) models it: an unmoved target stops in needs-action like a preflight that holds, and changes no remote), Windows toast notifications, and `notices/` removal races (`finish.rs:226-234`).
- A pass holds under these stated bounds and assumptions, not for unmodeled multi-target or OS-filesystem edge cases.
