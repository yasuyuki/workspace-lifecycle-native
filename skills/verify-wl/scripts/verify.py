#!/usr/bin/env python3
"""Build `wl` from a workspace-lifecycle-native checkout into scratch, drive its
task lifecycle with validation and slot_cache retention, retain evidence, and
clean up owned scratch state."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import stat
import subprocess
import sys
import tempfile
import time


PREFLIGHT_SCRIPT = """import json, pathlib, subprocess, sys
repo, log_file = sys.argv[1], pathlib.Path(sys.argv[2])
def git(*a):
    r = subprocess.run(["git", "-C", repo, *a], capture_output=True, text=True, encoding="utf-8", errors="replace")
    return r.stdout.strip() if r.returncode == 0 else None
branch = git("symbolic-ref", "--short", "HEAD")
merge = git("config", f"branch.{branch}.merge")
dest = merge[len("refs/heads/"):] if merge and merge.startswith("refs/heads/") else branch
with log_file.open("a", encoding="utf-8") as fh:
    fh.write(json.dumps({"repo": repo, "branch": branch, "destination": dest}) + "\\n")
print(json.dumps({"decision": "push", "reason": "VERIFY", "remote": "origin", "destination": dest}))
"""

VALIDATOR_SCRIPT = """import json, pathlib, subprocess, sys
out_file = pathlib.Path(sys.argv[1])
head = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True, encoding="utf-8").strip()
work_ok = pathlib.Path("work.txt").read_text(encoding="utf-8") == "verified\\n"
if not work_ok:
    sys.stderr.write("missing or wrong work.txt\\n")
    sys.exit(1)
with out_file.open("a", encoding="utf-8") as fh:
    fh.write(json.dumps({"cwd": str(pathlib.Path.cwd()), "commit": head}) + "\\n")
"""

EXPECTATIONS = {
    "build": "Checkout copies to scratch and `cargo build --locked -p wl` builds `wl` binary whose `help` lists begin, status, hold, release-hold, finish, retire, preflight.",
    "preflight": "`wl preflight product` on the local origin exits 11 with decision=ask, `--user-intent push` exits 0 with decision=push, `--user-intent hold` exits 10 with decision=hold, and none of them pushes.",
    "begin": "`wl begin alpha` exits 0 with ok=true, stage=ready, branch=alpha, worktree under product.slots, main checkout stays on main.",
    "hold-and-refuse": "`wl hold alpha` records hold, `wl finish alpha` while held exits 2 with error mentioning held and leaves origin/main unchanged, `wl release-hold` clears hold.",
    "finish-retire": "`wl finish alpha --retire` from inside the slot exits 0 with accepted=true, non-empty id, retire=true, attempt=1, commit=HEAD, target=main.",
    "integrated-and-retired": "Worker validates HEAD in integrate-main, runs preflight, pushes alpha and main, delivers one retired notice, deletes local branch alpha, keeps slot_cache `cache_keep/keep.txt`, and deletes undeclared `scratch_drop`.",
    "post-retire-finish-status": "After notice drain, `wl status alpha` exits 2 (ok=false), while `wl status --finish <id>` and `wl status alpha --finish <id>` report stage=retired on HEAD with detail=null; wrong-task and unknown `--finish` exit 2.",
    "slot-reuse-and-name-isolation": "`wl begin beta` reuses alpha's slot path with cache_keep intact; with a commit off main, plain `wl retire beta` and `--abandon` before `git push origin HEAD:beta` exit 2, and `wl retire beta --abandon --decision` after the push succeeds with abandoned=true, keeping origin/beta and leaving origin/main; and re-beginning and retiring alpha starts with finish=null while keeping original `status --finish <id>` intact.",
}

NOT_RUN = [
    "WSL offload (where = 'any')",
    "GitHub required checks polling and gh auth token",
    "merge conflict and needs-action fix-commit recovery",
    "non-default --target, --base, and --branch",
    "Windows toast notification",
    "release-mode latency p90 benchmark",
    "concurrent multi-task finish and worker crash recovery",
    "slot/claim concurrency and orphan/#prepare claim recovery",
    "wl finish without --retire (Integrated stage and uncommitted notice)",
    "other OS than the one running verify.py",
]

SKIP = shutil.ignore_patterns(".git", "target")
DEADLINE = 60


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def writable_retry(function, path, _):
    os.chmod(path, stat.S_IWRITE)
    function(path)


def run_cmd(argv, cwd, env=None):
    return subprocess.run(
        argv,
        cwd=cwd,
        env=env,
        text=True,
        encoding="utf-8",
        errors="replace",
        capture_output=True,
    )


def doctor(repo: Path) -> dict:
    wl_toml = repo / "wl" / "Cargo.toml"
    root_toml = repo / "Cargo.toml"
    lock_file = repo / "Cargo.lock"
    if not wl_toml.is_file() or not root_toml.is_file() or 'name = "wl"' not in wl_toml.read_text(encoding="utf-8"):
        raise ValueError("--repo is not a workspace-lifecycle-native checkout: " + str(repo))
    for tool in ("cargo", "git"):
        if shutil.which(tool) is None:
            raise ValueError(f"{tool} is required on PATH")
    revision = run_cmd(["git", "rev-parse", "HEAD"], repo)
    status = run_cmd(["git", "status", "--porcelain", "--untracked-files=normal"], repo)
    git_ok = revision.returncode == 0 and status.returncode == 0
    inputs = [root_toml, lock_file, wl_toml, *sorted((repo / "wl" / "src").rglob("*.rs"))]
    inputs = [p for p in inputs if p.is_file()]
    return {
        "revision": revision.stdout.strip() if git_ok else "unknown",
        "dirty": bool(status.stdout.strip()) if git_ok else "unknown",
        "inputs_sha256": {p.relative_to(repo).as_posix(): sha256_bytes(p.read_bytes()) for p in inputs},
        "helper_sha256": sha256_bytes(Path(__file__).read_bytes()),
        "git": run_cmd(["git", "--version"], repo).stdout.strip(),
        "cargo": run_cmd(["cargo", "--version"], repo).stdout.strip(),
        "rustc": run_cmd(["rustc", "--version"], repo).stdout.strip() if shutil.which("rustc") else "unknown",
        "python": platform.python_version(),
        "platform": platform.platform(),
        "entry_point": "built `wl` executable: begin, hold, release-hold, finish --retire, status [TASK] [--finish ID], retire",
    }


def run_verify(repo: Path, evidence_root: Path | None = None, doctor_only: bool = False, agent: str = "unknown"):
    repo = Path(repo).resolve()
    if evidence_root is not None:
        evidence_root = Path(evidence_root).resolve()
        evidence_root.mkdir(parents=True, exist_ok=True)
    evidence = Path(tempfile.mkdtemp(prefix="wl-verify-evidence-", dir=evidence_root))
    result = {
        "run_id": evidence.name,
        "started_at": datetime.now(timezone.utc).isoformat(),
        "status": "blocked",
        "feature": "task-finish-lifecycle",
        "feature_status": "not-run",
        "agent": agent,
        "expectations": EXPECTATIONS,
        "commands": [],
        "assertions": [],
        "artifacts": [],
        "not_run": NOT_RUN,
        "cleanup": {"ok": True, "scratch": None},
    }
    scratch = None
    product = None
    wl_bin = None
    drive_env = None

    def save_result():
        (evidence / "result.json").write_text(
            json.dumps(result, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
        )

    def assertion(name: str, expected, observed):
        ok = observed == expected
        result["assertions"].append({"name": name, "expected": expected, "observed": observed, "ok": ok})
        save_result()
        if not ok:
            raise AssertionError(f"{name}: expected {expected!r}, got {observed!r}")

    def execute(label: str, argv, cwd: Path, env=None):
        proc = run_cmd(argv, cwd, env=env)
        result["commands"].append(
            {
                "label": label,
                "argv": [str(a) for a in argv],
                "cwd": str(cwd),
                "exit_code": proc.returncode,
                "stdout": proc.stdout,
                "stderr": proc.stderr,
            }
        )
        save_result()
        return proc

    def keep(src: Path, dest_name: str | None = None):
        name = dest_name or src.name
        shutil.copy2(src, evidence / name)
        result["artifacts"].append(name)
        save_result()

    save_result()
    try:
        result["target"] = doctor(repo)
        if doctor_only:
            result["status"] = "not-run"
            return result, evidence

        scratch = Path(tempfile.mkdtemp(prefix="wl-verify-drive-")).resolve()
        result["cleanup"]["scratch"] = str(scratch)
        source = scratch / "source"
        target_dir = scratch / "target"
        shutil.copytree(repo, source, ignore=SKIP)

        build_cmd = [
            "cargo",
            "build",
            "--locked",
            "--offline",
            "-p",
            "wl",
            "--manifest-path",
            str(source / "Cargo.toml"),
            "--target-dir",
            str(target_dir),
        ]
        built = execute("cargo build offline", build_cmd, source)
        if built.returncode != 0:
            online_cmd = [a for a in build_cmd if a != "--offline"]
            built = execute("cargo build locked", online_cmd, source)
        if built.returncode != 0:
            raise ValueError("cargo build failed: " + built.stderr[-1500:])

        exe_suffix = ".exe" if os.name == "nt" else ""
        wl_bin = target_dir / "debug" / f"wl{exe_suffix}"
        result["status"] = result["feature_status"] = "fail"
        assertion("built wl executable exists", True, wl_bin.is_file())

        home = scratch / "home"
        localappdata = scratch / "localappdata"
        home.mkdir(parents=True, exist_ok=True)
        localappdata.mkdir(parents=True, exist_ok=True)
        empty_gitconfig = home / ".gitconfig"
        empty_gitconfig.write_text("", encoding="utf-8")

        preflight_py = scratch / "preflight.py"
        preflight_py.write_text(PREFLIGHT_SCRIPT, encoding="utf-8")
        keep(preflight_py)
        preflight_log = scratch / "preflight-log.txt"

        validator_py = scratch / "validator.py"
        validator_py.write_text(VALIDATOR_SCRIPT, encoding="utf-8")
        validation_log = scratch / "validation-ran.txt"

        wl_config = scratch / "wl-config.json"
        wl_config.write_text(
            json.dumps(
                {"preflight": [sys.executable, str(preflight_py), "{repo}", str(preflight_log)]}
            ),
            encoding="utf-8",
        )

        drive_env = {
            k: v
            for k, v in os.environ.items()
            if not k.upper().startswith("WL_") and not k.upper().startswith("GIT_")
        }
        drive_env.update(
            {
                "HOME": str(home),
                "USERPROFILE": str(home),
                "LOCALAPPDATA": str(localappdata),
                "XDG_CONFIG_HOME": str(home / ".config"),
                "GIT_CONFIG_GLOBAL": str(empty_gitconfig),
                "WL_CONFIG": str(wl_config),
                "GIT_AUTHOR_NAME": "verify",
                "GIT_AUTHOR_EMAIL": "verify@example.invalid",
                "GIT_COMMITTER_NAME": "verify",
                "GIT_COMMITTER_EMAIL": "verify@example.invalid",
                "PYTHONIOENCODING": "utf-8",
            }
        )

        help_proc = execute("wl help", [str(wl_bin), "help"], scratch, env=drive_env)
        assertion("wl help exit 0", 0, help_proc.returncode)
        for sub in ("begin", "status", "hold", "release-hold", "finish", "retire", "preflight"):
            assertion(f"help mentions {sub}", True, sub in help_proc.stdout)

        seed = scratch / "seed"
        origin = scratch / "origin.git"
        product = scratch / "product"
        seed.mkdir()
        lifecycle_toml = (
            'slot_cache = ["cache_keep"]\n\n'
            "[[validation]]\n"
            f"argv = [{json.dumps(sys.executable)}, {json.dumps(str(validator_py))}, {json.dumps(str(validation_log))}]\n"
            'where = "windows"\n'
        )
        for label, argv, cwd in (
            ("init seed", ["git", "init", "-q", "-b", "main"], seed),
        ):
            if execute(label, argv, cwd, env=drive_env).returncode != 0:
                raise ValueError(label + " failed")
        (seed / ".gitignore").write_text("cache_keep/\nscratch_drop/\n", encoding="utf-8")
        (seed / ".lifecycle.toml").write_text(lifecycle_toml, encoding="utf-8")
        (seed / "base.txt").write_text("base\n", encoding="utf-8")
        for label, argv, cwd in (
            ("add seed", ["git", "add", "-A"], seed),
            ("commit seed", ["git", "commit", "-q", "-m", "seed"], seed),
            ("clone bare origin", ["git", "clone", "-q", "--bare", str(seed), str(origin)], scratch),
            ("clone product", ["git", "clone", "-q", str(origin), str(product)], scratch),
        ):
            if execute(label, argv, cwd, env=drive_env).returncode != 0:
                raise ValueError(label + " failed")

        base_sha = run_cmd(["git", "rev-parse", "main"], origin, env=drive_env).stdout.strip()

        # The push decision on a local origin: no hosting metadata, so the default decision asks.
        for label, extra, code, decision in (
            ("wl preflight", [], 11, "ask"),
            ("wl preflight --user-intent push", ["--user-intent", "push"], 0, "push"),
            ("wl preflight --user-intent hold", ["--user-intent", "hold"], 10, "hold"),
        ):
            proc = execute(label, [str(wl_bin), "preflight", str(product), *extra], product, env=drive_env)
            assertion(f"{label} exit {code}", code, proc.returncode)
            assertion(f"{label} decision", decision, json.loads(proc.stdout).get("decision"))
        assertion("preflight pushed nothing", base_sha, run_cmd(["git", "rev-parse", "main"], origin, env=drive_env).stdout.strip())

        begin_proc = execute(
            "wl begin alpha",
            [str(wl_bin), "--repo", str(product), "begin", "alpha", "--request", "verify/lifecycle"],
            product,
            env=drive_env,
        )
        assertion("begin alpha exit 0", 0, begin_proc.returncode)
        begin_json = json.loads(begin_proc.stdout)
        assertion("begin alpha ok", True, begin_json.get("ok"))
        assertion("begin alpha stage ready", "ready", begin_json.get("stage"))
        assertion("begin alpha branch", "alpha", begin_json.get("branch"))
        worktree = Path(begin_json["worktree"]).resolve()
        assertion("slot created under product.slots", "product.slots", worktree.parent.name)
        assertion("slot worktree exists", True, worktree.is_dir())
        assertion(
            "slot branch is alpha",
            "alpha",
            run_cmd(["git", "symbolic-ref", "--short", "HEAD"], worktree, env=drive_env).stdout.strip(),
        )
        assertion(
            "product stays on main",
            "main",
            run_cmd(["git", "symbolic-ref", "--short", "HEAD"], product, env=drive_env).stdout.strip(),
        )

        hold_proc = execute(
            "wl hold alpha",
            [str(wl_bin), "--repo", str(product), "hold", "alpha", "--reason", "verify hold"],
            product,
            env=drive_env,
        )
        assertion("hold alpha exit 0", 0, hold_proc.returncode)
        assertion("hold alpha json", "verify hold", json.loads(hold_proc.stdout).get("hold"))
        status_held = json.loads(
            execute(
                "status while held",
                [str(wl_bin), "--repo", str(product), "status", "alpha"],
                product,
                env=drive_env,
            ).stdout
        )
        assertion("status reports hold", "verify hold", status_held["tasks"][0].get("hold"))

        held_finish = execute(
            "finish while held",
            [str(wl_bin), "--repo", str(product), "finish", "alpha"],
            product,
            env=drive_env,
        )
        assertion("held finish exit 2", 2, held_finish.returncode)
        held_err = json.loads(held_finish.stdout)
        assertion("held finish ok false", False, held_err.get("ok"))
        assertion("held finish names hold", True, "held" in held_err.get("error", ""))
        assertion(
            "origin/main unchanged while held",
            base_sha,
            run_cmd(["git", "rev-parse", "main"], origin, env=drive_env).stdout.strip(),
        )

        release_proc = execute(
            "wl release-hold alpha",
            [str(wl_bin), "--repo", str(product), "release-hold", "alpha", "--decision", "verify/release"],
            product,
            env=drive_env,
        )
        assertion("release-hold exit 0", 0, release_proc.returncode)
        assertion("release-hold reason", "verify hold", json.loads(release_proc.stdout).get("released"))
        status_unheld = json.loads(
            execute(
                "status after release-hold",
                [str(wl_bin), "--repo", str(product), "status", "alpha"],
                product,
                env=drive_env,
            ).stdout
        )
        assertion("hold cleared in status", None, status_unheld["tasks"][0].get("hold"))

        (worktree / "work.txt").write_text("verified\n", encoding="utf-8")
        for label, argv in (
            ("stage alpha work", ["git", "add", "work.txt"]),
            ("commit alpha work", ["git", "commit", "-q", "-m", "alpha work"]),
        ):
            if execute(label, argv, worktree, env=drive_env).returncode != 0:
                raise ValueError(label + " failed")
        alpha_head = run_cmd(["git", "rev-parse", "HEAD"], worktree, env=drive_env).stdout.strip()

        (worktree / "cache_keep").mkdir(parents=True, exist_ok=True)
        (worktree / "cache_keep" / "keep.txt").write_text("keep\n", encoding="utf-8")
        (worktree / "scratch_drop").mkdir(parents=True, exist_ok=True)
        (worktree / "scratch_drop" / "drop.txt").write_text("drop\n", encoding="utf-8")

        finish_proc = execute(
            "wl finish alpha --retire",
            [str(wl_bin), "--repo", str(worktree), "finish", "alpha", "--retire"],
            worktree,
            env=drive_env,
        )
        assertion("finish exit 0", 0, finish_proc.returncode)
        finish_json = json.loads(finish_proc.stdout)
        finish_id = finish_json.get("id")
        assertion("finish accepted", True, finish_json.get("accepted"))
        assertion("finish id present", True, isinstance(finish_id, str) and len(finish_id) > 0)
        assertion("finish retire true", True, finish_json.get("retire") is True)
        assertion("finish attempt 1", 1, finish_json.get("attempt"))
        assertion("finish commit matches HEAD", alpha_head, finish_json.get("commit"))
        assertion("finish target main", "main", finish_json.get("target"))

        notices = []
        deadline = time.monotonic() + DEADLINE
        retired_seen = False
        poll_checked = False
        while time.monotonic() < deadline:
            if not poll_checked:
                fin_poll_proc = run_cmd([str(wl_bin), "--repo", str(product), "status", "--finish", finish_id], product, env=drive_env)
                fin_poll = json.loads(fin_poll_proc.stdout)
                assertion(
                    "status --finish tracks running execution",
                    True,
                    fin_poll_proc.returncode == 0
                    and fin_poll.get("ok") is True
                    and fin_poll.get("finish", {}).get("id") == finish_id
                    and fin_poll.get("finish", {}).get("task") == "alpha",
                )
                poll_checked = True
            st = json.loads(
                run_cmd([str(wl_bin), "--repo", str(product), "status"], product, env=drive_env).stdout
            )
            notices.extend(st.get("notices", []))
            alpha_entries = [t for t in st.get("tasks", []) if t.get("task") == "alpha"]
            if alpha_entries and alpha_entries[0].get("finish", {}).get("stage") == "needs-action":
                raise AssertionError(f"alpha entered needs-action: {alpha_entries[0]}")
            if not alpha_entries and any(
                n.get("stage") == "retired" and n.get("task") == "alpha" and n.get("id") == finish_id
                for n in notices
            ):
                retired_seen = True
                break
            time.sleep(0.1)
        assertion("alpha retired and noticed", True, retired_seen)

        after_status_proc = execute(
            "status after retire",
            [str(wl_bin), "--repo", str(product), "status"],
            product,
            env=drive_env,
        )
        after_status = json.loads(after_status_proc.stdout)
        (evidence / "final-status.json").write_text(
            json.dumps(after_status, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
        )
        result["artifacts"].append("final-status.json")
        save_result()

        assertion("notices handed over once", None, after_status.get("notices"))
        assertion("at least one free slot", True, after_status["slots"]["free"] >= 1)

        alpha_after_proc = execute(
            "status alpha after retire",
            [str(wl_bin), "--repo", str(product), "status", "alpha"],
            product,
            env=drive_env,
        )
        assertion("status alpha retired exits 2", 2, alpha_after_proc.returncode)
        assertion("status alpha retired ok false", False, json.loads(alpha_after_proc.stdout).get("ok"))

        fin_status_proc = execute(
            "status --finish after retire",
            [str(wl_bin), "--repo", str(product), "status", "--finish", finish_id],
            product,
            env=drive_env,
        )
        assertion("status --finish exit 0", 0, fin_status_proc.returncode)
        fin_status = json.loads(fin_status_proc.stdout)
        (evidence / "finish-status-after-retire.json").write_text(
            json.dumps(fin_status, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
        )
        result["artifacts"].append("finish-status-after-retire.json")
        save_result()

        fin_obj = fin_status.get("finish", {})
        assertion("status --finish ok", True, fin_status.get("ok"))
        assertion("status --finish id", finish_id, fin_obj.get("id"))
        assertion("status --finish task", "alpha", fin_obj.get("task"))
        assertion("status --finish attempt", 1, fin_obj.get("attempt"))
        assertion("status --finish commit", alpha_head, fin_obj.get("commit"))
        assertion("status --finish merged", alpha_head, fin_obj.get("merged"))
        assertion("status --finish target", "main", fin_obj.get("target"))
        assertion("status --finish retire", True, fin_obj.get("retire"))
        assertion("status --finish reports stage retired", "retired", fin_obj.get("stage"))
        assertion("status --finish detail null", None, fin_obj.get("detail"))

        fin_task_proc = execute(
            "status alpha --finish after retire",
            [str(wl_bin), "--repo", str(product), "status", "alpha", "--finish", finish_id],
            product,
            env=drive_env,
        )
        assertion("status alpha --finish exit 0", 0, fin_task_proc.returncode)
        fin_task_obj = json.loads(fin_task_proc.stdout).get("finish", {})
        assertion("status alpha --finish id", finish_id, fin_task_obj.get("id"))
        assertion("status alpha --finish stage retired", "retired", fin_task_obj.get("stage"))

        fin_mismatch_proc = execute(
            "status beta --finish mismatch",
            [str(wl_bin), "--repo", str(product), "status", "beta", "--finish", finish_id],
            product,
            env=drive_env,
        )
        assertion("status beta --finish mismatch exits 2", 2, fin_mismatch_proc.returncode)
        assertion("status beta --finish mismatch ok false", False, json.loads(fin_mismatch_proc.stdout).get("ok"))

        fin_missing_proc = execute(
            "status --finish missing",
            [str(wl_bin), "--repo", str(product), "status", "--finish", "missing-id"],
            product,
            env=drive_env,
        )
        assertion("status --finish missing exits 2", 2, fin_missing_proc.returncode)
        assertion("status --finish missing ok false", False, json.loads(fin_missing_proc.stdout).get("ok"))

        assertion(
            "origin/main integrated alpha HEAD",
            alpha_head,
            run_cmd(["git", "rev-parse", "main"], origin, env=drive_env).stdout.strip(),
        )
        assertion(
            "origin/alpha pushed alpha HEAD",
            alpha_head,
            run_cmd(["git", "rev-parse", "alpha"], origin, env=drive_env).stdout.strip(),
        )
        assertion("validation ran", True, validation_log.is_file())
        val_records = [json.loads(line) for line in validation_log.read_text(encoding="utf-8").splitlines() if line.strip()]
        assertion("validation ran once on HEAD", [alpha_head], [r["commit"] for r in val_records])
        assertion(
            "validation ran in integrate-main",
            True,
            Path(val_records[0]["cwd"]).name == "integrate-main",
        )
        keep(validation_log)
        assertion("preflight log exists", True, preflight_log.is_file())
        pre_records = [json.loads(line) for line in preflight_log.read_text(encoding="utf-8").splitlines() if line.strip()]
        assertion("preflight ran for alpha and main", ["alpha", "main"], [r["destination"] for r in pre_records])
        keep(preflight_log)
        worker_log = product / ".git" / "wl" / "logs" / "alpha-1.log"
        if worker_log.is_file():
            keep(worker_log)
        assertion(
            "local branch alpha deleted",
            "",
            run_cmd(["git", "branch", "--list", "alpha"], product, env=drive_env).stdout.strip(),
        )
        assertion("slot_cache keep.txt preserved", True, (worktree / "cache_keep" / "keep.txt").is_file())
        assertion("undeclared scratch_drop removed", False, (worktree / "scratch_drop").exists())

        beta_begin = execute(
            "wl begin beta",
            [str(wl_bin), "--repo", str(product), "begin", "beta", "--request", "verify/reuse"],
            product,
            env=drive_env,
        )
        assertion("begin beta exit 0", 0, beta_begin.returncode)
        beta_json = json.loads(beta_begin.stdout)
        assertion("begin beta stage ready", "ready", beta_json.get("stage"))
        assertion("beta reuses alpha slot", str(worktree), str(Path(beta_json["worktree"]).resolve()))
        assertion("slot_cache keep.txt survives reuse", True, (worktree / "cache_keep" / "keep.txt").is_file())

        (worktree / "abandoned.txt").write_text("not integrated\n", encoding="utf-8")
        for argv in (["add", "abandoned.txt"], ["commit", "-q", "-m", "abandoned work"]):
            assertion(f"git {argv[0]} in beta exit 0", 0, run_cmd(["git", *argv], worktree, env=drive_env).returncode)
        beta_tip = run_cmd(["git", "rev-parse", "HEAD"], worktree, env=drive_env).stdout.strip()
        origin_main_before_abandon = run_cmd(["git", "rev-parse", "main"], origin, env=drive_env).stdout.strip()

        beta_plain_retire = execute(
            "wl retire beta with unintegrated commit",
            [str(wl_bin), "--repo", str(product), "retire", "beta"],
            product,
            env=drive_env,
        )
        assertion("retire beta unintegrated exit 2", 2, beta_plain_retire.returncode)
        beta_unpushed = execute(
            "wl retire beta --abandon before topic push",
            [str(wl_bin), "--repo", str(product), "retire", "beta", "--abandon", "--decision", "verify/abandon"],
            product,
            env=drive_env,
        )
        assertion("abandon before topic push exit 2", 2, beta_unpushed.returncode)
        assertion("abandon before topic push names push", True, "push the topic" in json.loads(beta_unpushed.stdout).get("error", ""))
        assertion("refused abandon keeps beta HEAD", beta_tip, run_cmd(["git", "rev-parse", "HEAD"], worktree, env=drive_env).stdout.strip())

        assertion("push beta topic exit 0", 0, run_cmd(["git", "push", "-q", "origin", "HEAD:beta"], worktree, env=drive_env).returncode)
        beta_retire = execute(
            "wl retire beta --abandon",
            [str(wl_bin), "--repo", str(product), "retire", "beta", "--abandon", "--decision", "verify/abandon"],
            product,
            env=drive_env,
        )
        assertion("retire beta --abandon exit 0", 0, beta_retire.returncode)
        beta_retire_json = json.loads(beta_retire.stdout)
        assertion("retire beta retired true", True, beta_retire_json.get("retired"))
        assertion("retire beta abandoned true", True, beta_retire_json.get("abandoned"))
        assertion("retire beta decision", "verify/abandon", beta_retire_json.get("decision"))
        assertion("origin/beta keeps abandoned tip", beta_tip, run_cmd(["git", "rev-parse", "beta"], origin, env=drive_env).stdout.strip())
        assertion("abandon leaves origin/main", origin_main_before_abandon, run_cmd(["git", "rev-parse", "main"], origin, env=drive_env).stdout.strip())

        alpha_rebegin = execute(
            "wl begin alpha rebegin",
            [str(wl_bin), "--repo", str(product), "begin", "alpha", "--request", "verify/rebegin"],
            product,
            env=drive_env,
        )
        assertion("rebegin alpha exit 0", 0, alpha_rebegin.returncode)
        alpha_rebegin_json = json.loads(alpha_rebegin.stdout)
        assertion("rebegin alpha stage ready", "ready", alpha_rebegin_json.get("stage"))
        assertion("rebegin alpha finish null", None, alpha_rebegin_json.get("finish"))

        alpha_rebegin_status = json.loads(
            execute(
                "status alpha after rebegin",
                [str(wl_bin), "--repo", str(product), "status", "alpha"],
                product,
                env=drive_env,
            ).stdout
        )
        assertion("status alpha after rebegin finish null", None, alpha_rebegin_status["tasks"][0].get("finish"))

        alpha_reretire = execute(
            "wl retire alpha rebegin",
            [str(wl_bin), "--repo", str(product), "retire", "alpha"],
            product,
            env=drive_env,
        )
        assertion("retire alpha exit 0", 0, alpha_reretire.returncode)
        assertion("retire alpha retired true", True, json.loads(alpha_reretire.stdout).get("retired"))

        fin_after_rebegin_proc = execute(
            "original status --finish after rebegin",
            [str(wl_bin), "--repo", str(product), "status", "--finish", finish_id],
            product,
            env=drive_env,
        )
        assertion("original status --finish exit 0", 0, fin_after_rebegin_proc.returncode)
        fin_after_rebegin = json.loads(fin_after_rebegin_proc.stdout).get("finish", {})
        assertion("original status --finish still retired", "retired", fin_after_rebegin.get("stage"))
        assertion("original status --finish still alpha HEAD", alpha_head, fin_after_rebegin.get("commit"))

        result["status"] = result["feature_status"] = "pass"
    except (OSError, ValueError, AssertionError, KeyError, IndexError, json.JSONDecodeError, subprocess.TimeoutExpired) as exc:
        result["error"] = f"{type(exc).__name__}: {exc}"
    finally:
        if scratch is not None:
            if wl_bin is not None and product is not None and product.exists() and drive_env is not None:
                bg_deadline = time.monotonic() + 30
                while time.monotonic() < bg_deadline:
                    probe = run_cmd([str(wl_bin), "--repo", str(product), "status"], product, env=drive_env)
                    if probe.returncode == 0:
                        try:
                            if json.loads(probe.stdout).get("background") is False:
                                break
                        except json.JSONDecodeError:
                            pass
                    time.sleep(0.1)
            last_err = None
            for _ in range(20):
                try:
                    shutil.rmtree(scratch, onerror=writable_retry)
                    if not scratch.exists():
                        break
                except OSError as exc:
                    last_err = exc
                    time.sleep(0.15)
            result["cleanup"]["ok"] = not scratch.exists()
            if not result["cleanup"]["ok"] and last_err is not None:
                result["cleanup"]["error"] = str(last_err)
        if not result["cleanup"]["ok"]:
            result["status"] = "fail"
        result["finished_at"] = datetime.now(timezone.utc).isoformat()
        result["evidence_retained"] = all((evidence / item).exists() for item in result["artifacts"])
        if not result["evidence_retained"]:
            result["status"] = "fail"
        save_result()
    return result, evidence


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True, help="workspace-lifecycle-native checkout to build and verify")
    parser.add_argument("--evidence-root", type=Path, help="parent directory for the new evidence directory")
    parser.add_argument("--doctor-only", action="store_true", help="identify inputs only; leave feature not-run")
    parser.add_argument("--agent", default="unknown", help="driving agent recorded in result.json")
    args = parser.parse_args()
    result, evidence = run_verify(args.repo, args.evidence_root, args.doctor_only, args.agent)
    print(json.dumps({"status": result["status"], "result": str(evidence / "result.json")}, indent=2))
    return {"pass": 0, "fail": 1, "blocked": 2, "not-run": 3}[result["status"]]


if __name__ == "__main__":
    sys.exit(main())
