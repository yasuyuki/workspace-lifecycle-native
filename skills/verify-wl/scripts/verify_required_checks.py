#!/usr/bin/env python3
"""Exercise required-check gates through a supplied wl binary and a loopback API."""

import argparse
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import tempfile
from threading import Thread
import time

from verify import PREFLIGHT_SCRIPT, sha256_bytes, writable_retry


REPOSITORY = "https://github.com/example/product.git"
CHECK = "build"
APP = "github-actions"
DETAIL_URL = REPOSITORY.removesuffix(".git") + "/actions/runs/1"
POLL_BUDGET = 22


def check_run(run_id, app=APP, conclusion=None):
    return {
        "id": run_id,
        "name": CHECK,
        "app": {"slug": app},
        "status": "completed" if conclusion else "in_progress",
        "conclusion": conclusion,
        "html_url": DETAIL_URL,
    }


def run(args, evidence):
    result = {
        "started_at": datetime.now(timezone.utc).isoformat(),
        "agent": args.agent,
        "platform": platform.platform(),
        "python": platform.python_version(),
        "binary_revision": args.revision,
        "binary_source": args.binary_source,
        "scenario": args.scenario,
        "entry_point": "wl begin, wl finish, wl status",
        "commands": [],
        "api": [],
        "assertions": [],
        "not_run": ["live GitHub", "WSL offload", "Windows toast delivery"],
    }
    record = evidence / "result.json"

    def save():
        record.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")

    def command(argv, cwd, env, *, exit_code=0):
        argv = [str(part) for part in argv]
        proc = subprocess.run(
            argv, cwd=cwd, env=env, capture_output=True, encoding="utf-8",
            errors="replace", timeout=20,
        )
        result["commands"].append({
            "argv": argv, "cwd": str(cwd), "exit": proc.returncode,
            "stdout": proc.stdout, "stderr": proc.stderr,
        })
        if proc.returncode != exit_code:
            raise RuntimeError(f"{argv}: exit {proc.returncode}, expected {exit_code}")
        return proc.stdout.strip()

    def expect(label, observed, expected):
        result["assertions"].append({"name": label, "observed": observed, "expected": expected})
        save()
        if observed != expected:
            raise AssertionError(label)

    def until(observe, label, seconds=POLL_BUDGET):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            value = observe()
            if value:
                return value
            time.sleep(0.25)
        raise AssertionError(f"{label} did not occur within {seconds} seconds")

    replies = {}
    fallback = []
    draining = False
    api = None
    server_thread = None
    scratch = None
    finish_started = False

    class Api(BaseHTTPRequestHandler):
        def do_GET(self):
            match = re.fullmatch(
                r"/repos/example/product/commits/([0-9a-f]{40})/check-(runs|suites)(?:\?per_page=100)?",
                self.path,
            )
            authed = self.headers.get("Authorization") == "Bearer wl-verifier-synthetic"
            sha, route = match.groups() if match else (None, None)
            if not authed or not match:
                code, body = 400, {}
            elif route == "suites":
                code, body = 200, {"check_suites": [{
                    "id": 1, "status": "completed", "conclusion": "failure",
                    "app": {"slug": APP}, "latest_check_runs_count": 0, "url": DETAIL_URL,
                }]}
            else:
                runs = [check_run(1, conclusion="failure")] if draining else replies.get(sha, fallback)
                code, body = 200, {"check_runs": runs}
            result["api"].append({"sha": sha, "route": route, "status": code, "body": body})
            data = json.dumps(body).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def log_message(self, *_):
            pass

    try:
        result["status"] = "blocked"
        if not args.wl.is_file() or not all(shutil.which(name) for name in ("git", "gh")):
            raise ValueError("wl binary, Git and GitHub CLI are required")
        if not re.fullmatch(r"[0-9a-f]{40}", args.revision):
            raise ValueError("--revision must identify the binary's full Git commit")
        result["binary_sha256"] = sha256_bytes(args.wl.read_bytes())
        inputs = [Path(__file__), Path(__file__).with_name("verify.py")]
        result["verifier_inputs_sha256"] = {path.name: sha256_bytes(path.read_bytes()) for path in inputs}
        result["verifier_revision"] = command(["git", "rev-parse", "HEAD"], Path(__file__).parent, None)
        result["verifier_dirty"] = bool(command(["git", "status", "--porcelain"], Path(__file__).parent, None))
        result["git"] = command(["git", "--version"], evidence, None)
        result["gh"] = command(["gh", "--version"], evidence, None)
        scratch = Path(tempfile.mkdtemp(prefix="wl-checks-scratch-", dir=evidence.parent)).resolve()
        if scratch.parent != evidence.parent or evidence.is_relative_to(scratch):
            raise ValueError("scratch must be a sibling of evidence")
        home = scratch / "home"
        home.mkdir()
        api = ThreadingHTTPServer(("127.0.0.1", 0), Api)
        server_thread = Thread(target=api.serve_forever, daemon=True)
        server_thread.start()
        env = {key: value for key, value in os.environ.items()
               if not key.upper().startswith(("WL_", "GIT_", "GH_", "GITHUB_"))}
        env.update({
            "HOME": str(home), "USERPROFILE": str(home), "LOCALAPPDATA": str(home),
            "XDG_CONFIG_HOME": str(home), "GH_CONFIG_DIR": str(home),
            "GH_TOKEN": "wl-verifier-synthetic", "GH_PROMPT_DISABLED": "1",
            "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_ALLOW_PROTOCOL": "file", "GIT_TERMINAL_PROMPT": "0",
            "GIT_AUTHOR_NAME": "Verifier", "GIT_AUTHOR_EMAIL": "verify@example.invalid",
            "GIT_COMMITTER_NAME": "Verifier", "GIT_COMMITTER_EMAIL": "verify@example.invalid",
            "WL_CONFIG": str(scratch / "config.json"),
            "WL_GITHUB_API": f"http://127.0.0.1:{api.server_port}", "NO_PROXY": "127.0.0.1",
        })
        seed, origin, product = (scratch / name for name in ("seed", "origin.git", "product"))
        seed.mkdir()
        preflight = scratch / "preflight.py"
        preflight_log = evidence / "preflight.jsonl"
        preflight.write_text(PREFLIGHT_SCRIPT, encoding="utf-8")
        Path(env["WL_CONFIG"]).write_text(json.dumps({
            "preflight": [sys.executable, str(preflight), "{repo}", str(preflight_log)],
        }), encoding="utf-8")

        def git(cwd, *argv):
            return command(["git", *argv], cwd, env)

        def wl(*argv, exit_code=0):
            return json.loads(command([args.wl, "--repo", product, *argv], scratch, env, exit_code=exit_code))

        def status():
            return wl("status", "alpha")

        def finish_status():
            return status()["tasks"][0]["finish"]

        def origin_ref(branch):
            return git(origin, "rev-parse", f"refs/heads/{branch}")

        def saw(sha, body):
            return any(event["sha"] == sha and event["status"] == 200 and event["body"] == body
                       for event in result["api"])

        def commit_file(cwd, name, contents):
            path = cwd / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(contents, encoding="utf-8")
            git(cwd, "add", name)
            git(cwd, "commit", "-qm", name)
            return git(cwd, "rev-parse", "HEAD")

        git(seed, "init", "-qb", "main")
        commit_file(seed, "base.txt", "base\n")
        git(scratch, "clone", "-q", "--bare", seed.as_uri(), origin)
        git(scratch, "clone", "-q", origin.as_uri(), product)
        git(product, "remote", "set-url", "origin", REPOSITORY)
        git(product, "config", f"url.{origin.as_uri()}.insteadOf", REPOSITORY)
        git(product, "ls-remote", "--exit-code", "origin", "refs/heads/main")
        if args.doctor_only:
            result["status"] = "not-run"
            return result

        result["status"] = "fail"
        result["expectations"] = (
            ["Completed failed suite without check runs reaches needs-action at the next poll",
             "Failure detail names the run URL", "Origin main stays at its original commit"]
            if args.scenario == "failed-suite" else
            ["Wrong-app success and superseded success leave the topic prepared",
             "A failed required topic run leaves origin main unchanged and names the run URL",
             "A corrected topic gets a new attempt and is checked on its own SHA",
             "The distinct pushed merge SHA needs its own successful check before integration"]
        )
        save()
        task = wl("begin", "alpha", "--request", "https://example.invalid/work/1")
        expect("begin is ready", task["stage"], "ready")
        worktree = Path(task["worktree"])
        topic = commit_file(worktree, ".github/required-checks.json", json.dumps([{"app": APP, "name": CHECK}]))
        base = commit_file(seed, "main.txt", "target advanced\n")
        git(seed, "push", "-q", origin, "main")
        if args.scenario != "failed-suite":
            latest = "success" if args.scenario == "unexpected-success" else None
            replies[topic] = [check_run(1, conclusion="success"), check_run(2, conclusion=latest),
                              check_run(3, "other-app", "success")]
        finish_started = True
        expect("first attempt accepted the topic SHA", wl("finish", "alpha")["commit"], topic)
        until(lambda: saw(topic, {"check_runs": replies.get(topic, [])}), "topic check poll")
        expect("topic pushed", origin_ref("alpha"), topic)
        expect("main unchanged at first topic poll", origin_ref("main"), base)

        if args.scenario == "failed-suite":
            until(lambda: finish_status()["stage"] == "needs-action"
                  or sum(event["route"] == "runs" and event["sha"] == topic for event in result["api"]) >= 2,
                  "the next poll after the failed workflow")
            observed = finish_status()
            expect("failed workflow leaves main unchanged", origin_ref("main"), base)
            if observed["stage"] == "prepared":
                corrected = commit_file(worktree, "fixed.txt", "corrected\n")
                refused = wl("finish", "alpha", exit_code=2)
                expect("the corrected SHA is blocked by the running attempt",
                       corrected != topic and "still running" in refused["error"], True)
            expect("failed workflow leaves needs-action at the next poll", observed["stage"], "needs-action")
            expect("failed workflow detail identifies the run", DETAIL_URL in observed["detail"], True)
        else:
            until(lambda: sum(event["sha"] == topic for event in result["api"]) >= 2
                  or finish_status()["stage"] != "prepared", "a second poll or a stage change")
            expect("latest pending check holds the worker", finish_status()["stage"], "prepared")
            expect("pending topic leaves main unchanged", origin_ref("main"), base)
            replies[topic] = [check_run(4, conclusion="failure")]
            failed = until(lambda: (state if (state := finish_status())["stage"] == "needs-action" else None),
                           "failed check reaches needs-action")
            expect("failed check names the run URL", DETAIL_URL in failed["detail"], True)
            expect("failed topic leaves main unchanged", origin_ref("main"), base)
            until(lambda: not status()["background"], "first worker exits")
            fixed = commit_file(worktree, "fixed.txt", "corrected\n")
            replies[fixed] = [check_run(1, conclusion="success")]
            accepted = wl("finish", "alpha")
            expect("second attempt", accepted["attempt"], 2)
            expect("corrected SHA accepted", accepted["commit"], fixed)
            until(lambda: saw(fixed, {"check_runs": replies[fixed]}), "corrected topic check poll")
            merged = until(lambda: tip if (tip := origin_ref("main")) != base else None, "target push")
            expect("pushed target differs from topic", merged != fixed, True)
            until(lambda: saw(merged, {"check_runs": []}), "pushed target check poll")
            expect("integration waits on the pushed SHA", finish_status()["stage"], "target-pushed")
            expect("pushed target includes the corrected topic", git(origin, "merge-base", merged, fixed), fixed)
            replies[merged] = [check_run(1, conclusion="success")]
            until(lambda: finish_status()["stage"] == "integrated", "finish integrates")
            expect("main still points to the checked merge SHA", origin_ref("main"), merged)
        result["status"] = "pass"
    except (OSError, ValueError, RuntimeError, AssertionError, KeyError, subprocess.TimeoutExpired) as error:
        result["error"] = str(error)
    finally:
        result["feature_status"] = result["status"]
        cleanup = result["cleanup"] = {"scratch": str(scratch) if scratch else None, "ok": False}
        try:
            if finish_started:
                draining = True
                def workers_exited():
                    state = status()
                    progress = state["tasks"][0].get("finish")
                    terminal = not progress or progress["stage"] in {"needs-action", "integrated"}
                    return terminal and not state["background"]

                until(workers_exited, "owned workers exit", seconds=40)
            if scratch and scratch.parent == evidence.parent:
                shutil.rmtree(scratch, onerror=writable_retry)
            cleanup["ok"] = scratch is None or not scratch.exists()
        except (OSError, RuntimeError, AssertionError, subprocess.TimeoutExpired) as error:
            cleanup["error"] = str(error)
        finally:
            if server_thread:
                api.shutdown()
                server_thread.join()
            if api:
                api.server_close()
        result["evidence_retained"] = record.parent.is_dir()
        if not cleanup["ok"] or not result["evidence_retained"]:
            result["status"] = "fail"
        save()
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--wl", required=True, type=lambda name: Path(name).resolve())
    parser.add_argument("--revision", required=True)
    parser.add_argument("--binary-source", required=True)
    parser.add_argument("--evidence-root", required=True, type=Path)
    parser.add_argument("--agent", required=True)
    parser.add_argument("--scenario", choices=["gate", "failed-suite", "unexpected-success"], default="gate")
    parser.add_argument("--doctor-only", action="store_true")
    args = parser.parse_args()
    args.evidence_root.mkdir(parents=True, exist_ok=True)
    evidence = Path(tempfile.mkdtemp(prefix="required-checks-", dir=args.evidence_root)).resolve()
    result = run(args, evidence)
    if json.loads((evidence / "result.json").read_text(encoding="utf-8"))["status"] != result["status"]:
        raise RuntimeError("saved result disagrees with the completed run")
    print(json.dumps({"status": result["status"], "error": result.get("error"), "evidence": str(evidence / "result.json")}))
    return {"pass": 0, "fail": 1, "blocked": 2, "not-run": 3}[result["status"]]


if __name__ == "__main__":
    sys.exit(main())
