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

    /// Polls status until a notice about `stage` arrives; the worker writes it just after its progress.
    fn wait_for_notice(&self, stage: &str, mut notices: Vec<Value>) -> Vec<Value> {
        let start = Instant::now();
        while !notices.iter().any(|n| n["stage"] == stage) {
            assert!(start.elapsed() < Duration::from_secs(90), "no {stage} notice");
            std::thread::sleep(Duration::from_millis(100));
            notices.extend(self.wl(&["status"])["notices"].as_array().cloned().unwrap_or_default());
        }
        notices
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

    fn wait_for_finish(&self, id: &str, stage: &str) -> Value {
        let start = Instant::now();
        loop {
            let status = self.wl(&["status", "--finish", id]);
            assert_eq!(status["ok"], true, "{status}");
            let observed = status["finish"]["stage"].as_str();
            if observed == Some(stage) {
                return status;
            }
            if observed == Some("needs-action") && stage != "needs-action" {
                panic!("finish {id} entered needs-action while waiting for {stage}: {status}");
            }
            assert!(start.elapsed() < Duration::from_secs(90), "finish {id} did not reach {stage}: {status}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[test]
fn finish_returns_at_once_and_the_worker_fast_forwards_the_target() {
    let f = Fixture::new(None);
    let worktree = f.begin_with_commit("alpha", "b.txt", "b\n");
    let commit = git(&worktree, &["rev-parse", "HEAD"]);
    std::fs::write(worktree.join("new.txt"), "not added\n").unwrap();
    let out = f.wl(&["finish", "alpha"]);
    assert_eq!(out["ok"], true, "{out}");
    assert_eq!(out["commit"], commit.as_str());
    let (task, notices) = f.wait_for("alpha", "integrated");
    let notices = f.wait_for_notice("integrated", notices);
    assert_eq!(task["finish"]["uncommitted"], true, "{task}");
    assert!(notices[0]["message"].as_str().unwrap().contains("untracked files"), "{notices:?}");
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
    let notices = f.wait_for_notice("retired", notices);
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
    let (_, notices) = f.wait_for("alpha", "retired");
    f.wait_for_notice("retired", notices);
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
    let notices = f.wait_for_notice("needs-action", notices);
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
fn the_worker_decides_pushes_itself_from_the_push_policy() {
    let f = Fixture::new(None);
    f.begin_with_commit("alpha", "b.txt", "b\n");
    // A local origin has no hosting metadata, so wl's own decision asks instead of pushing.
    let policy = f.dir.path().join("policy.json");
    std::fs::write(&policy, r#"{"default_branch_push_repositories": []}"#).unwrap();
    std::fs::write(f.dir.path().join("config.json"), serde_json::json!({ "push_policy": policy }).to_string()).unwrap();
    f.wl(&["finish", "alpha"]);
    let (task, _) = f.wait_for("alpha", "needs-action");
    assert!(task["finish"]["detail"].as_str().unwrap().contains("METADATA_UNAVAILABLE"), "{task}");
    let pushed = Command::new("git").arg("-C").arg(&f.origin).args(["rev-parse", "--verify", "--quiet", "alpha"]).output().unwrap();
    assert!(!pushed.status.success(), "no topic push");

    std::fs::write(
        f.dir.path().join("config.json"),
        serde_json::json!({ "push_policy": policy, "preflight": ["python", "{repo}"] }).to_string(),
    )
    .unwrap();
    f.wl(&["finish", "alpha"]);
    let start = Instant::now();
    loop {
        let task = f.wl(&["status", "alpha"])["tasks"][0].clone();
        let detail = task["finish"]["detail"].as_str().unwrap_or_default().to_owned();
        if detail.contains("keep one") {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(90), "both settings were not refused: {task}");
        std::thread::sleep(Duration::from_millis(100));
    }
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
    a_required_check_needs_action("failure", &["required check test"]);
}

#[cfg(unix)]
#[test]
fn a_concluded_check_github_never_completed_needs_action_with_its_rerun() {
    a_required_check_needs_action("unfinalized", &["required check test", "will not finish", "gh run rerun -R example/product --job 1"]);
}

#[cfg(unix)]
#[test]
fn a_workflow_github_could_not_start_needs_action_with_its_run() {
    a_required_check_needs_action("workflow failure", &["required check test", "could not start", "https://github.com/example/product/actions/runs/44"]);
}

#[cfg(unix)]
fn a_required_check_needs_action(last: &str, details: &[&str]) {
    use std::os::unix::fs::PermissionsExt;
    let checks = r#"[{"app": "github-actions", "name": "test"}]"#;
    let f = Fixture::new(None);
    let worktree = f.begin_with_commit("alpha", ".github/required-checks.json", checks);
    git(&f.main, &["config", "remote.origin.url", "https://github.com/example/product.git"]);
    git(&f.main, &["config", &format!("url.{}.insteadOf", f.origin.display()), "https://github.com/example/product.git"]);
    let bin = f.dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    // gh only hands over the token; the check runs come from a local stand-in for the GitHub API.
    let gh_calls = f.dir.path().join("gh-calls.txt");
    std::fs::write(bin.join("gh"), format!("#!/bin/sh\necho \"$*\" >> {}\necho the-token\n", gh_calls.display())).unwrap();
    std::fs::set_permissions(bin.join("gh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let conclusion = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let api = format!("http://{}", listener.local_addr().unwrap());
    {
        let (conclusion, requests) = (conclusion.clone(), requests.clone());
        std::thread::spawn(move || {
            use std::io::{BufRead, Write};
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                    head.push_str(&line);
                }
                let path = head.split_whitespace().nth(1).unwrap_or_default().to_owned();
                requests.lock().unwrap().push(head);
                let c = conclusion.lock().unwrap().clone();
                // Until then the Actions suite is queued without runs; another app's suite stays queued for good.
                let body = if path.contains("/check-suites") {
                    let (status, conclusion) = if c == "workflow failure" { ("completed", r#""failure""#) } else { ("queued", "null") };
                    format!(r#"{{"check_suites":[{{"id":7,"app":{{"slug":"github-actions"}},"status":"{status}","conclusion":{conclusion},"latest_check_runs_count":0,"url":"s"}},{{"id":8,"app":{{"slug":"cursor"}},"status":"queued","conclusion":null,"latest_check_runs_count":0}}]}}"#)
                } else if path.contains("/actions/runs") {
                    r#"{"workflow_runs":[{"id":44,"check_suite_id":7,"html_url":"https://github.com/example/product/actions/runs/44"}]}"#.to_owned()
                } else if c.is_empty() || c == "workflow failure" {
                    r#"{"check_runs":[]}"#.to_owned()
                } else if c == "unfinalized" {
                    r#"{"check_runs":[{"id":1,"name":"test","app":{"slug":"github-actions"},"status":"in_progress","conclusion":"success","completed_at":"2026-11-20T23:29:51Z","html_url":"u"}]}"#.to_owned()
                } else {
                    format!(r#"{{"check_runs":[{{"id":1,"name":"test","app":{{"slug":"github-actions"}},"status":"completed","conclusion":"{c}","html_url":"u"}}]}}"#)
                };
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
    }
    let before = f.origin_main();
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_wl")).arg("--repo").arg(&f.main).args(args)
            .env("PATH", &path).env("WL_CONFIG", f.dir.path().join("config.json")).env("WL_GITHUB_API", &api)
            .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t")
            .output().unwrap();
        serde_json::from_slice::<Value>(&out.stdout).unwrap()
    };
    run(&["finish", "alpha"]);
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(f.origin_main(), before, "nothing reaches the target while the check is pending");
    assert_ne!(run(&["status", "alpha"])["tasks"][0]["finish"]["stage"], "needs-action", "a queued suite keeps the wait");
    *conclusion.lock().unwrap() = last.to_owned();
    let start = Instant::now();
    loop {
        let status = run(&["status", "alpha"]);
        if status["tasks"][0]["finish"]["stage"] == "needs-action" {
            let detail = status["tasks"][0]["finish"]["detail"].as_str().unwrap();
            assert!(details.iter().all(|d| detail.contains(d)), "{status}");
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(60), "{status}");
        std::thread::sleep(Duration::from_millis(200));
    }
    assert_eq!(f.origin_main(), before);
    // The attempt has ended, so a corrected commit finishes again.
    git(&worktree, &["commit", "--allow-empty", "-m", "fix"]);
    let again = run(&["finish", "alpha"]);
    assert_eq!(again["attempt"], 2, "{again}");
    let start = Instant::now();
    while run(&["status", "alpha"])["tasks"][0]["finish"]["stage"] != "needs-action" {
        assert!(start.elapsed() < Duration::from_secs(60));
        std::thread::sleep(Duration::from_millis(200));
    }
    let requests = requests.lock().unwrap().clone();
    assert!(requests[0].contains("/repos/example/product/commits/"), "{requests:?}");
    assert!(requests.iter().all(|r| r.to_lowercase().contains("authorization: bearer the-token")), "{requests:?}");
    assert_eq!(std::fs::read_to_string(&gh_calls).unwrap(), "auth token\nauth token\n", "gh starts once per wait of the two attempts, not once per poll");
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

fn overlap_fixture(delay_secs: f64) -> (Fixture, PathBuf) {
    let f = Fixture::new(None);
    let log = f.dir.path().join("val_events.jsonl");
    let script = f.dir.path().join("validator.py");
    let body = r#"import json, os, pathlib, subprocess, sys, time
log_path = pathlib.Path(sys.argv[1])
delay = float(sys.argv[2])
head = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
files = sorted(p.name for p in pathlib.Path.cwd().iterdir() if p.name not in (".git", ".lifecycle.toml"))
start = time.time()
with log_path.open("a", encoding="utf-8") as fh:
    fh.write(json.dumps({"event": "start", "pid": os.getpid(), "head": head, "files": files, "ts": start}) + "\n")
time.sleep(delay)
head_end = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
files_end = sorted(p.name for p in pathlib.Path.cwd().iterdir() if p.name not in (".git", ".lifecycle.toml"))
end = time.time()
with log_path.open("a", encoding="utf-8") as fh:
    fh.write(json.dumps({"event": "end", "pid": os.getpid(), "head": head_end, "files": files_end, "ts": end}) + "\n")
"#;
    std::fs::write(&script, body).unwrap();
    let toml = format!(
        "[[validation]]\nargv = [\"{PYTHON}\", {}, {}, {:?}]\nwhere = \"windows\"\n",
        serde_json::to_string(&script).unwrap(),
        serde_json::to_string(&log).unwrap(),
        delay_secs.to_string(),
    );
    let pusher = f.dir.path().join("cfg_pusher");
    git(f.dir.path(), &["clone", "-q", f.origin.to_str().unwrap(), pusher.to_str().unwrap()]);
    std::fs::write(pusher.join(".lifecycle.toml"), toml).unwrap();
    git(&pusher, &["add", ".lifecycle.toml"]);
    git(&pusher, &["commit", "-q", "-m", "add validation"]);
    git(&pusher, &["push", "-q", "origin", "main"]);
    git(&f.main, &["fetch", "-q", "origin"]);
    (f, log)
}

fn read_val_events(log: &Path) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(log) else { return Vec::new() };
    text.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).unwrap()).collect()
}

fn wait_for_val_starts(log: &Path, count: usize) {
    let start = Instant::now();
    loop {
        let starts = read_val_events(log).into_iter().filter(|e| e["event"] == "start").count();
        if starts >= count {
            return;
        }
        assert!(start.elapsed() < Duration::from_secs(30), "only {starts} starts");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn assert_disjoint_validations(log: &Path) {
    let events = read_val_events(log);
    let mut intervals = Vec::new();
    for start in events.iter().filter(|e| e["event"] == "start") {
        let pid = start["pid"].as_i64().unwrap();
        let end = events.iter().find(|e| e["event"] == "end" && e["pid"] == pid).expect("missing end event");
        assert_eq!(start["head"], end["head"], "HEAD mutated mid-validation: {events:?}");
        assert_eq!(start["files"], end["files"], "files mutated mid-validation: {events:?}");
        intervals.push((start["ts"].as_f64().unwrap(), end["ts"].as_f64().unwrap()));
    }
    intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
    for pair in intervals.windows(2) {
        assert!(pair[0].1 <= pair[1].0 + 1e-6, "overlapping validations: {intervals:?}");
    }
}

#[test]
fn tasks_whose_names_share_a_suffix_do_not_overlap_in_integration() {
    let (f, log) = overlap_fixture(1.5);
    f.begin_with_commit("x-b", "xb.txt", "xb\n");
    f.begin_with_commit("b", "b.txt", "b\n");
    assert_eq!(f.wl(&["finish", "x-b"])["ok"], true);
    wait_for_val_starts(&log, 1);
    assert_eq!(f.wl(&["finish", "b"])["ok"], true);
    f.wait_for("x-b", "integrated");
    f.wait_for("b", "integrated");
    assert_disjoint_validations(&log);
    assert_eq!(git(&f.origin, &["show", "main:xb.txt"]), "xb");
    assert_eq!(git(&f.origin, &["show", "main:b.txt"]), "b");
}

#[test]
fn a_restarted_worker_with_an_older_timestamp_does_not_overlap_active_validation() {
    let (f, log) = overlap_fixture(1.5);
    let wt_a = f.begin_with_commit("a", "file_a.txt", "file_a\n");
    let sha_a = git(&wt_a, &["rev-parse", "HEAD"]);
    f.begin_with_commit("b", "b.txt", "b\n");
    assert_eq!(f.wl(&["finish", "b"])["ok"], true);
    wait_for_val_starts(&log, 1);

    let b_accept: Value = serde_json::from_slice(&std::fs::read(f.main.join(".git/wl/finishes/b/accept.json")).unwrap()).unwrap();
    let older_ts = b_accept["accepted_at_ms"].as_u64().unwrap().saturating_sub(5000);
    let a_dir = f.main.join(".git/wl/finishes/a");
    std::fs::create_dir_all(&a_dir).unwrap();
    std::fs::write(
        a_dir.join("accept.json"),
        serde_json::json!({
            "version": 1, "task": "a", "attempt": 1, "commit": sha_a, "target": "main", "retire": false, "accepted_at_ms": older_ts
        }).to_string(),
    ).unwrap();
    std::fs::write(
        a_dir.join("progress.json"),
        serde_json::json!({
            "version": 1, "task": "a", "attempt": 1, "stage": "topic-pushed", "merged": null, "target_base": null, "detail": null, "uncommitted": false
        }).to_string(),
    ).unwrap();

    assert_eq!(f.wl(&["status"])["ok"], true);
    f.wait_for("a", "integrated");
    f.wait_for("b", "integrated");
    assert_disjoint_validations(&log);
    assert_eq!(git(&f.origin, &["show", "main:file_a.txt"]), "file_a");
    assert_eq!(git(&f.origin, &["show", "main:b.txt"]), "b");
}

#[test]
fn finish_with_retire_status_by_id_reports_result_after_retirement_and_notice_drain() {
    let f = Fixture::new(None);
    let worktree = f.begin_with_commit("alpha", "b.txt", "b\n");
    let commit = git(&worktree, &["rev-parse", "HEAD"]);
    let out = f.wl(&["finish", "alpha", "--retire"]);
    assert_eq!(out["ok"], true, "{out}");
    assert_eq!(out["retire"], true, "{out}");
    let id = out["id"].as_str().expect("finish id").to_owned();
    assert!(!id.is_empty());

    let (_, notices) = f.wait_for("alpha", "retired");
    let notices = f.wait_for_notice("retired", notices);
    assert!(notices.iter().any(|n| n["stage"] == "retired"), "{notices:?}");
    let drained = f.wl(&["status"]);
    assert!(drained.get("notices").is_none(), "{drained}");
    assert_eq!(f.wl(&["status", "alpha"])["ok"], false);

    for args in [&["status", "--finish", id.as_str()][..], &["status", "alpha", "--finish", id.as_str()]] {
        let status = f.wl(args);
        assert_eq!(status["ok"], true, "{args:?}: {status}");
        let fin = &status["finish"];
        assert_eq!(fin["id"], id.as_str());
        assert_eq!(fin["task"], "alpha");
        assert_eq!(fin["attempt"], 1);
        assert_eq!(fin["commit"], commit.as_str());
        assert_eq!(fin["merged"], commit.as_str());
        assert_eq!(fin["target"], "main");
        assert_eq!(fin["retire"], true);
        assert_eq!(fin["stage"], "retired");
        assert_eq!(fin["detail"], Value::Null);
        // The time of each stage outlives the task, so the finish can be timed after retirement.
        let stages: Vec<&str> = fin["stages"].as_array().unwrap().iter().map(|s| s["stage"].as_str().unwrap()).collect();
        assert_eq!(stages, ["topic-pushed", "prepared", "target-pushed", "integrated", "retired"], "{fin}");
        let mut last = fin["accepted_at_ms"].as_u64().unwrap();
        for at in fin["stages"].as_array().unwrap().iter().map(|s| s["at_ms"].as_u64().unwrap()) {
            assert!(at >= last, "stage times run forward: {fin}");
            last = at;
        }
    }

    let mismatch = f.wl(&["status", "beta", "--finish", &id]);
    assert_eq!(mismatch["ok"], false, "{mismatch}");
}

#[test]
fn unregistered_failed_and_reused_name_runs_are_not_treated_as_prior_success() {
    let f = Fixture::new(None);
    assert_eq!(f.wl(&["status", "--finish", "missing-id"])["ok"], false);
    assert_eq!(f.wl(&["status", "--finish", "../bad"])["ok"], false);
    assert_eq!(f.wl(&["status", "unregistered"])["ok"], false);

    let wt1 = f.begin_with_commit("alpha", "b.txt", "b\n");
    let c1 = git(&wt1, &["rev-parse", "HEAD"]);
    let out1 = f.wl(&["finish", "alpha", "--retire"]);
    assert_eq!(out1["ok"], true, "{out1}");
    let id1 = out1["id"].as_str().unwrap().to_owned();
    let s1 = f.wait_for_finish(&id1, "retired");
    assert_eq!(s1["finish"]["commit"], c1.as_str());

    let wt2 = f.wl(&["begin", "alpha", "--request", "r2"]);
    assert_eq!(wt2["ok"], true, "{wt2}");
    assert_eq!(wt2["finish"], Value::Null, "{wt2}");
    std::fs::write(&f.decision, "hold").unwrap();
    let out2 = f.wl(&["finish", "alpha", "--retire"]);
    assert_eq!(out2["ok"], true, "{out2}");
    let id2 = out2["id"].as_str().unwrap().to_owned();
    assert_ne!(id2, id1);
    let s2 = f.wait_for_finish(&id2, "needs-action");
    assert!(s2["finish"]["detail"].as_str().unwrap().contains("hold"), "{s2}");

    assert_eq!(f.wl(&["retire", "alpha"])["ok"], true);
    assert_eq!(f.wl(&["status", "alpha"])["ok"], false);
    assert_eq!(f.wl(&["status", "--finish", &id2])["finish"]["stage"], "needs-action");
    let s1_after = f.wl(&["status", "--finish", &id1]);
    assert_eq!(s1_after["finish"]["stage"], "retired");
    assert_eq!(s1_after["finish"]["commit"], c1.as_str());

    std::fs::write(&f.decision, "push").unwrap();
    let wt3 = f.begin_with_commit("alpha", "c.txt", "c\n");
    let c3 = git(&wt3, &["rev-parse", "HEAD"]);
    let out3 = f.wl(&["finish", "alpha", "--retire"]);
    assert_eq!(out3["ok"], true, "{out3}");
    let id3 = out3["id"].as_str().unwrap().to_owned();
    assert!(id3 != id1 && id3 != id2);
    let s3 = f.wait_for_finish(&id3, "retired");
    assert_eq!(s3["finish"]["commit"], c3.as_str());
    assert_eq!(s3["finish"]["merged"], c3.as_str());
}

#[test]
fn crashed_retire_after_task_delete_finalizes_run_on_next_status_and_before_rebegin() {
    let f = Fixture::new(None);
    let simulate_crash = |task: &str, file: &str, content: &str| {
        let wt = f.begin_with_commit(task, file, content);
        let commit = git(&wt, &["rev-parse", "HEAD"]);
        let out = f.wl(&["finish", task]);
        assert_eq!(out["ok"], true, "{out}");
        let id = out["id"].as_str().expect("finish id").to_owned();
        f.wait_for(task, "integrated");
        while f.wl(&["status"])["background"] != false {
            std::thread::sleep(Duration::from_millis(50));
        }

        let accept_path = f.main.join(format!(".git/wl/finishes/{task}/accept.json"));
        let mut accept: Value = serde_json::from_slice(&std::fs::read(&accept_path).unwrap()).unwrap();
        accept["retire"] = Value::Bool(true);
        std::fs::write(&accept_path, serde_json::to_vec_pretty(&accept).unwrap()).unwrap();

        let run_path = f.main.join(format!(".git/wl/runs/{id}.json"));
        let mut run: Value = serde_json::from_slice(&std::fs::read(&run_path).unwrap()).unwrap();
        run["retire"] = Value::Bool(true);
        std::fs::write(&run_path, serde_json::to_vec_pretty(&run).unwrap()).unwrap();

        std::fs::remove_file(f.main.join(format!(".git/wl/tasks/{task}.json"))).unwrap();
        (id, commit, accept_path)
    };

    let (id1, c1, accept1) = simulate_crash("alpha", "b.txt", "b\n");
    let st1 = f.wl(&["status", "--finish", &id1]);
    assert_eq!(st1["ok"], true, "{st1}");
    assert_eq!(st1["finish"]["stage"], "retired", "{st1}");
    assert_eq!(st1["finish"]["commit"], c1.as_str());
    assert_eq!(st1["finish"]["merged"], c1.as_str());
    assert!(!accept1.exists(), "accept.json should be cleaned after finalizing retired finish");
    let claims_dir = f.main.join(".git/wl/slots");
    for entry in std::fs::read_dir(&claims_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) == Some("claim") {
            assert_ne!(std::fs::read_to_string(&path).unwrap(), "alpha", "claim should be released");
        }
    }

    let (id2, c2, _) = simulate_crash("beta", "d.txt", "d\n");
    let rebegin = f.wl(&["begin", "beta", "--request", "r2"]);
    assert_eq!(rebegin["ok"], true, "{rebegin}");
    assert_eq!(rebegin["finish"], Value::Null, "{rebegin}");
    let st2 = f.wl(&["status", "--finish", &id2]);
    assert_eq!(st2["ok"], true, "{st2}");
    assert_eq!(st2["finish"]["stage"], "retired", "{st2}");
    assert_eq!(st2["finish"]["commit"], c2.as_str());
}
