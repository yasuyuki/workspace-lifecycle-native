"""Model-check FinishIntegration.tla with TLC across passing and known-bug configs.

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
SPEC = "FinishIntegration.tla"
TLC_VERSION = "TLC2 Version 2.19 of 08 August 2024"

# None: every invariant holds. A rule name: TLC reports that invariant violated.
EXPECTED = {
    "single-task-gated": None,
    "single-task-ungated": None,
    "two-tasks-crash-queue": "ExclusiveIntegrationWorktree",
    "two-tasks-accept-race": "ExclusiveIntegrationWorktree",
    "two-tasks-suffix": "QueueEntryMatchesOwner",
}

# (name, code location, config, expected violated rule, original TLA+ text, broken TLA+ text)
MUTANTS = [
    (
        "worker skips topic required checks in push_target",
        "wl/src/finish.rs push_target() wait_for_checks(commit)",
        "single-task-gated",
        "OnlyCheckedWorkReachesTarget",
        '        THEN CASE ciTopic[c] = "success" ->',
        '        THEN CASE TRUE ->',
    ),
    (
        "worker skips target required checks in TargetPushed",
        "wl/src/finish.rs Worker::step() Stage::TargetPushed wait_for_checks(merged)",
        "single-task-gated",
        "IntegratedChecksPassed",
        '     IN IF ~Gated \\/ (mSrc \\in TargetCommits /\\ ciTarget[mSrc] = "success")',
        "     IN IF TRUE",
    ),
    (
        "worker skips validation in integrate-target",
        "wl/src/finish.rs prepare() validation step loop",
        "single-task-ungated",
        "OnlyCheckedWorkReachesTarget",
        "     IN IF wtMergedValid",
        "     IN IF TRUE",
    ),
    (
        "worker skips hold check before target push",
        "wl/src/finish.rs push_target() HoldRecord check",
        "single-task-ungated",
        "NoTargetPushWhileHeld",
        "     IN IF hold[t]",
        "     IN IF FALSE",
    ),
    (
        "worker records TargetPushed without pushing target branch",
        "wl/src/finish.rs push_target() git push",
        "single-task-ungated",
        "IntegratedIsOnRemote",
        "             /\\ tRemoteTip' = merged\n             /\\ tRemoteCommits' = tRemoteCommits \\cup {c}",
        "             /\\ UNCHANGED <<tRemoteTip, tRemoteCommits>>",
    ),
    (
        "retire_task skips uncommitted/untracked check",
        "wl/src/commands.rs retire_task() git status check",
        "single-task-ungated",
        "NoUnpublishedOrDirtyRetire",
        "     IN IF ~hold[t] /\\ ~dirty[t] /\\ cur = accept[t].commit /\\ Published(cur)",
        "     IN IF ~hold[t] /\\ cur = accept[t].commit /\\ Published(cur)",
    ),
    (
        "retire_task skips hold check",
        "wl/src/commands.rs retire_task() HoldRecord check",
        "single-task-ungated",
        "NoUnpublishedOrDirtyRetire",
        "     IN IF ~hold[t] /\\ ~dirty[t] /\\ cur = accept[t].commit /\\ Published(cur)",
        "     IN IF ~dirty[t] /\\ cur = accept[t].commit /\\ Published(cur)",
    ),
    (
        "retire_task skips expected_head and published check",
        "wl/src/commands.rs retire_task() expected_head and is_ancestor check",
        "single-task-ungated",
        "NoUnpublishedOrDirtyRetire",
        "     IN IF ~hold[t] /\\ ~dirty[t] /\\ cur = accept[t].commit /\\ Published(cur)",
        "     IN IF ~hold[t] /\\ ~dirty[t]",
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
                spec.write_bytes(text.replace(old, new).encode("utf-8"))
                result = run_tlc(args.java, args.tla2tools, copy, config)
            caught = result["violated"] == rule
            ok &= caught
            print(json.dumps({"mutant": name, "code": code, "config": config, "caught": caught, **result}))

    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
