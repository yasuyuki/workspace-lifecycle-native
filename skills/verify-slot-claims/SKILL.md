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
- `NoRetireUnsafeTask`: `retire_task()` never retires a task that is held, dirty/untracked, or has a finish in progress, never retires one whose commits are on neither the target nor `origin/<branch>`, and retires one whose commits are off the target only for `retire --abandon --decision REF`.
- `SlotBranchMatchesReadyTask`: every `TaskStage::Ready` task not mid-retirement has its slot checkout on its recorded branch.
- `UniqueRetirementOrder`: no two retired slots share the same non-zero `retirement_order`.
- `OldestRetiredSlotReusedFirst`: `claim_slot()` picks the unclaimed free slot with the earliest non-zero `retirement_order` before any never-retired free slot.

**Harm when they break.** Leaked `.claim` or `#prepare` files permanently remove slots from the reusable pool so `wl` keeps cloning fresh checkouts, `wl begin` / `wl retire` races detach or overwrite another task's active slot branch mid-edit, concurrent retirements collide on `retirement_order` or skip the oldest retired slot, or dirty/unpublished/held user work is erased during retirement.

## Current status at `2df5c4ac9572b3d2b45cce890ea0bc3da7901ddf`

All eight configurations (`single-task-retire-guards`, `two-tasks-concurrent`, `slot-reuse-order`, `finding-begin-base-error`, `finding-prepare-crash`, `finding-retire-crash`, `finding-same-task-begin-race`, `finding-begin-retire-race`) pass all ten invariants (`EXPECTED` is `None` for every config), and all 16 mutants break their expected invariant. In addition to the five findings resolved in `dce9d18`:

1. `119601a` added `SlotRecord.retirement_order` (`wl/src/commands.rs:111-116, 144, 340-349`) and `Store::retirement_lock()` (`wl/src/store.rs:23`, `commands.rs:101, 337`), so `retire_task()` assigns monotonically increasing `retirement_order` values before removing `tasks/<task>.json` and `claim(n)` and `claim_slot()` reuses the earliest-retired free slot first (`slot-reuse-order`).
2. Note on `finding-same-task-begin-race`: because `claim_slot()` now serializes slot selection under `retirement_lock()`, omitting `task_lock` in `begin()` still fails `TaskSlotMatchesClaim` (concurrent `reclaim_orphan_task_claims()` deletes the un-locked `claim` before `BeginWriteTask` writes `tasks/<task>.json`) rather than claiming two slots.
3. `0149b85` added `finalize_retired_finish_locked()` inside `begin()` (`commands.rs:189`), `housekeeping()` (`finish.rs:219`), and absent-task `retire_task()` (`commands.rs:288`), and `write_retired_run()` / `clear_finish_files()` after `retire_task()`'s `retirement_lock` critical section (`commands.rs:356-359`); claim/slot ownership semantics remain preserved.
4. `2df5c4a` added `retire --abandon --decision REF` (`commands.rs:266-295`). `retire_task()` takes `abandon` and, when the tip is not in the target, accepts it only with `abandon` and only when `on_remote_topic()` finds it in `origin/<branch>` fetched from the remote (`commands.rs:349-358`). The model's `unpublished` is now `none`, `topic` or `local`, `RetireCheckAndClean` chooses `abandon` when `AllowAbandon`, and `NoRetireUnsafeTask` adds that commits off the target are left only by an abandonment. This is a promise changed on purpose (workspace-lifecycle-native#8). Old wording: never retires an unpublished task. The mutant `retire --abandon skips the remote topic check` shows the fetch-and-ancestor check is needed. `finish --retire` passes `abandon = false`, so the finish model is unchanged.

## When to use

Run this check when a change touches slot/claim/task lifecycle code in `wl/src/commands.rs`, `wl/src/store.rs`, `wl/src/finish.rs` (`housekeeping`), or `wl/src/model.rs`. To see what changed since the model was last confirmed:

```sh
sync=$(git log -1 --format=%H -- skills/verify-slot-claims)
git diff "$sync" -- wl/src/commands.rs wl/src/store.rs wl/src/model.rs wl/src/finish.rs
```

Empty output (or changes only inside `hold`, `release_hold`, `status`, or `Worker` internals outside `housekeeping` and `finish_status`) means the model still describes the covered functions.

## Coverage

Sync commit: `2df5c4ac9572b3d2b45cce890ea0bc3da7901ddf` (verified after commits `119601a`, `0149b85` and `2df5c4a`).

| Code at `2df5c4a` (`wl/src/commands.rs` unless named) | Model action |
| --- | --- |
| `begin()` `ws.lock_task(&task)` (`store.rs:31, 139`, `commands.rs:48-50, 188`) | `BeginAcquireLock` |
| `begin()` `resolve_base()` before `claim_slot()`, task lookup, `retirement_lock`, initial claim scan (lines 191-196, 100-107) | `BeginResolveAndScan` / `BeginBaseError` |
| `claim_slot()` / `claim_new_slot()` earliest-`retirement_order` choice + `Store::create_new(&claim(n), owner)` (lines 108-132) | `BeginCreateClaim` |
| `begin()` `Store::create_new_json(&task_path, &fresh)` (lines 198-216) | `BeginWriteTask` |
| `begin()` `ensure_slot_checkout()` worktree add + `slots/<n>.json` (`retirement_order: None`) (lines 226, 135-147) | `BeginEnsureCheckout` |
| `begin()` `switch_slot()` dirty check, branch switch, `write_json(Ready)` (lines 227-263) | `BeginSwitchAndReady` |
| `retire()` `--abandon`/`--decision` parsing, `retire_task()` `lock_task()`, absent-task cleanup, safety checks incl. `on_remote_topic()` & clean (lines 266-295, 301-362, 52-59) | `RetireCheckAndClean` |
| `retire_task()` `retirement_lock()` acquire + max `retirement_order` scan (lines 367-380; `store.rs:23`) | `RetireReadOrder` |
| `retire_task()` `retirement_order` write (lines 381-382) + `Store::remove(&task_path)` (line 384) | `RetireRemoveTask` |
| `retire_task()` conditional `Store::remove(&claim(n))` + drop `retirement_lock` (lines 385-388) | `RetireRemoveClaim` / `RetireCrashAfterTaskDelete` |
| `prepare_slot()` lock acquire, free/new `#prepare` claim creation (lines 482-523) | `PrepareClaimSlot` |
| `prepare_slot()` checkout/fetch/settle + error-safe `Store::remove(&claim(n))` (lines 499-523) | `PrepareFinish` / `PrepareCrash` |
| `finish::housekeeping()` (`finish.rs:213-225`) -> `reclaim_orphan_claims()` (lines 61-97) | `HousekeepingReclaim` |
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

To confirm the check catches a real code change on a scratch copy, remove `let _task_lock = ws.lock_task(&task)?;` in `wl/src/commands.rs` (`begin()` line 188) and flip `BeginAcquireLock` (`IF TRUE` -> `IF FALSE`) in `model/SlotClaims.tla` to see `finding-same-task-begin-race` fail `TaskSlotMatchesClaim`, or remove `(order.is_none(), order, n)` ordering in `claim_slot()` (`commands.rs:113`) and flip `BeginCreateClaim` (`IF TRUE` -> `IF FALSE`) to see `slot-reuse-order` fail `OldestRetiredSlotReusedFirst`, then drop both edits and re-verify exit `0`.

## What the model leaves out

- **Bounds**: 1 task / 2 slots (`single-task-retire-guards`), 2 tasks / 3 slots (`two-tasks-concurrent` with `MaxCycles = 1`; `slot-reuse-order` with `MaxCycles = 2`), and at most 1 `prepare_slot()` run in `Init` -> `Done`. Crash and fault toggles are configured per scenario (`AllowBaseError`, `AllowPrepareCrash`, `AllowRetireCrash`, `AllowSameTaskRace`, `AllowRetireClaimed`).
- **Not modeled**: unbounded slot numbers beyond `{1, 2, 3}`, repeated begin/retire cycles beyond `MaxCycles`, `begin` failing mid-checkout inside `git worktree add` (`commands.rs:142`), `branch`/`--target` mismatch rejection (`commands.rs:219-221`), `switch_slot` dirty error when dirty files appear mid-`begin`, Windows file-handle sharing delays (`store.rs:96`), and internal details of `wl finish` workers (modeled separately in `skills/verify-finish-integration/`).
- A pass holds under these bounds and assumptions, not for arbitrary crash combinations when the finding toggles are disabled.
