---
name: verify-finish-integration
description: Model-check that `wl finish` and its detached worker push only validated and CI-checked commits to the integration target, enforce holds and retirement safety, and expose concurrency/crash-recovery races on the shared integration worktree and queue. Use when a change touches `wl/src/finish.rs` (`finish`, `work`, `housekeeping`, `Worker::step`, `push_topic`, `wait_for_turn`, `leave_queue`, `prepare`, `push_target`, `wait_for_checks`, `withdraw`) or `wl/src/commands.rs` (`retire_task`).
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

## Current findings at `a0a389f3312c98f267ea88f2ec5f86c651948651`

Three two-task configurations in `model/` are known findings against the current code (do not weaken the rules to hide them):

1. `two-tasks-crash-queue` -> violates `ExclusiveIntegrationWorktree`: `Worker::work()` calls `worker.withdraw()` (which runs `leave_queue()`) before `Store::write_durable(&progress_path, &progress)` saves `Stage::NeedsAction`, and `push_target()` also calls `leave_queue()` before `write_durable(Stage::TargetPushed)`. If the worker dies in that gap, the task remains `running()` at `TopicPushed` (or `Prepared`) without a queue entry; `housekeeping()` restarts it, and `wait_for_turn()` re-inserts its older `accepted_at_ms` ahead of an active newer worker already using `integrate-<target>` (or restarted `Prepared` fetches inside `integrate-<target>` directly).
2. `two-tasks-accept-race` -> violates `ExclusiveIntegrationWorktree`: `finish()` writes `accept.json` (`finish.rs:79`) before `Store::create_new` writes `queue/<ts>-<task>` (`finish.rs:80`). If preempted between those writes, a newer `finish` enters `integrate-<target>` first, and the older `finish` later creates a smaller-timestamp queue entry and enters `integrate-<target>` concurrently.
3. `two-tasks-suffix` -> violates `QueueEntryMatchesOwner` (and `ExclusiveIntegrationWorktree`): `wait_for_turn()` and `leave_queue()` match entries with `ends_with(&format!("-{}", self.task()))`. Task `b` matches `x-b`'s queue entry `<ts>-x-b`, steals turn into `integrate-<target>`, and deletes `x-b`'s queue entry.

## When to use

Run this check when a change touches covered code in `wl/src/finish.rs` or `wl/src/commands.rs`. To see what changed since the model was last confirmed:

```sh
sync=$(git log -1 --format=%H -- skills/verify-finish-integration)
git diff "$sync" -- wl/src/finish.rs wl/src/commands.rs
```

Empty output (or changes only inside `begin`, `status`, `hold`, `release_hold`, `prepare_slot`) means the model still describes the covered functions.

## Coverage

Sync commit: `a0a389f3312c98f267ea88f2ec5f86c651948651` (initial confirmation at `verify-finish-integration` creation; advance after verifying code alignment at a newer commit).

| Code at `a0a389f` (`wl/src/finish.rs` unless named) | Model action |
| --- | --- |
| `finish()` validation checks + `Store::write_json` of `accept.json` (lines 24-79) | `FinishWriteAccept` |
| `finish()` `Store::create_new` of `queue/<target>/<ts>-<task>` (line 80) | `FinishEnqueue` (split when `PreemptAccept`) |
| `housekeeping()` restart + `work()` lock acquisition & progress load (lines 118-124, 152-176) | `WorkerStart` |
| `Worker::step()` `Stage::Accepted` -> `push_topic()` (lines 212-219, 266-271) | `WorkerPushTopic` |
| `wait_for_turn()` queue inspection, stale cleanup, and re-insert (lines 279-301) | `WorkerWaitTurn` |
| `prepare()` fetch, ancestor short-circuit, fast-forward/merge in `integrate-<target>` (lines 337-362) | `WorkerPrepareMerge` |
| `prepare()` `.lifecycle.toml` validation run on `integrate-<target>` (lines 363-384) | `WorkerPrepareValidate` |
| `push_target()` topic `wait_for_checks(commit)` (lines 418-421) | `WorkerCheckTopicCI` |
| `push_target()` `HoldRecord`, fetch, remote tip compare, `preflight`, push, `leave_queue()` (lines 422-445) | `WorkerPushTarget` |
| `Worker::step()` `Stage::TargetPushed` -> `wait_for_checks(merged)` (lines 230-236) | `WorkerTargetChecks` |
| `Worker::step()` `Stage::Integrated` -> `commands.rs` `retire_task()` (lines 237-240; `commands.rs:224-275`) | `WorkerRetire` |
| `work()` error branch `worker.withdraw()` / `leave_queue()` (lines 180-183, 498-505) | `WorkerWithdraw` |
| `work()` `Store::write_durable(&progress_path, &progress)` (lines 186-188) | `WorkerWriteStage` |
| Worker process termination (`workers/<task>.lock` drops, disk files stay) | `WorkerKill` |

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

1. **Product change broke a single-task rule or introduced a new race**: replay the `/statechart` path step-by-step against real `wl` before altering the model or muting the failure.
2. **Code fix resolved a recorded two-task finding**: confirm the matching replay on real `wl` no longer reproduces the race, update `EXPECTED` in `scripts/check.py` (to `None` or the remaining rule), update the Coverage table and Findings above, and advance the sync note only after re-running `--mutants`.
3. **Mutant no longer caught**: either the guard in `wl/src/` was lost or `MUTANTS` text in `scripts/check.py` drifted out of sync with `FinishIntegration.tla`.

To confirm the check catches a real code change on a scratch copy, remove a guard in `wl/src/finish.rs` (for example `if let Some(hold) = ...` in `push_target()`), apply the matching change in `model/FinishIntegration.tla` (`IF hold[t]` -> `IF FALSE`), run `scripts/check.py` and observe `match: false` on `single-task-ungated` (`NoTargetPushWhileHeld`), then revert both edits and re-verify exit `0`.

## What the model leaves out

- **Bounds**: 1 task with up to 1 fix commit, 3 finish attempts, and 1 external push (`single-task-*`); 2 tasks with 1 commit and 1 finish attempt (`two-tasks-*`).
- **Not modeled**: explicit `wl retire`, `--target` selection (one shared target branch is modeled), `--base`, WSL offload transport details (`tar -m` / `wl-offload`), `gh auth token` / HTTP polling retries, `wl hold` arriving concurrently *between* `push_target`'s `HoldRecord` check (`finish.rs:422`) and `git push` (`finish.rs:441`), merge-Tree/`reset_integration_worktree` failures on ignored data, Windows toast notifications, and `wl begin` / `prepare_slot` slot-claim leaks when `wl begin` or `prepare_slot` crashes mid-claim.
- A pass holds under these stated bounds and assumptions, not for unmodeled multi-target or OS-filesystem edge cases.
