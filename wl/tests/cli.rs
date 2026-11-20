use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").arg("-C").arg(dir).args(args)
        .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t")
        .output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn wl(repo: &Path, args: &[&str]) -> Value {
    let home = repo.parent().unwrap().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_wl")).arg("--repo").arg(repo).args(args)
        .env("HOME", &home).env("USERPROFILE", &home).env("XDG_CONFIG_HOME", &home)
        .output().unwrap();
    serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("not json: {}", String::from_utf8_lossy(&out.stdout)))
}

struct Fixture {
    _dir: tempfile::TempDir,
    main: PathBuf,
    origin: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin.git");
    let seed = dir.path().join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "-q", "-b", "main"]);
    for i in 0..20 {
        std::fs::write(seed.join(format!("f{i}.txt")), format!("{i}\n")).unwrap();
    }
    git(&seed, &["add", "-A"]);
    git(&seed, &["commit", "-q", "-m", "seed"]);
    git(dir.path(), &["clone", "-q", "--bare", seed.to_str().unwrap(), origin.to_str().unwrap()]);
    let main = dir.path().join("product");
    git(dir.path(), &["clone", "-q", origin.to_str().unwrap(), main.to_str().unwrap()]);
    Fixture { _dir: dir, main, origin }
}

fn wait_for_free_slot(repo: &Path) -> Value {
    let start = Instant::now();
    loop {
        let status = wl(repo, &["status"]);
        if status["slots"]["free"].as_u64() >= Some(1) {
            return status;
        }
        assert!(start.elapsed() < Duration::from_secs(60), "no free slot was prepared: {status}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn begin_puts_the_task_on_its_own_branch_in_a_slot_beside_the_repository() {
    let f = fixture();
    let base = git(&f.main, &["rev-parse", "origin/HEAD"]);
    let out = wl(&f.main, &["begin", "alpha", "--request", "https://example.invalid/1"]);
    assert_eq!(out["ok"], true, "{out}");
    assert_eq!(out["stage"], "ready");
    let worktree = PathBuf::from(out["worktree"].as_str().unwrap());
    assert_eq!(worktree.parent().unwrap().file_name().unwrap(), "product.slots");
    assert_eq!(git(&worktree, &["symbolic-ref", "--short", "HEAD"]), "alpha");
    assert_eq!(git(&worktree, &["rev-parse", "HEAD"]), base);
    assert_eq!(git(&worktree, &["status", "--porcelain"]), "");
    assert_eq!(git(&f.main, &["symbolic-ref", "--short", "HEAD"]), "main", "the main checkout is untouched");
}

#[test]
fn begin_again_with_the_same_request_reports_the_same_task_and_another_request_is_refused() {
    let f = fixture();
    let first = wl(&f.main, &["begin", "alpha", "--request", "r1"]);
    let again = wl(&f.main, &["begin", "alpha", "--request", "r1"]);
    assert_eq!(first, again);
    let other = wl(&f.main, &["begin", "alpha", "--request", "r2"]);
    assert_eq!(other["ok"], false);
    assert!(other["error"].as_str().unwrap().contains("already exists"), "{other}");
}

#[test]
fn a_prepared_free_slot_is_used_by_the_next_begin_and_switched_by_git_refs_alone() {
    let f = fixture();
    wl(&f.main, &["begin", "alpha", "--request", "r"]);
    let status = wait_for_free_slot(&f.main);
    assert_eq!(status["slots"]["total"], 2, "{status}");
    let prepared = f.main.with_file_name("product.slots").join("2");
    let marker = std::fs::metadata(prepared.join("f0.txt")).unwrap().modified().unwrap();

    let out = wl(&f.main, &["begin", "beta", "--request", "r"]);
    assert_eq!(out["ok"], true, "{out}");
    assert_eq!(PathBuf::from(out["worktree"].as_str().unwrap()), prepared);
    assert_eq!(git(&prepared, &["symbolic-ref", "--short", "HEAD"]), "beta");
    assert_eq!(git(&prepared, &["status", "--porcelain"]), "");
    assert_eq!(std::fs::metadata(prepared.join("f0.txt")).unwrap().modified().unwrap(), marker, "files were not rewritten");
    assert!(git(&f.main, &["worktree", "list"]).contains("[beta]"));
}

#[test]
fn begin_starts_from_the_remote_default_branch_as_last_fetched() {
    let f = fixture();
    let pusher = f.main.with_file_name("pusher");
    git(f.main.parent().unwrap(), &["clone", "-q", f.origin.to_str().unwrap(), pusher.to_str().unwrap()]);
    std::fs::write(pusher.join("new.txt"), "n\n").unwrap();
    git(&pusher, &["add", "-A"]);
    git(&pusher, &["commit", "-q", "-m", "advance"]);
    git(&pusher, &["push", "-q", "origin", "main"]);
    let stale = git(&f.main, &["rev-parse", "origin/HEAD"]);

    let out = wl(&f.main, &["begin", "alpha", "--request", "r"]);
    assert_eq!(out["base"], stale.as_str(), "begin does not fetch");
    wait_for_free_slot(&f.main);
    let fresh = git(&pusher, &["rev-parse", "HEAD"]);
    assert_eq!(git(&f.main, &["rev-parse", "origin/HEAD"]), fresh, "the background preparation fetched");
    let out = wl(&f.main, &["begin", "beta", "--request", "r"]);
    assert_eq!(out["base"], fresh.as_str());
}

#[test]
fn hold_is_reported_until_released_with_a_decision() {
    let f = fixture();
    wl(&f.main, &["begin", "alpha", "--request", "r"]);
    assert_eq!(wl(&f.main, &["hold", "alpha", "--reason", "waiting for the device"])["ok"], true);
    assert_eq!(wl(&f.main, &["status", "alpha"])["tasks"][0]["hold"], "waiting for the device");
    let released = wl(&f.main, &["release-hold", "alpha", "--decision", "https://example.invalid/2"]);
    assert_eq!(released["released"], "waiting for the device");
    assert_eq!(wl(&f.main, &["status", "alpha"])["tasks"][0]["hold"], Value::Null);
    assert_eq!(wl(&f.main, &["release-hold", "alpha", "--decision", "x"])["ok"], false);
}

#[test]
fn invalid_names_and_unknown_tasks_are_refused() {
    let f = fixture();
    assert_eq!(wl(&f.main, &["begin", "../x", "--request", "r"])["ok"], false);
    assert_eq!(wl(&f.main, &["begin", "a", "--request", "r", "--branch", "bad..name"])["ok"], false);
    assert_eq!(wl(&f.main, &["hold", "missing", "--reason", "x"])["ok"], false);
    assert_eq!(wl(&f.main, &["status", "missing"])["ok"], false);
}

#[test]
fn begin_refuses_a_free_slot_with_modified_or_staged_files() {
    for staged in [false, true] {
        let f = fixture();
        wl(&f.main, &["begin", "alpha", "--request", "r"]);
        wait_for_free_slot(&f.main);
        let free = f.main.with_file_name("product.slots").join("2");
        std::fs::write(free.join("f3.txt"), "changed\n").unwrap();
        if staged {
            git(&free, &["add", "f3.txt"]);
        }
        let out = wl(&f.main, &["begin", "beta", "--request", "r"]);
        assert_eq!(out["ok"], false, "staged={staged}: {out}");
        assert!(out["error"].as_str().unwrap().contains("uncommitted changes"), "{out}");
    }
}

#[test]
fn retire_frees_the_slot_for_the_next_begin_and_deletes_the_branch() {
    let f = fixture();
    let first = wl(&f.main, &["begin", "alpha", "--request", "r"]);
    let out = wl(&f.main, &["retire", "alpha"]);
    assert_eq!(out["ok"], true, "{out}");
    assert_eq!(wl(&f.main, &["retire", "alpha"])["ok"], true, "retiring twice is harmless");
    assert!(!git(&f.main, &["branch", "--list", "alpha"]).contains("alpha"));
    let next = wl(&f.main, &["begin", "beta", "--request", "r"]);
    assert_eq!(next["worktree"], first["worktree"]);
    assert_eq!(wl(&f.main, &["status"])["tasks"].as_array().unwrap().len(), 1);
}

#[test]
fn retire_refuses_to_lose_work_or_a_held_task() {
    let f = fixture();
    let out = wl(&f.main, &["begin", "alpha", "--request", "r"]);
    let worktree = PathBuf::from(out["worktree"].as_str().unwrap());

    std::fs::write(worktree.join("new.txt"), "n\n").unwrap();
    let refused = wl(&f.main, &["retire", "alpha"]);
    assert!(refused["error"].as_str().unwrap().contains("untracked"), "{refused}");

    git(&worktree, &["add", "new.txt"]);
    git(&worktree, &["commit", "-q", "-m", "work"]);
    let refused = wl(&f.main, &["retire", "alpha"]);
    assert!(refused["error"].as_str().unwrap().contains("not in refs/remotes/origin/HEAD"), "{refused}");

    git(&worktree, &["push", "-q", "origin", "HEAD:main"]);
    git(&f.main, &["fetch", "-q", "origin"]);
    wl(&f.main, &["hold", "alpha", "--reason", "keep"]);
    assert_eq!(wl(&f.main, &["retire", "alpha"])["ok"], false);
    wl(&f.main, &["release-hold", "alpha", "--decision", "d"]);
    assert_eq!(wl(&f.main, &["retire", "alpha"])["ok"], true, "published work can be retired");
}

#[test]
fn commands_run_from_inside_a_slot_find_the_same_tasks() {
    let f = fixture();
    let out = wl(&f.main, &["begin", "alpha", "--request", "r"]);
    let worktree = PathBuf::from(out["worktree"].as_str().unwrap());
    let inside = wl(&worktree, &["status", "alpha"]);
    assert_eq!(inside["tasks"][0]["worktree"], out["worktree"], "{inside}");
    assert_eq!(wl(&worktree, &["hold", "alpha", "--reason", "x"])["ok"], true);
    assert_eq!(wl(&f.main, &["status", "alpha"])["tasks"][0]["hold"], "x");
}

#[test]
fn retire_keeps_the_declared_slot_cache_and_removes_other_ignored_files() {
    let f = fixture();
    let first = wl(&f.main, &["begin", "alpha", "--request", "r"]);
    let worktree = PathBuf::from(first["worktree"].as_str().unwrap());
    std::fs::write(worktree.join(".gitignore"), "target/\nscratch/\n").unwrap();
    std::fs::write(worktree.join(".lifecycle.toml"), "slot_cache = [\"target\"]\n").unwrap();
    git(&worktree, &["add", ".gitignore", ".lifecycle.toml"]);
    git(&worktree, &["commit", "-q", "-m", "declare the cache"]);
    git(&worktree, &["push", "-q", "origin", "HEAD:main"]);
    git(&f.main, &["fetch", "-q", "origin"]);
    for dir in ["target", "scratch"] {
        std::fs::create_dir_all(worktree.join(dir)).unwrap();
        std::fs::write(worktree.join(dir).join("out"), "x").unwrap();
    }
    assert_eq!(wl(&f.main, &["retire", "alpha"])["ok"], true);
    assert!(worktree.join("target/out").exists(), "the slot cache stays");
    assert!(!worktree.join("scratch").exists(), "other ignored files go");
}

/// Git Bash sets `EXEPATH` to its `bin` directory. `wl` must not take that for the Git root, or gix launches git to find it.
#[cfg(windows)]
#[test]
fn an_exepath_that_is_not_the_git_root_is_replaced_from_the_cache() {
    let f = fixture();
    let local = f.main.parent().unwrap().join("local");
    let run = || {
        let out = Command::new(env!("CARGO_BIN_EXE_wl")).arg("--repo").arg(&f.main).arg("status")
            .env("EXEPATH", f.main.parent().unwrap()).env("LOCALAPPDATA", &local).env("WL_TRACE", "1")
            .output().unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stderr).unwrap()
    };
    let first = run();
    assert!(first.contains("git root not cached"), "{first}");
    let second = run();
    assert!(second.contains("git root from cache"), "{second}");
}
