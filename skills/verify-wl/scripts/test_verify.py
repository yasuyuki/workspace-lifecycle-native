"""Challenge tests for skills/verify-wl/scripts/verify.py using disposable copies."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[3]
HELPER = ROOT / "skills" / "verify-wl" / "scripts" / "verify.py"


class VerifyWlSkillTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.base = Path(temp.name)
        self.evidence = self.base / "evidence"

    def run_helper(self, repo: Path, *extra: str):
        proc = subprocess.run(
            [sys.executable, str(HELPER), "--repo", str(repo), "--evidence-root", str(self.evidence), *extra],
            capture_output=True,
            text=True,
            encoding="utf-8",
            env=os.environ.copy(),
        )
        info = json.loads(proc.stdout)
        result_path = Path(info["result"])
        return proc.returncode, json.loads(result_path.read_text(encoding="utf-8")), result_path.parent

    def assert_cleaned(self, result: dict, evidence: Path):
        self.assertTrue(result["cleanup"]["ok"], result["cleanup"])
        self.assertFalse(Path(result["cleanup"]["scratch"]).exists())
        self.assertTrue(result["evidence_retained"])
        for item in result["artifacts"]:
            self.assertTrue((evidence / item).exists(), item)

    def test_checkout_passes_and_retains_evidence(self):
        code, result, evidence = self.run_helper(ROOT, "--agent", "test_verify")
        self.assertEqual((code, result["status"]), (0, "pass"), result.get("error"))
        self.assertTrue(all(a["ok"] for a in result["assertions"]))
        self.assertIn("preflight-log.txt", result["artifacts"])
        self.assertIn("validation-ran.txt", result["artifacts"])
        self.assert_cleaned(result, evidence)

    def test_faulty_copy_skipping_clear_ignored_fails_even_with_exit_0(self):
        copy = self.base / "faulty"
        shutil.copytree(ROOT, copy, ignore=shutil.ignore_patterns(".git", "target"))
        commands_rs = copy / "wl" / "src" / "commands.rs"
        text = commands_rs.read_text(encoding="utf-8")
        needle = "        clear_ignored(&worktree)?;"
        self.assertEqual(text.count(needle), 1)
        commands_rs.write_text(text.replace(needle, "        let _ = &worktree;"), encoding="utf-8")

        code, result, evidence = self.run_helper(copy, "--agent", "test_faulty")
        self.assertEqual((code, result["status"], result["target"]["revision"]), (1, "fail", "unknown"))
        failed = [a["name"] for a in result["assertions"] if not a["ok"]]
        self.assertEqual(failed, ["undeclared scratch_drop removed"])
        self.assert_cleaned(result, evidence)

    def test_wrong_checkout_is_blocked_and_doctor_only_is_not_run(self):
        code, result, _ = self.run_helper(self.base)
        self.assertEqual((code, result["status"], result["feature_status"]), (2, "blocked", "not-run"))
        self.assertEqual(result["commands"], [])

        code, result, _ = self.run_helper(ROOT, "--doctor-only")
        self.assertEqual((code, result["status"], result["feature_status"]), (3, "not-run", "not-run"))
        self.assertIsNone(result["cleanup"]["scratch"])


if __name__ == "__main__":
    unittest.main()
