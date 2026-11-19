//! Command latency on a repository of the first target's size. CI runs it with `--ignored` in release mode.
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const LIMIT_MS: f64 = 50.0;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git").arg("-C").arg(dir).args(args)
        .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t")
        .output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn timed(repo: &Path, args: &[&str]) -> (f64, Value) {
    let start = Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_wl")).arg("--repo").arg(repo).args(args).output().unwrap();
    let ms = start.elapsed().as_secs_f64() * 1e3;
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["ok"], true, "{args:?}: {value}");
    (ms, value)
}

fn trace(repo: &Path, args: &[&str]) {
    let out = Command::new(env!("CARGO_BIN_EXE_wl")).arg("--repo").arg(repo).args(args).env("WL_TRACE", "1").output().unwrap();
    println!("trace {args:?}\n{}", String::from_utf8_lossy(&out.stderr));
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

/// Waits until both spare slots are free again, so the next begin reuses a slot as real use does.
fn wait_until_quiet(repo: &Path) {
    std::thread::sleep(Duration::from_millis(300));
    let start = Instant::now();
    while timed(repo, &["status"]).1["slots"]["free"].as_u64() < Some(2) {
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
    let main: PathBuf = dir.path().join("product");
    git(dir.path(), &["clone", "-q", seed.to_str().unwrap(), main.to_str().unwrap()]);

    timed(&main, &["begin", "first", "--request", "r"]);
    wait_for_free_slot(&main);
    timed(&main, &["begin", "warm", "--request", "r"]);
    wait_for_free_slot(&main);
    timed(&main, &["retire", "warm"]);
    let mut begin = Vec::new();
    for i in 0..30 {
        wait_until_quiet(&main);
        let start = Instant::now();
        let out = Command::new(env!("CARGO_BIN_EXE_wl")).arg("--repo").arg(&main)
            .args(["begin", &format!("t{i}"), "--request", "r"]).env("WL_TRACE", "1").output().unwrap();
        let ms = start.elapsed().as_secs_f64() * 1e3;
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
        let inner = String::from_utf8_lossy(&out.stderr);
        println!("begin t{i} wall {ms:.2} ms; {}", inner.lines().map(str::trim).collect::<Vec<_>>().join(" | "));
        begin.push(ms);
        timed(&main, &["retire", &format!("t{i}")]);
    }
    wait_until_quiet(&main);
    let baseline: Vec<f64> = (0..30).map(|_| {
        let start = Instant::now();
        Command::new(env!("CARGO_BIN_EXE_wl")).arg("help").output().unwrap();
        start.elapsed().as_secs_f64() * 1e3
    }).collect();
    p90("process_start_and_pipes_baseline", baseline);
    let status: Vec<f64> = (0..30).map(|_| timed(&main, &["status"]).0).collect();
    let mut hold = Vec::new();
    for _ in 0..20 {
        hold.push(timed(&main, &["hold", "first", "--reason", "x"]).0);
        hold.push(timed(&main, &["release-hold", "first", "--decision", "y"]).0);
    }
    for args in [&["status"][..], &["hold", "first", "--reason", "x"], &["release-hold", "first", "--decision", "y"]] {
        trace(&main, args);
    }
    wait_for_free_slot(&main);
    trace(&main, &["begin", "traced", "--request", "r"]);
    let slots = timed(&main, &["status"]).1["slots"]["total"].as_u64().unwrap();
    assert!(slots <= 3, "the measured begins reused slots instead of creating them: {slots} slots");
    let worst = [p90("begin", begin), p90("status", status), p90("hold_and_release", hold)];
    assert!(worst.iter().all(|&p| p <= LIMIT_MS), "p90 above {LIMIT_MS} ms: {worst:?}");
}
