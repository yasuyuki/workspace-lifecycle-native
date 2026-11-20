use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const PYTHON: &str = if cfg!(windows) { "python" } else { "python3" };

/// Decides like the real preflight: the destination is the upstream's branch on origin, else the current branch.
const PREFLIGHT: &str = r#"
import json, subprocess, sys
repo = sys.argv[1]
def git(*a):
    r = subprocess.run(["git", "-C", repo, *a], capture_output=True, text=True)
    return r.stdout.strip() if r.returncode == 0 else None
branch = git("symbolic-ref", "--short", "HEAD")
merge = git("config", f"branch.{branch}.merge")
destination = merge[len("refs/heads/"):] if merge else branch
decision = open(sys.argv[2]).read().strip() if len(sys.argv) > 2 else "push"
print(json.dumps({"decision": decision, "reason": "TEST", "remote": "origin", "destination": destination}))
"#;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").arg("-C").arg(dir).args(args)
        .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t")
        .output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

struct Fixture {
    dir: tempfile::TempDir,
    main: PathBuf,
    origin: PathBuf,
    decision: PathBuf,
}

impl Fixture {
    fn new(lifecycle_toml: Option<&str>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let seed = root.join("seed");
        std::fs::create_dir_all(&seed).unwrap();
        git(&seed, &["init", "-q", "-b", "main"]);
        std::fs::write(seed.join("a.txt"), "a\n").unwrap();
        if let Some(text) = lifecycle_toml {
            std::fs::write(seed.join(".lifecycle.toml"), text).unwrap();
        }
        git(&seed, &["add", "-A"]);
        git(&seed, &["commit", "-q", "-m", "seed"]);
        let origin = root.join("origin.git");
        git(root, &["clone", "-q", "--bare", seed.to_str().unwrap(), origin.to_str().unwrap()]);
        let main = root.join("product");
        git(root, &["clone", "-q", origin.to_str().unwrap(), main.to_str().unwrap()]);
        let script = root.join("preflight.py");
        std::fs::write(&script, PREFLIGHT).unwrap();
        let decision = root.join("decision.txt");
        std::fs::write(&decision, "push").unwrap();
        let config = root.join("config.json");
        let preflight = serde_json::json!({ "preflight": [PYTHON, script, "{repo}", decision] });
        std::fs::write(&config, preflight.to_string()).unwrap();
        Fixture { dir, main, origin, decision }
    }

    fn wl(&self, args: &[&str]) -> Value {
        let home = self.dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_wl")).arg("--repo").arg(&self.main).args(args)
            .env("HOME", &home).env("USERPROFILE", &home).env("XDG_CONFIG_HOME", &home)
            .env("WL_CONFIG", self.dir.path().join("config.json"))
            .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t")
            .output().unwrap();
        serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("not json: {}", String::from_utf8_lossy(&out.stdout)))
    }

    fn begin_with_commit(&self, task: &str, file: &str, content: &str) -> PathBuf {
        let out = self.wl(&["begin", task, "--request", "r"]);
        assert_eq!(out["ok"], true, "{out}");
        let worktree = PathBuf::from(out["worktree"].as_str().unwrap());
        std::fs::create_dir_all(worktree.join(file).parent().unwrap()).unwrap();
        std::fs::write(worktree.join(file), content).unwrap();
        git(&worktree, &["add", file]);
        git(&worktree, &["commit", "-q", "-m", &format!("work of {task}")]);
        worktree
    }

    fn origin_main(&self) -> String {
        git(&self.origin, &["rev-parse", "main"])
    }

    /// Polls status until the task's finish reaches `stage`, collecting every notice handed over meanwhile.
    fn wait_for(&self, task: &str, stage: &str) -> (Value, Vec<Value>) {
        let start = Instant::now();
        let mut notices = Vec::new();
        loop {
            let status = self.wl(&["status"]);
            notices.extend(status["notices"].as_array().cloned().unwrap_or_default());
            let found = status["tasks"].as_array().unwrap().iter().find(|t| t["task"] == task).cloned();
            match &found {
                Some(t) if t["finish"]["stage"] == stage => return (t.clone(), notices),
                None if stage == "retired" => return (Value::Null, notices),
                _ => {}
            }
            assert!(start.elapsed() < Duration::from_secs(90), "{task} did not reach {stage}: {status}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[test]
fn finish_returns_at_once_and_the_worker_fast_forwards_the_target() {
    let f = Fixture::new(None);
    let worktree = f.begin_with_commit("alpha", "b.txt", "b\n");
    let commit = git(&worktree, &["rev-parse", "HEAD"]);
    std::fs::write(worktree.join("a.txt"), "not committed\n").unwrap();
    let out = f.wl(&["finish", "alpha"]);
    assert_eq!(out["ok"], true, "{out}");
    assert_eq!(out["commit"], commit.as_str());
    let (task, notices) = f.wait_for("alpha", "integrated");
    assert_eq!(task["finish"]["uncommitted"], true, "{task}");
    assert!(notices[0]["message"].as_str().unwrap().contains("uncommitted changes"), "{notices:?}");
    assert_eq!(f.origin_main(), commit, "fast-forwarded");
    assert_eq!(git(&f.origin, &["rev-parse", "alpha"]), commit, "the topic was pushed");
    assert_eq!(task["finish"]["merged"], commit.as_str());
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0]["message"].as_str().unwrap().contains("integrated"));
    assert!(f.wl(&["status"]).get("notices").is_none(), "a notice is handed over once");
}

#[test]
fn finish_with_retire_integrates_then_frees_the_slot() {
    let f = Fixture::new(None);
    f.begin_with_commit("alpha", "b.txt", "b\n");
    assert_eq!(f.wl(&["finish", "alpha", "--retire"])["ok"], true);
    let (_, notices) = f.wait_for("alpha", "retired");
    assert!(notices.iter().any(|n| n["stage"] == "retired"), "{notices:?}");
    assert!(!git(&f.main, &["branch", "--list", "alpha"]).contains("alpha"));
    assert!(f.wl(&["status"])["slots"]["free"].as_u64() >= Some(1));
}

#[test]
fn a_task_name_used_again_after_retiring_integrates_its_new_commit() {
    let f = Fixture::new(None);
    f.begin_with_commit("alpha", "b.txt", "b\n");
    assert_eq!(f.wl(&["finish", "alpha", "--retire"])["ok"], true);
    // The retired notice comes after everything the first worker writes.
    let start = Instant::now();
    let (_, mut notices) = f.wait_for("alpha", "retired");
    while !notices.iter().any(|n| n["stage"] == "retired") {
        assert!(start.elapsed() < Duration::from_secs(90), "no retired notice");
        std::thread::sleep(Duration::from_millis(100));
        notices.extend(f.wl(&["status"])["notices"].as_array().cloned().unwrap_or_default());
    }
    let worktree = f.begin_with_commit("alpha", "c.txt", "c\n");
    let commit = git(&worktree, &["rev-parse", "HEAD"]);
    let out = f.wl(&["finish", "alpha"]);
    assert_eq!(out["attempt"], 1, "{out}");
    f.wait_for("alpha", "integrated");
    assert_eq!(f.origin_main(), commit);
}

#[test]
fn a_moved_target_gets_a_merge_commit_with_both_parents() {
    let f = Fixture::new(None);
    let worktree = f.begin_with_commit("alpha", "b.txt", "b\n");
    let pusher = f.dir.path().join("pusher");
    git(f.dir.path(), &["clone", "-q", f.origin.to_str().unwrap(), pusher.to_str().unwrap()]);
    std::fs::write(pusher.join("c.txt"), "c\n").unwrap();
    git(&pusher, &["add", "c.txt"]);
    git(&pusher, &["commit", "-q", "-m", "elsewhere"]);
    git(&pusher, &["push", "-q", "origin", "main"]);
    let other = git(&pusher, &["rev-parse", "HEAD"]);
    let commit = git(&worktree, &["rev-parse", "HEAD"]);

    f.wl(&["finish", "alpha"]);
    f.wait_for("alpha", "integrated");
    let parents = git(&f.origin, &["rev-list", "--parents", "-n", "1", "main"]);
    assert_eq!(parents.split(' ').skip(1).collect::<Vec<_>>(), vec![other.as_str(), commit.as_str()]);
}

#[test]
fn a_conflict_needs_action_leaves_the_target_alone_and_a_fixed_finish_integrates() {
    let f = Fixture::new(None);
    let worktree = f.begin_with_commit("alpha", "a.txt", "mine\n");
    let pusher = f.dir.path().join("pusher");
    git(f.dir.path(), &["clone", "-q", f.origin.to_str().unwrap(), pusher.to_str().unwrap()]);
    std::fs::write(pusher.join("a.txt"), "theirs\n").unwrap();
    git(&pusher, &["commit", "-q", "-am", "conflicting"]);
    git(&pusher, &["push", "-q", "origin", "main"]);
    let before = f.origin_main();

    f.wl(&["finish", "alpha"]);
    let (task, notices) = f.wait_for("alpha", "needs-action");
    let detail = task["finish"]["detail"].as_str().unwrap();
    assert!(detail.contains("does not merge") && detail.contains("a.txt"), "names the conflicting file: {detail}");
    assert!(notices.iter().any(|n| n["stage"] == "needs-action"));
    assert_eq!(f.origin_main(), before);

    git(&worktree, &["fetch", "-q", "origin"]);
    let merge = Command::new("git").arg("-C").arg(&worktree).args(["merge", "-q", "origin/main"])
        .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t")
        .output().unwrap();
    assert!(!merge.status.success() && worktree.join(".git").exists(), "the merge stops at the conflict");
    std::fs::write(worktree.join("a.txt"), "both\n").unwrap();
    git(&worktree, &["commit", "-q", "-am", "resolve"]);
    let out = f.wl(&["finish", "alpha"]);
    assert_eq!(out["attempt"], 2, "{out}");
    f.wait_for("alpha", "integrated");
    assert_eq!(git(&f.origin, &["show", "main:a.txt"]), "both");
}

#[test]
fn a_failing_validation_step_needs_action_and_names_the_step() {
    let toml = format!("[[validation]]\nargv = [\"{PYTHON}\", \"-c\", \"import sys; sys.exit(3)\"]\nwhere = \"any\"\n");
    let f = Fixture::new(Some(&toml));
    f.begin_with_commit("alpha", "b.txt", "b\n");
    let before = f.origin_main();
    f.wl(&["finish", "alpha"]);
    let (task, _) = f.wait_for("alpha", "needs-action");
    let detail = task["finish"]["detail"].as_str().unwrap();
    assert!(detail.contains("validation step 1"), "{detail}");
    assert_eq!(f.origin_main(), before);
}

#[test]
fn a_preflight_that_holds_stops_before_any_push() {
    let f = Fixture::new(None);
    let worktree = f.begin_with_commit("alpha", "b.txt", "b\n");
    std::fs::write(&f.decision, "hold").unwrap();
    f.wl(&["finish", "alpha"]);
    let (task, _) = f.wait_for("alpha", "needs-action");
    assert!(task["finish"]["detail"].as_str().unwrap().contains("hold"), "{task}");
    let pushed = Command::new("git").arg("-C").arg(&f.origin).args(["rev-parse", "--verify", "--quiet", "alpha"]).output().unwrap();
    assert!(!pushed.status.success(), "no topic push");
    let _ = worktree;
}

#[test]
fn a_held_task_is_not_accepted() {
    let f = Fixture::new(None);
    f.begin_with_commit("alpha", "b.txt", "b\n");
    f.wl(&["hold", "alpha", "--reason", "device check"]);
    let refused = f.wl(&["finish", "alpha"]);
    assert!(refused["error"].as_str().unwrap().contains("held"), "{refused}");
}

#[test]
fn any_command_restarts_a_finish_whose_worker_is_gone() {
    let f = Fixture::new(None);
    let worktree = f.begin_with_commit("alpha", "b.txt", "b\n");
    std::fs::write(&f.decision, "hold").unwrap();
    f.wl(&["finish", "alpha"]);
    f.wait_for("alpha", "needs-action");
    std::fs::write(&f.decision, "push").unwrap();
    // A worker that died before writing any progress leaves only the acceptance behind.
    std::fs::remove_file(f.main.join(".git/wl/finishes/alpha/progress.json")).unwrap();
    let commit = git(&worktree, &["rev-parse", "HEAD"]);
    f.wait_for("alpha", "integrated");
    assert_eq!(f.origin_main(), commit);
}

#[cfg(unix)]
#[test]
fn the_target_waits_for_required_checks_and_a_failed_check_needs_action() {
    use std::os::unix::fs::PermissionsExt;
    let checks = r#"[{"app": "github-actions", "name": "test"}]"#;
    let f = Fixture::new(None);
    let worktree = f.begin_with_commit("alpha", ".github/required-checks.json", checks);
    git(&f.main, &["config", "remote.origin.url", "https://github.com/example/product.git"]);
    git(&f.main, &["config", &format!("url.{}.insteadOf", f.origin.display()), "https://github.com/example/product.git"]);
    let bin = f.dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let conclusion = f.dir.path().join("conclusion.txt");
    std::fs::write(&conclusion, "").unwrap();
    let gh = format!(
        "#!/bin/sh\nc=$(cat {})\nif [ -z \"$c\" ]; then echo '{{\"check_runs\":[]}}'; else echo '{{\"check_runs\":[{{\"id\":1,\"name\":\"test\",\"app\":{{\"slug\":\"github-actions\"}},\"status\":\"completed\",\"conclusion\":\"'$c'\",\"html_url\":\"u\"}}]}}'; fi\n",
        conclusion.display()
    );
    std::fs::write(bin.join("gh"), gh).unwrap();
    std::fs::set_permissions(bin.join("gh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let before = f.origin_main();
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_wl")).arg("--repo").arg(&f.main).args(args)
            .env("PATH", &path).env("WL_CONFIG", f.dir.path().join("config.json"))
            .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t")
            .output().unwrap();
        serde_json::from_slice::<Value>(&out.stdout).unwrap()
    };
    run(&["finish", "alpha"]);
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(f.origin_main(), before, "nothing reaches the target while the check is pending");
    std::fs::write(&conclusion, "failure").unwrap();
    let start = Instant::now();
    loop {
        let status = run(&["status", "alpha"]);
        if status["tasks"][0]["finish"]["stage"] == "needs-action" {
            assert!(status["tasks"][0]["finish"]["detail"].as_str().unwrap().contains("required check test"), "{status}");
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(60), "{status}");
        std::thread::sleep(Duration::from_millis(200));
    }
    assert_eq!(f.origin_main(), before);
    let _ = worktree;
}

#[cfg(unix)]
#[test]
fn any_steps_run_in_the_offload_directory_with_fresh_file_times_and_windows_steps_run_here() {
    let probe = "import os, sys; open(sys.argv[1], 'a').write(os.getcwd() + ' ' + os.environ.get('WL_OFFLOADED', 'here') + ' ' + str(os.path.getmtime('a.txt')) + '\\n')";
    let dir = tempfile::tempdir().unwrap();
    let record = dir.path().join("record.txt");
    let toml = format!(
        "[[validation]]\nargv = [\"{PYTHON}\", \"-c\", {probe:?}, {record:?}]\nwhere = \"any\"\n\n[[validation]]\nargv = [\"{PYTHON}\", \"-c\", {probe:?}, {record:?}]\nwhere = \"windows\"\n",
        record = record.display().to_string()
    );
    let f = Fixture::new(Some(&toml));
    let offload_root = dir.path().join("offload");
    let config = serde_json::json!({
        "preflight": [PYTHON, f.dir.path().join("preflight.py"), "{repo}", f.decision],
        "offload": { "prefix": ["env", "WL_OFFLOADED=offloaded"], "root": offload_root },
    });
    std::fs::write(f.dir.path().join("config.json"), config.to_string()).unwrap();
    f.begin_with_commit("alpha", "b.txt", "b\n");
    let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
    f.wl(&["finish", "alpha"]);
    f.wait_for("alpha", "integrated");
    let lines: Vec<String> = std::fs::read_to_string(&record).unwrap().lines().map(String::from).collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    let any: Vec<&str> = lines[0].split(' ').collect();
    assert_eq!(any[0], offload_root.join("product-main").display().to_string());
    assert_eq!(any[1], "offloaded");
    assert!(any[2].parse::<f64>().unwrap() >= started - 1.0, "files carry the time they were written, not the commit's: {lines:?}");
    let windows: Vec<&str> = lines[1].split(' ').collect();
    assert!(windows[0].ends_with("integrate-main"), "{lines:?}");
    assert_eq!(windows[1], "here");
}

#[test]
fn a_task_begun_with_a_target_starts_from_it_and_integrates_into_it() {
    let f = Fixture::new(None);
    git(&f.main, &["push", "-q", "origin", "HEAD:refs/heads/feature"]);
    let pusher = f.dir.path().join("pusher");
    git(f.dir.path(), &["clone", "-q", "-b", "feature", f.origin.to_str().unwrap(), pusher.to_str().unwrap()]);
    std::fs::write(pusher.join("f.txt"), "f\n").unwrap();
    git(&pusher, &["add", "f.txt"]);
    git(&pusher, &["commit", "-q", "-m", "feature work"]);
    git(&pusher, &["push", "-q", "origin", "feature"]);
    git(&f.main, &["fetch", "-q", "origin"]);
    let feature = git(&pusher, &["rev-parse", "HEAD"]);
    let main_before = f.origin_main();

    let out = f.wl(&["begin", "alpha", "--request", "r", "--target", "feature"]);
    assert_eq!(out["base"], feature.as_str(), "{out}");
    assert_eq!(out["target"], "feature");
    let worktree = PathBuf::from(out["worktree"].as_str().unwrap());
    std::fs::write(worktree.join("b.txt"), "b\n").unwrap();
    git(&worktree, &["add", "b.txt"]);
    git(&worktree, &["commit", "-q", "-m", "work"]);
    let commit = git(&worktree, &["rev-parse", "HEAD"]);
    f.wl(&["finish", "alpha", "--retire"]);
    f.wait_for("alpha", "retired");
    assert_eq!(git(&f.origin, &["rev-parse", "feature"]), commit);
    assert_eq!(f.origin_main(), main_before);
}
