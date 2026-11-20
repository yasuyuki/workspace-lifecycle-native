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

**Harm when they break.** Two workers overwrite each other's `integrate-<target>` checkout mid-validation so unchecked code lands on `main`, a running task's queue entry is deleted early, unverified commits reach `main`, held tasks integrate, or dirty/unpushed user work is erased during retirement.

## Current status at `dce9d18489a94fad25344283e2e77505ce430112`

All five configurations (`single-task-gated`, `single-task-ungated`, `two-tasks-crash-queue`, `two-tasks-accept-race`, `two-tasks-suffix`) pass all seven invariants (`EXPECTED` is `None` for every config). The three previously open two-task findings were resolved in commit `49d2d43` (included in `dce9d18`):

1. `two-tasks-suffix` -> `queue_entry_task()` (`wl/src/finish.rs:110-112`) parses `<ts>-<task>` via `split_once('-')` instead of `ends_with()`, so `wait_for_turn()` and `leave_queue()` match only exact owners.
2. `two-tasks-accept-race` -> `prepare()` (`finish.rs:356`) and `push_target()` (`finish.rs:444`) acquire the per-target exclusive OS lock `Store::lock_exclusive(&integration_lock(target))` (`wl/src/store.rs:29, 136`), preventing concurrent entry into `integrate-<target>` even when an older `accept.json` creates its queue file late.
3. `two-tasks-crash-queue` -> `Worker::work()` (`finish.rs:197-203`) writes `Stage::TargetPushed` / `Stage::NeedsAction` via `Store::write_durable()` before calling `worker.leave_queue()`, `wait_for_turn()` checks `needs_queue()` (`finish.rs:115-122`), unguarded `merge --abort` was removed from `withdraw()`, and `integration_lock` serializes `prepare()` / `push_target()` across restarts.

## When to use

Run this check when a change touches covered code in `wl/src/finish.rs`, `wl/src/store.rs`, or `wl/src/commands.rs`. To see what changed since the model was last confirmed:

```sh
sync=$(git log -1 --format=%H -- skills/verify-finish-integration)
git diff "$sync" -- wl/src/finish.rs wl/src/store.rs wl/src/commands.rs
```

Empty output (or changes only inside `begin`, `status`, `hold`, `release_hold`, `prepare_slot`, or `reclaim_*` outside `retire_task` and `Store::*lock*`) means the model still describes the covered functions.

## Coverage

Sync commit: `dce9d18489a94fad25344283e2e77505ce430112` (verified against `49d2d43` and `dce9d18`).

| Code at `dce9d18` (`wl/src/finish.rs` unless named) | Model action |
| --- | --- |
| `finish()` validation checks + `Store::write_json` of `accept.json` (lines 24-79) | `FinishWriteAccept` |
| `finish()` `Store::create_new` of `queue/<target>/<ts>-<task>` (line 80) | `FinishEnqueue` (split when `PreemptAccept`) |
| `housekeeping()` restart + `work()` `try_lock_exclusive` & progress load (lines 134-139, 167-187) | `WorkerStart` |
| `Worker::step()` `Stage::Accepted` -> `push_topic()` (lines 227-234, 282-287) | `WorkerPushTopic` |
| `wait_for_turn()`, `queue_entry_task()`, `needs_queue()` (lines 110-122, 299-320) | `WorkerWaitTurn` |
| `prepare()` `lock_integration()` (`store.rs:29, 136`) | `WorkerPrepareAcquire` |
| `prepare()` fetch, ancestor short-circuit, fast-forward/merge in `integrate-<target>` (lines 357-381) | `WorkerPrepareMerge` |
| `prepare()` `.lifecycle.toml` validation run under lock (lines 382-403) | `WorkerPrepareValidate` |
| `push_target()` topic `wait_for_checks(commit)` (lines 437-440) | `WorkerCheckTopicCI` |
| `push_target()` `lock_integration()` (line 444) | `WorkerPushAcquire` |
| `push_target()` `HoldRecord`, fetch, remote tip compare, `preflight`, push (lines 441-464) | `WorkerPushTarget` |
| `Worker::step()` `Stage::TargetPushed` -> `leave_queue()` & `wait_for_checks(merged)` (lines 245-252) | `WorkerTargetChecks` |
| `Worker::step()` `Stage::Integrated` -> `commands.rs` `retire_task()` (lines 253-256; `commands.rs:268-321`) | `WorkerRetire` |
| `work()` `Store::write_durable(&progress_path, &progress)` (lines 197-199) | `WorkerWriteStage` |
| `work()` post-durable `worker.leave_queue()` (lines 201-203, 322-328) | `WorkerLeaveQueue` |
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

To confirm the check catches a real code change on a scratch copy, remove `self.lock_integration()?` in `wl/src/finish.rs` (`prepare()` line 356), apply the matching edit in `model/FinishIntegration.tla` (`WorkerPrepareAcquire` `IF TRUE` -> `IF FALSE`), run `scripts/check.py` and observe `match: false` on `two-tasks-accept-race` (`ExclusiveIntegrationWorktree`), then drop both edits and re-verify exit `0`.

## What the model leaves out

- **Bounds**: 1 task with up to 1 fix commit, 3 finish attempts, and 1 external push (`single-task-*`); 2 tasks with 1 commit and 1 finish attempt (`two-tasks-*`).
- **Not modeled**: explicit `wl retire`, `--target` selection (one shared target branch is modeled), `--base`, WSL offload transport details (`tar -m` / `wl-offload`), `gh auth token` / HTTP polling retries, `wl hold` arriving concurrently *between* `push_target`'s `HoldRecord` check (`finish.rs:441`) and `git push` (`finish.rs:460`), `reset_integration_worktree` failures on ignored data, Windows toast notifications, and `notices/` removal races (`finish.rs:140-149`).
- A pass holds under these stated bounds and assumptions, not for unmodeled multi-target or OS-filesystem edge cases.
