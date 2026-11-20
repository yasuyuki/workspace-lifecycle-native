---
name: verify-slot-claims
description: Model-check that `wl begin`, `wl retire`, `__prepare-slot`, and `housekeeping()` manage `slots/<n>.claim`, `tasks/<task>.json`, per-task locks, and `#prepare` claims safely without orphan claim leaks, duplicate slot claims, cross-task slot collisions, or unsafe retirement. Use when a change touches `wl/src/commands.rs`, `wl/src/store.rs`, `wl/src/finish.rs` (`housekeeping`), or `wl/src/model.rs`.
license: MIT
---

# Verify slot and claim lifecycle (`wl`)

**Rules.**

- `ExclusiveSlotOwnership`: no two live tasks own the same slot number, and `prepare_slot()` never modifies a slot claimed by a task.
- `TaskSlotMatchesClaim`: every live `tasks/<task>.json` points to a slot whose `slots/<n>.claim` still names `<task>`.
- `AtMostOneClaimPerTask`: no task simultaneously holds more than one `slots/<n>.claim` file.
- `NoStalePrepareClaimWhenIdle`: when `prepare_slot()` is not running (`workers/prepare.lock` released), no slot retains `#prepare` in `slots/<n>.claim` (for crash configs, holds once `housekeeping()` runs `reclaim_orphan_claims()` via `NoOrphanClaimAfterHousekeeping`).
- `NoOrphanClaimWhenIdle`: when no `begin`, `retire`, or `prepare_slot` operation is in progress, every `slots/<n>.claim` belongs to a live `tasks/<task>.json` pointing at slot `n` (for crash configs, enforced after `housekeeping()` / `prepare_slot()` recovery via `NoOrphanClaimAfterHousekeeping`).
- `NoRetireUnsafeTask`: `retire_task()` never retires a task that is held, dirty/untracked, unpublished, or has a finish in progress.
- `SlotBranchMatchesReadyTask`: every `TaskStage::Ready` task not mid-retirement has its slot checkout on its recorded branch.

**Harm when they break.** Leaked `.claim` or `#prepare` files permanently remove slots from the reusable pool so `wl` keeps cloning fresh checkouts, `wl begin` / `wl retire` races detach or overwrite another task's active slot branch mid-edit, or dirty/unpublished/held user work is erased during retirement.

## Current status at `dce9d18489a94fad25344283e2e77505ce430112`

All seven configurations (`single-task-retire-guards`, `two-tasks-concurrent`, `finding-begin-base-error`, `finding-prepare-crash`, `finding-retire-crash`, `finding-same-task-begin-race`, `finding-begin-retire-race`) pass (`EXPECTED` is `None` for every config). The five findings recorded at `a0a389f` were resolved in commit `dce9d18`:

1. `finding-begin-base-error` -> `begin()` (`wl/src/commands.rs:183-186`) now calls `repo::resolve_base(...)` before `ws.claim_slot(task)` and cleans up on `create_new_json` error (`commands.rs:201-204`).
2. `finding-prepare-crash` -> `housekeeping()` (`wl/src/finish.rs:133`) calls `ws.reclaim_orphan_claims()` (`commands.rs:85-97`), `prepare_slot()` calls `reclaim_stale_prepare_claims_locked()` (`commands.rs:404`), and the existing-slot path cleans `#prepare` even on `has_changes` errors (`commands.rs:418-427`). Promise refinement for crash configs: `NoOrphanClaimAfterHousekeeping` verifies that once `housekeeping()` runs after a `SIGKILL`, zero stale `#prepare` claims remain (replacing strict pre-recovery `NoStalePrepareClaimWhenIdle` on the crash configs because `SIGKILL` cannot run destructors, evidenced by `wl/tests/cli.rs` `stale_prepare_and_orphan_task_claims_are_reclaimed_by_housekeeping`).
3. `finding-retire-crash` -> `retire_task()` calls `ws.release_claims_for(task)` when `tasks/<task>.json` is absent (`commands.rs:272`), and `housekeeping()` / `prepare_slot()` run `reclaim_orphan_task_claims()` under non-blocking `try_lock_exclusive(&task_lock(t))` (`commands.rs:64-83`). Promise refinement for crash configs: verified via `NoOrphanClaimAfterHousekeeping`.
4. `finding-same-task-begin-race` -> `begin()` holds `ws.lock_task(&task)` (`store.rs:28, 136`, `commands.rs:48-50, 178`) across lookup, claim, checkout, and `TaskStage::Ready` write, preventing duplicate claims from concurrent `wl begin <task>`.
5. `finding-begin-retire-race` -> `retire_task()` holds `ws.lock_task(task)` (`commands.rs:269`) across checks and deletion, serializing `begin` and `retire` on the same task.

## When to use

Run this check when a change touches slot/claim/task lifecycle code in `wl/src/commands.rs`, `wl/src/store.rs`, `wl/src/finish.rs` (`housekeeping`), or `wl/src/model.rs`. To see what changed since the model was last confirmed:

```sh
sync=$(git log -1 --format=%H -- skills/verify-slot-claims)
git diff "$sync" -- wl/src/commands.rs wl/src/store.rs wl/src/model.rs wl/src/finish.rs
```

Empty output (or changes only inside `hold`, `release_hold`, `status`, or `Worker` internals outside `housekeeping`) means the model still describes the covered functions.

## Coverage

Sync commit: `dce9d18489a94fad25344283e2e77505ce430112` (verified after commit `dce9d18`).

| Code at `dce9d18` (`wl/src/commands.rs` unless named) | Model action |
| --- | --- |
| `begin()` `ws.lock_task(&task)` (`store.rs:28, 136`, `commands.rs:48-50, 178`) | `BeginAcquireLock` |
| `begin()` `resolve_base()` before `claim_slot()`, task lookup, initial claim scan (lines 180-186, 100-106) | `BeginResolveAndScan` / `BeginBaseError` |
| `claim_slot()` / `claim_new_slot()` `Store::create_new(&claim(n), owner)` loop (lines 107-123) | `BeginCreateClaim` |
| `begin()` `Store::create_new_json(&task_path, &fresh)` (lines 187-205) | `BeginWriteTask` |
| `begin()` `ensure_slot_checkout()` worktree add + `slots/<n>.json` write (lines 215, 126-137) | `BeginEnsureCheckout` |
| `begin()` `switch_slot()` dirty check, branch switch, `write_json(Ready)` (lines 217-252) | `BeginSwitchAndReady` |
| `retire_task()` `lock_task()`, absent-task `release_claims_for()`, safety checks & clean (lines 268-310, 52-58) | `RetireCheckAndClean` |
| `retire_task()` `Store::remove(&task_path)` (line 315) | `RetireRemoveTask` |
| `retire_task()` conditional `Store::remove(&claim(n))` (lines 316-318) | `RetireRemoveClaim` / `RetireCrashAfterTaskDelete` |
| `prepare_slot()` lock acquire, free/new `#prepare` claim creation (lines 401-430) | `PrepareClaimSlot` |
| `prepare_slot()` checkout/fetch/settle + error-safe `Store::remove(&claim(n))` (lines 418-436) | `PrepareFinish` / `PrepareCrash` |
| `finish::housekeeping()` (`finish.rs:133`) -> `reclaim_orphan_claims()` (lines 60-97) | `HousekeepingReclaim` |
| Concurrent `hold` / dirty worktree / unpublished commit / active finish | `ToggleHold`, `ToggleDirty`, `ToggleUnpublished`, `ToggleFinishRunning` |

## Run

Requirements: Python 3.10+, Java 11+, and `tla2tools.jar` from TLA+ v1.7.4 (`TLC2 Version 2.19 of 08 August 2024`, SHA-256 `936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`).

From the repository root:

```sh
python -B skills/verify-slot-claims/scripts/check.py --mutants --java JAVA --tla2tools TLA2TOOLS_JAR
```

`--java` and `--tla2tools` fall back to `JAVA` and `TLA2TOOLS`. Exit `0` means every config matches `EXPECTED` and every mutant breaks its expected invariant; `1` means a mismatch; `2` means TLC could not run.

## Read a result

Read every result with `/statechart`. Give it `model/SlotClaims.tla` and the failing `.cfg` (or `model/SlotChart.tla` with `model/chart.cfg` for the nominal single-slot chart), plus the view `model/chart.mjs`. `/statechart` shows each invariant violation as its shortest step-by-step path in plain language.

If a run disagrees with `EXPECTED`:

1. **Invariant violation**: replay the `/statechart` path step-by-step against real `wl` before editing the model or muting the failure.
2. **Intentional contract change**: confirm against code and tests, update `EXPECTED`, the Coverage table, and status above, and advance the sync note only after re-running `--mutants`.
3. **Mutant no longer caught**: either the guard/lock/reclaim in `wl/src/` regressed or `MUTANTS` text in `scripts/check.py` drifted out of sync with `SlotClaims.tla`.

To confirm the check catches a real code change on a scratch copy, remove `let _task_lock = ws.lock_task(&task)?;` in `wl/src/commands.rs` (`begin()` line 178), apply the matching edit in `model/SlotClaims.tla` (`BeginAcquireLock` `IF TRUE` -> `IF FALSE`), run `scripts/check.py` and observe `match: false` on `finding-same-task-begin-race` (`AtMostOneClaimPerTask`), then drop both edits and re-verify exit `0`.

## What the model leaves out

- **Bounds**: 1 task / 2 slots (`single-task-retire-guards`), 2 tasks / 3 slots (`two-tasks-concurrent`), 1 `begin` cycle per actor and at most 1 `prepare_slot()` run in `Init` -> `Done`. Crash and fault toggles are configured per scenario (`AllowBaseError`, `AllowPrepareCrash`, `AllowRetireCrash`, `AllowSameTaskRace`, `AllowRetireClaimed`).
- **Not modeled**: unbounded slot numbers beyond `{1, 2, 3}`, repeated begin/retire cycles beyond `MaxCycles = 1`, `begin` failing mid-checkout inside `git worktree add` (`commands.rs:133`), `branch`/`--target` mismatch rejection (`commands.rs:208-210`), `switch_slot` dirty error when dirty files appear mid-`begin`, Windows file-handle sharing delays (`store.rs:93`), and internal details of `wl finish` workers (modeled separately in `skills/verify-finish-integration/`).
- A pass holds under these bounds and assumptions, not for arbitrary crash combinations when the finding toggles are disabled.
