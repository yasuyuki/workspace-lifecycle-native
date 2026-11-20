---
name: verify-slot-claims
description: Model-check that `wl begin`, `wl retire`, and `__prepare-slot` manage `slots/<n>.claim`, `tasks/<task>.json`, and `#prepare` claims safely without orphan claim leaks, duplicate slot claims, cross-task slot collisions, or unsafe retirement. Use when a change touches `wl/src/commands.rs`, `wl/src/store.rs`, or `wl/src/model.rs` slot/task claim logic.
license: MIT
---

# Verify slot and claim lifecycle (`wl`)

**Rules.**

- `ExclusiveSlotOwnership`: no two live tasks own the same slot number, and `prepare_slot()` never modifies a slot claimed by a task.
- `TaskSlotMatchesClaim`: every live `tasks/<task>.json` points to a slot whose `slots/<n>.claim` still names `<task>`.
- `AtMostOneClaimPerTask`: no task simultaneously holds more than one `slots/<n>.claim` file.
- `NoStalePrepareClaimWhenIdle`: when `prepare_slot()` is not running (`workers/prepare.lock` released), no slot retains `#prepare` in `slots/<n>.claim`.
- `NoOrphanClaimWhenIdle`: when no `begin`, `retire`, or `prepare_slot` operation is in progress, every `slots/<n>.claim` belongs to a live `tasks/<task>.json` pointing at slot `n`.
- `NoRetireUnsafeTask`: `retire_task()` never retires a task that is held, dirty/untracked, unpublished, or has a finish in progress.
- `SlotBranchMatchesReadyTask`: every `TaskStage::Ready` task not mid-retirement has its slot checkout on its recorded branch.

**Harm when they break.** Leaked `.claim` or `#prepare` files permanently remove slots from the reusable pool so `wl` keeps cloning fresh checkouts, `wl begin` / `wl retire` races detach or overwrite another task's active slot branch mid-edit, or dirty/unpublished/held user work is erased during retirement.

## Current findings at `a0a389f3312c98f267ea88f2ec5f86c651948651`

Five finding configurations in `model/` fail against the current code (do not weaken the rules or model to hide them):

1. `finding-begin-base-error` -> violates `NoOrphanClaimWhenIdle`: `begin()` claims `slots/<n>.claim` via `ws.claim_slot(task.as_str())` (`wl/src/commands.rs:133`) before calling `repo::resolve_base(...)` (`commands.rs:138, 142`). If `resolve_base()` fails (bad `--base` or missing `origin/HEAD`), `begin()` returns `Err` without creating `tasks/<task>.json` and without releasing `slots/<n>.claim`. Because `retire_task()` returns `Ok(())` early when `tasks/<task>.json` is absent (`commands.rs:226-228`), the claimed slot is permanently leaked.
2. `finding-prepare-crash` -> violates `NoStalePrepareClaimWhenIdle`: `prepare_slot()` creates a `#prepare` claim via `Store::create_new(&ws.store.claim(n), PREPARING.as_bytes())` (`commands.rs:372`) or `ws.claim_new_slot(PREPARING)` (`commands.rs:382`) and only deletes it at lines 378 / 387. If `__prepare-slot` crashes, is killed, or exits early via `?` (line 373 `has_changes` error) while `prepare.lock` drops, `#prepare` remains in `slots/<n>.claim` and nothing ever reclaims stale `#prepare` claims.
3. `finding-retire-crash` -> violates `NoOrphanClaimWhenIdle`: `retire_task()` deletes `tasks/<task>.json` (`commands.rs:269`) before deleting `slots/<n>.claim` (`commands.rs:270-272`). If the process crashes in that gap, the next `wl retire <task>` sees no `tasks/<task>.json` and returns `Ok(())`, permanently leaking `slots/<n>.claim`.
4. `finding-same-task-begin-race` -> violates `AtMostOneClaimPerTask`: two concurrent `wl begin <task>` processes scanning `slots/*.claim` (`commands.rs:49-63, 133`) can both find no existing claim for `<task>` and create distinct `slots/1.claim` and `slots/2.claim` with content `<task>`. `Store::create_new_json(&task_path)` (`commands.rs:154-158`) picks one winner without releasing the loser's claim, leaving the extra slot claimed by `<task>` even after `wl retire <task>`.
5. `finding-begin-retire-race` -> violates `TaskSlotMatchesClaim` (and `ExclusiveSlotOwnership` when a second task enters): `retire_task()` (`commands.rs:226`) accepts any existing `TaskRecord`, including `TaskStage::Claimed`. If `wl retire <task>` completes while an earlier `wl begin <task>` is suspended between `create_new_json(&task_path)` (`commands.rs:154`) and `switch_slot` / `write_json(TaskStage::Ready)` (`commands.rs:173-175`), the resumed `begin` overwrites `tasks/<task>.json` back to `Ready` after its `.claim` was deleted and after another task may have claimed that same slot.

## When to use

Run this check when a change touches slot/claim/task lifecycle code in `wl/src/commands.rs`, `wl/src/store.rs`, or `wl/src/model.rs`. To see what changed since the model was last confirmed:

```sh
sync=$(git log -1 --format=%H -- skills/verify-slot-claims)
git diff "$sync" -- wl/src/commands.rs wl/src/store.rs wl/src/model.rs
```

Empty output (or changes only inside `hold`, `release_hold`, `status`, or `spawn_detached`) means the model still describes the covered functions.

## Coverage

Sync commit: `a0a389f3312c98f267ea88f2ec5f86c651948651` (initial confirmation at `verify-slot-claims` creation; advance after verifying code alignment at a newer commit).

| Code at `a0a389f` (`wl/src/commands.rs` unless named) | Model action |
| --- | --- |
| `begin()` task lookup + `claim_slot()` initial scan of `slots/*.claim` (lines 129-133, 49-55) | `BeginStartScan` |
| `claim_slot()` / `claim_new_slot()` `Store::create_new(&claim(n), owner)` loop (lines 56-72) | `BeginCreateClaim` |
| `begin()` `resolve_base()` + `Store::create_new_json(&task_path, &fresh)` (lines 135-159) | `BeginWriteTask` / `BeginBaseError` |
| `begin()` `ensure_slot_checkout()` worktree add + `slots/<n>.json` write (lines 168, 75-86) | `BeginEnsureCheckout` |
| `begin()` `switch_slot()` dirty check, branch switch, `write_json(Ready)` (lines 173-208) | `BeginSwitchAndReady` |
| `retire_task()` hold / `finish::running` / dirty / published checks + branch detach & `clear_ignored` (lines 224-268) | `RetireCheckAndClean` |
| `retire_task()` `Store::remove(&task_path)` (line 269) | `RetireRemoveTask` |
| `retire_task()` conditional `Store::remove(&claim(n))` (lines 270-272) | `RetireRemoveClaim` / `RetireCrashAfterTaskDelete` |
| `prepare_slot()` lock acquire, free/new `#prepare` claim creation (lines 355-382) | `PrepareClaimSlot` |
| `prepare_slot()` checkout/fetch/settle + `Store::remove(&claim(n))` (lines 373-388) | `PrepareFinish` / `PrepareCrash` |
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

1. **Product change broke a passing config or introduced a new leak/race**: replay the `/statechart` path against real `wl` before editing the model or muting the failure.
2. **Code fix resolved a recorded finding**: confirm the matching replay on real `wl` no longer reproduces the leak or race, update `EXPECTED` in `scripts/check.py` (to `None` or the remaining violated invariant), update the Coverage table and Findings above, and advance the sync note only after re-running `--mutants`.
3. **Mutant no longer caught**: either the guard/ordering in `wl/src/commands.rs` regressed or `MUTANTS` text in `scripts/check.py` drifted out of sync with `SlotClaims.tla`.

To confirm the check catches a real code change on a scratch copy, remove a guard in `wl/src/commands.rs` (for example the `HoldRecord` check at `retire_task` line 229), apply the matching edit in `model/SlotClaims.tla` (`/\ ~hold[t]` -> `/\ TRUE`), run `scripts/check.py` and observe `match: false` on `single-task-retire-guards` (`NoRetireUnsafeTask`), then drop the scratch edits and re-verify exit `0`.

## What the model leaves out

- **Bounds**: 1 task / 1 slot (`single-task-retire-guards`), 2 tasks / 3 slots (`two-tasks-concurrent`), 1 `begin` cycle per actor and at most 1 `prepare_slot()` run in `Init` -> `Done`. Crash and fault toggles are isolated in dedicated finding configs (`AllowBaseError`, `AllowPrepareCrash`, `AllowRetireCrash`, `AllowSameTaskRace`, `AllowRetireClaimed`) so passing baseline runs stay deterministic under those assumptions.
- **Not modeled**: unbounded slot numbers beyond `{1, 2, 3}`, repeated begin/retire cycles beyond `MaxCycles = 1`, `begin` failing mid-checkout inside `git worktree add` (`commands.rs:82`), `prepare_slot` creating a slot while `begin` enters `ensure_slot_checkout` with the `branch`/`--target` mismatch rejection (`commands.rs:161-163`), `switch_slot` dirty error when dirty files appear between `begin` scans, Windows file-handle sharing delays (`store.rs:91`), and internal details of `wl finish` workers (modeled separately in `skills/verify-finish-integration/`).
- A pass holds under these bounds and assumptions, not for arbitrary crash combinations when the finding toggles are disabled.
