//! Command latency on a repository of the first target's size. CI runs it with `--ignored` in release mode.
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// ADR 0008: p90 50 ms on the Windows controller. Runner VMs differ by about 2x in speed, so CI compares
/// each command scaled to a machine whose process start through pipes takes REFERENCE_START_MS, the fast
/// runners' baseline; a faster machine is not scaled up. The controller measurement in P6 is the acceptance;
/// this gate catches regressions.
const LIMIT_MS: f64 = 50.0;
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
print(json.dumps({"decision": "push", "reason": "TEST", "remote": "origin", "destination": merge[len("refs/heads/"):] if merge else branch}))
"#;
const REFERENCE_START_MS: f64 = 10.0;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git").arg("-C").arg(dir).args(args)
        .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t")
        .output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// Prints the run's phase trace when it is slower than this, to explain outliers.
const EXPLAIN_ABOVE_MS: f64 = 40.0;

fn timed(repo: &Path, args: &[&str]) -> (f64, Value) {
    let start = Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_wl")).arg("--repo").arg(repo).args(args).env("WL_TRACE", "1").output().unwrap();
    let ms = start.elapsed().as_secs_f64() * 1e3;
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["ok"], true, "{args:?}: {value}");
    if ms > EXPLAIN_ABOVE_MS {
        let inner = String::from_utf8_lossy(&out.stderr);
        println!("slow {args:?} wall {ms:.2} ms; {}", inner.lines().map(str::trim).collect::<Vec<_>>().join(" | "));
    }
    (ms, value)
}

fn trace(repo: &Path, args: &[&str]) {
    let start = Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_wl")).arg("--repo").arg(repo).args(args).env("WL_TRACE", "1").output().unwrap();
    let ms = start.elapsed().as_secs_f64() * 1e3;
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    let inner = String::from_utf8_lossy(&out.stderr);
    println!("trace {args:?} wall {ms:.2} ms; {}", inner.lines().map(str::trim).collect::<Vec<_>>().join(" | "));
}

fn median(samples: &[f64]) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[sorted.len() / 2]
}

fn p90(name: &str, mut samples: Vec<f64>) -> f64 {
    samples.sort_by(f64::total_cmp);
    let p90 = samples[((samples.len() - 1) as f64 * 0.9).round() as usize];
    println!("{{\"case\":{name:?},\"n\":{},\"p50_ms\":{:.2},\"p90_ms\":{p90:.2},\"max_ms\":{:.2}}}",
        samples.len(), samples[samples.len() / 2], samples[samples.len() - 1]);
    p90
}

fn wait_for_free_slot(repo: &Path) {
    let start = Instant::now();
    while timed(repo, &["status"]).1["slots"]["free"].as_u64() < Some(1) {
        assert!(start.elapsed() < Duration::from_secs(120), "no free slot was prepared");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn wait_until_retired(repo: &Path, task: &str) {
    let start = Instant::now();
    loop {
        let status = timed(repo, &["status"]).1;
        let tasks = status["tasks"].as_array().unwrap();
        if let Some(t) = tasks.iter().find(|t| t["task"] == task) {
            assert_ne!(t["finish"]["stage"], "needs-action", "{t}");
        } else {
            return;
        }
        assert!(start.elapsed() < Duration::from_secs(120), "{task} was not retired: {status}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Waits until background work has ended and both spare slots are free, so the next begin reuses a slot.
fn wait_until_quiet(repo: &Path) {
    // The detached preparation takes its lock shortly after it starts.
    std::thread::sleep(Duration::from_millis(300));
    let start = Instant::now();
    loop {
        let status = timed(repo, &["status"]).1;
        if status["background"] == false && status["slots"]["free"].as_u64() >= Some(2) {
            return;
        }
        assert!(start.elapsed() < Duration::from_secs(120), "slots did not become free");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "measures release-mode latency; run in CI"]
fn foreground_commands_stay_under_the_latency_limit() {
    let dir = tempfile::tempdir().unwrap();
    let seed = dir.path().join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "-q", "-b", "main"]);
    for i in 0..400 {
        std::fs::write(seed.join(format!("f{i}.txt")), "x\n".repeat(64 + i)).unwrap();
    }
    git(&seed, &["add", "-A"]);
    git(&seed, &["commit", "-q", "-m", "seed"]);
    let origin = dir.path().join("origin.git");
    git(dir.path(), &["clone", "-q", "--bare", seed.to_str().unwrap(), origin.to_str().unwrap()]);
    let main: PathBuf = dir.path().join("product");
    git(dir.path(), &["clone", "-q", origin.to_str().unwrap(), main.to_str().unwrap()]);
    let script = dir.path().join("preflight.py");
    std::fs::write(&script, PREFLIGHT).unwrap();
    let config = dir.path().join("config.json");
    std::fs::write(&config, serde_json::json!({ "preflight": [PYTHON, script, "{repo}"] }).to_string()).unwrap();
    // SAFETY: set before any thread of this test starts a process.
    unsafe { std::env::set_var("WL_CONFIG", &config) };

    timed(&main, &["begin", "first", "--request", "r"]);
    wait_for_free_slot(&main);
    timed(&main, &["begin", "warm", "--request", "r"]);
    wait_for_free_slot(&main);
    timed(&main, &["retire", "warm"]);
    let mut begin = Vec::new();
    for i in 0..35 {
        wait_until_quiet(&main);
        let name = format!("t{i}");
        if i < 30 {
            begin.push(timed(&main, &["begin", &name, "--request", "r"]).0);
        } else {
            trace(&main, &["begin", &name, "--request", "r"]);
        }
        timed(&main, &["retire", &name]);
    }
    let mut finish = Vec::new();
    for i in 0..15 {
        wait_until_quiet(&main);
        let name = format!("f{i}");
        let out = timed(&main, &["begin", &name, "--request", "r"]).1;
        let worktree = PathBuf::from(out["worktree"].as_str().unwrap());
        std::fs::write(worktree.join(format!("{name}.txt")), "x\n").unwrap();
        git(&worktree, &["add", "-A"]);
        git(&worktree, &["commit", "-q", "-m", &name]);
        finish.push(timed(&main, &["finish", &name, "--retire"]).0);
        wait_until_retired(&main, &name);
    }
    wait_until_quiet(&main);
    let baseline: Vec<f64> = (0..30).map(|_| {
        let start = Instant::now();
        Command::new(env!("CARGO_BIN_EXE_wl")).arg("help").output().unwrap();
        start.elapsed().as_secs_f64() * 1e3
    }).collect();
    let start_ms = median(&baseline);
    p90("process_start_and_pipes_baseline", baseline);
    let scale = (REFERENCE_START_MS / start_ms).min(1.0);
    let status: Vec<f64> = (0..30).map(|_| timed(&main, &["status"]).0).collect();
    let mut hold = Vec::new();
    for _ in 0..20 {
        hold.push(timed(&main, &["hold", "first", "--reason", "x"]).0);
        hold.push(timed(&main, &["release-hold", "first", "--decision", "y"]).0);
    }
    for args in [&["status"][..], &["hold", "first", "--reason", "x"], &["release-hold", "first", "--decision", "y"]] {
        trace(&main, args);
    }
    let slots = timed(&main, &["status"]).1["slots"]["total"].as_u64().unwrap();
    assert!(slots <= 3, "the measured begins reused slots instead of creating them: {slots} slots");
    let worst = [p90("begin", begin), p90("status", status), p90("hold_and_release", hold), p90("finish_acceptance", finish)].map(|p| p * scale);
    println!("p90 scaled to a {REFERENCE_START_MS} ms process start (scale {scale:.2}): {worst:?}");
    assert!(worst.iter().all(|&p| p <= LIMIT_MS), "scaled p90 above {LIMIT_MS} ms: {worst:?}");
}
