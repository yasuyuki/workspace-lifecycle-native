"""Model-check SlotClaims.tla with TLC across all slot-claim configurations.

Exit 0 when every configuration matches EXPECTED (and, with --mutants, every
mutant breaks its expected invariant); 1 on mismatch; 2 when TLC cannot run.
"""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile


MODEL = Path(__file__).resolve().parents[1] / "model"
SPEC = "SlotClaims.tla"
TLC_VERSION = "TLC2 Version 2.19 of 08 August 2024"

# None: every invariant holds.
EXPECTED = {
    "single-task-retire-guards": None,
    "two-tasks-concurrent": None,
    "slot-reuse-order": None,
    "finding-begin-base-error": None,
    "finding-prepare-crash": None,
    "finding-retire-crash": None,
    "finding-same-task-begin-race": None,
    "finding-begin-retire-race": None,
}

# (name, code location, config, expected violated rule, original TLA+ text, broken TLA+ text)
MUTANTS = [
    (
        "claim_slot skips atomic create_new exclusivity",
        "wl/src/commands.rs claim_slot() Store::create_new(&claim(n))",
        "two-tasks-concurrent",
        "ExclusiveSlotOwnership",
        '        /\\ IF slotClaim[chosen] = "none"',
        "        /\\ IF TRUE",
    ),
    (
        "begin skips switch_slot branch checkout before writing Ready",
        "wl/src/commands.rs begin() switch_slot() call (line 228)",
        "two-tasks-concurrent",
        "SlotBranchMatchesReadyTask",
        "        /\\ IF TRUE \\* Mutant hook: skip switch_slot before writing Ready",
        "        /\\ IF FALSE \\* Mutant hook: skip switch_slot before writing Ready",
    ),
    (
        "prepare_slot skips #prepare claim before resetting free slot",
        "wl/src/commands.rs prepare_slot() Store::create_new(&claim(n), PREPARING)",
        "two-tasks-concurrent",
        "ExclusiveSlotOwnership",
        "             IN IF TRUE \\* Mutant hook: skip #prepare claim on free slot",
        "             IN IF FALSE \\* Mutant hook: skip #prepare claim on free slot",
    ),
    (
        "retire_task removes claim before removing task record",
        "wl/src/commands.rs retire_task() Store::remove order (lines 351-354)",
        "two-tasks-concurrent",
        "TaskSlotMatchesClaim",
        "        /\\ IF TRUE \\* Mutant hook: swap order so claim is removed BEFORE task record",
        "        /\\ IF FALSE \\* Mutant hook: swap order so claim is removed BEFORE task record",
    ),
    (
        "retire_task skips HoldRecord check",
        "wl/src/commands.rs retire_task() HoldRecord check",
        "single-task-retire-guards",
        "NoRetireUnsafeTask",
        "          /\\ ~hold[t]",
        "          /\\ TRUE",
    ),
    (
        "retire_task skips finish running check",
        "wl/src/commands.rs retire_task() finish::running check",
        "single-task-retire-guards",
        "NoRetireUnsafeTask",
        "          /\\ ~finishRunning[t]",
        "          /\\ TRUE",
    ),
    (
        "retire_task skips dirty and untracked check",
        "wl/src/commands.rs retire_task() git status check",
        "single-task-retire-guards",
        "NoRetireUnsafeTask",
        "          /\\ ~dirty[t]",
        "          /\\ TRUE",
    ),
    (
        "retire_task skips published commit check",
        "wl/src/commands.rs retire_task() `!published && !abandon` refusal",
        "single-task-retire-guards",
        "NoRetireUnsafeTask",
        '          /\\ (unpublished[t] = "none" \\/ ab)',
        "          /\\ TRUE",
    ),
    (
        "retire --abandon skips the remote topic check",
        "wl/src/commands.rs retire_task() `!published && !on_remote_topic()` refusal",
        "single-task-retire-guards",
        "NoRetireUnsafeTask",
        '          /\\ unpublished[t] # "local"',
        "          /\\ TRUE",
    ),
    (
        "begin() skips task_lock mutual exclusion across same-task begins",
        "wl/src/commands.rs begin() lock_task(&task) (line 188)",
        "finding-same-task-begin-race",
        "TaskSlotMatchesClaim",
        "     IN /\\ IF TRUE \\* Mutant hook: begin() acquires task_lock before checking task record",
        "     IN /\\ IF FALSE \\* Mutant hook: begin() acquires task_lock before checking task record",
    ),
    (
        "retire_task() skips task_lock mutual exclusion mid-begin",
        "wl/src/commands.rs retire_task() lock_task(task) (line 280)",
        "finding-begin-retire-race",
        "TaskSlotMatchesClaim",
        "  /\\ IF TRUE \\* Mutant hook: retire_task() acquires task_lock",
        "  /\\ IF FALSE \\* Mutant hook: retire_task() acquires task_lock",
    ),
    (
        "begin() claims slot before resolve_base()",
        "wl/src/commands.rs begin() resolve_base before claim_slot (lines 194-196)",
        "finding-begin-base-error",
        "NoOrphanClaimWhenIdle",
        "     IN IF TRUE \\* Mutant hook: resolve_base() fails BEFORE calling claim_slot()",
        "     IN IF FALSE \\* Mutant hook: resolve_base() fails BEFORE calling claim_slot()",
    ),
    (
        "reclaim_orphan_claims() skips stale #prepare reclamation",
        "wl/src/commands.rs reclaim_stale_prepare_claims_locked() (lines 61-63, 91-95)",
        "finding-prepare-crash",
        "NoOrphanClaimAfterHousekeeping",
        "         clearPrep == IF TRUE \\* Mutant hook: reclaim stale #prepare claims when prepare_lock is free",
        "         clearPrep == IF FALSE \\* Mutant hook: reclaim stale #prepare claims when prepare_lock is free",
    ),
    (
        "reclaim_orphan_task_claims() skips try_lock_exclusive(task_lock)",
        "wl/src/commands.rs reclaim_orphan_task_claims() (lines 75-79)",
        "two-tasks-concurrent",
        "TaskSlotMatchesClaim",
        "  /\\ LET clearTask == IF TRUE \\* Mutant hook: check task_lock before reclaiming orphan task claim",
        "  /\\ LET clearTask == IF FALSE \\* Mutant hook: check task_lock before reclaiming orphan task claim",
    ),
    (
        "claim_slot picks lowest slot number instead of earliest retirement_order",
        "wl/src/commands.rs claim_slot() (order.is_none(), order, n) sort (lines 111-116)",
        "slot-reuse-order",
        "OldestRetiredSlotReusedFirst",
        "                       THEN IF TRUE \\* Mutant hook: claim_slot() picks earliest retirement_order first",
        "                       THEN IF FALSE \\* Mutant hook: claim_slot() picks earliest retirement_order first",
    ),
    (
        "retire_task skips retirement_lock around retirement_order update",
        "wl/src/commands.rs retire_task() Store::lock_exclusive(&retirement_lock) (line 337)",
        "slot-reuse-order",
        "UniqueRetirementOrder",
        "  /\\ IF TRUE \\* Mutant hook: retire_task() acquires retirement_lock",
        "  /\\ IF FALSE \\* Mutant hook: retire_task() acquires retirement_lock",
    ),
]


def fail(report):
    print(json.dumps(report))
    sys.exit(2)


def run_tlc(java: str, jar: str, directory: Path, config: str) -> dict:
    with tempfile.TemporaryDirectory() as temp:
        proc = subprocess.run(
            [
                java,
                "-XX:+UseParallelGC",
                "-Djava.io.tmpdir=" + temp,
                "-cp",
                jar,
                "tlc2.TLC",
                "-workers",
                "1",
                "-deadlock",
                "-metadir",
                str(Path(temp) / "meta"),
                "-config",
                config + ".cfg",
                SPEC,
            ],
            cwd=directory,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
    out = proc.stdout + proc.stderr
    if TLC_VERSION not in out:
        fail({"error": "expected " + TLC_VERSION, "output": out[-2000:]})
    violated = re.search(r"Invariant (\w+) is violated", out)
    if violated:
        steps = re.findall(r"^State \d+: <(\w+)", out, re.M)
        return {"violated": violated.group(1), "path": steps}
    if "Model checking completed. No error has been found." in out:
        states = re.search(r"states generated, (\d+) distinct states found", out)
        return {"violated": None, "distinct_states": int(states.group(1)) if states else None}
    fail({"error": "TLC did not finish", "config": config, "output": out[-3000:]})


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--java", default=os.environ.get("JAVA") or shutil.which("java"))
    parser.add_argument("--tla2tools", default=os.environ.get("TLA2TOOLS"))
    parser.add_argument("--mutants", action="store_true", help="also verify each mutant breaks its invariant")
    args = parser.parse_args()
    if not args.java or not args.tla2tools or not Path(args.tla2tools).is_file():
        print(json.dumps({"error": "pass --java and --tla2tools, or set JAVA and TLA2TOOLS"}))
        return 2

    ok = True
    for config, expected in EXPECTED.items():
        result = run_tlc(args.java, args.tla2tools, MODEL, config)
        match = result["violated"] == expected
        ok &= match
        print(json.dumps({"config": config, "expected": expected, "match": match, **result}))

    if args.mutants:
        for name, code, config, rule, old, new in MUTANTS:
            with tempfile.TemporaryDirectory() as temp:
                copy = Path(temp)
                for path in MODEL.iterdir():
                    if path.is_file():
                        shutil.copy(path, copy)
                spec = copy / SPEC
                text = spec.read_text(encoding="utf-8")
                if text.count(old) != 1:
                    print(json.dumps({"mutant": name, "error": "model text not found once; update MUTANTS"}))
                    ok = False
                    continue
                spec.write_bytes(text.replace(old, new, 1).encode("utf-8"))
                result = run_tlc(args.java, args.tla2tools, copy, config)
            caught = result["violated"] == rule
            ok &= caught
            print(json.dumps({"mutant": name, "code": code, "config": config, "caught": caught, **result}))

    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
