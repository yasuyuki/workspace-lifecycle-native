use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// Gives git, and the git that `wl` starts, the same configuration: no system or user file. Otherwise the machine's
/// files reach only the test's own git, since `wl` runs with a home of its own, and a system `core.autocrlf=true`
/// (Git for Windows' default) makes a slot that `wl` checked out look modified.
fn same_git_config(command: &mut Command) -> &mut Command {
    command.env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" })
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = same_git_config(&mut Command::new("git")).arg("-C").arg(dir).args(args)
        .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t")
        .output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn wl(repo: &Path, args: &[&str]) -> Value {
    let home = repo.parent().unwrap().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let out = same_git_config(&mut Command::new(env!("CARGO_BIN_EXE_wl"))).arg("--repo").arg(repo).args(args)
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
fn begin_reuses_the_slot_retired_earliest() {
    let f = fixture();
    let alpha = wl(&f.main, &["begin", "alpha", "--request", "r"]);
    assert_eq!(alpha["ok"], true, "{alpha}");
    wait_for_free_slot(&f.main);
    let beta = wl(&f.main, &["begin", "beta", "--request", "r"]);
    assert_eq!(beta["ok"], true, "{beta}");
    assert_ne!(alpha["worktree"], beta["worktree"]);
    wait_for_free_slot(&f.main);

    // Retire in reverse slot-number order. A prepared spare has no retirement yet.
    assert_eq!(wl(&f.main, &["retire", "beta"])["ok"], true);
    assert_eq!(wl(&f.main, &["retire", "alpha"])["ok"], true);
    let gamma = wl(&f.main, &["begin", "gamma", "--request", "r"]);
    assert_eq!(gamma["worktree"], beta["worktree"], "{gamma}");

    assert_eq!(wl(&f.main, &["retire", "gamma"])["ok"], true);
    let delta = wl(&f.main, &["begin", "delta", "--request", "r"]);
    assert_eq!(delta["worktree"], alpha["worktree"], "{delta}");
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
fn retire_abandon_ends_an_unintegrated_task_only_when_the_remote_topic_keeps_its_commits() {
    let f = fixture();
    let out = wl(&f.main, &["begin", "alpha", "--request", "r"]);
    let worktree = PathBuf::from(out["worktree"].as_str().unwrap());
    std::fs::write(worktree.join("new.txt"), "n\n").unwrap();
    git(&worktree, &["add", "new.txt"]);
    git(&worktree, &["commit", "-q", "-m", "work"]);
    let tip = git(&worktree, &["rev-parse", "HEAD"]);
    let main_before = git(&f.origin, &["rev-parse", "main"]);

    let refused = wl(&f.main, &["retire", "alpha", "--abandon"]);
    assert!(refused["error"].as_str().unwrap().contains("--decision"), "{refused}");
    let refused = wl(&f.main, &["retire", "alpha", "--abandon", "--decision", "d"]);
    assert!(refused["error"].as_str().unwrap().contains("push the topic"), "{refused}");
    assert_eq!(git(&worktree, &["rev-parse", "HEAD"]), tip, "a refused abandonment keeps the checkout");

    git(&worktree, &["push", "-q", "origin", "HEAD:alpha"]);
    wl(&f.main, &["hold", "alpha", "--reason", "keep"]);
    assert_eq!(wl(&f.main, &["retire", "alpha", "--abandon", "--decision", "d"])["ok"], false, "a held task stays");
    wl(&f.main, &["release-hold", "alpha", "--decision", "d"]);
    let refused = wl(&f.main, &["retire", "alpha"]);
    assert!(refused["error"].as_str().unwrap().contains("not in refs/remotes/origin/HEAD"), "{refused}");

    let out = wl(&f.main, &["retire", "alpha", "--abandon", "--decision", "https://example.invalid/decision"]);
    assert_eq!(out["ok"], true, "{out}");
    assert_eq!(out["abandoned"], true, "{out}");
    assert_eq!(out["decision"], "https://example.invalid/decision", "{out}");
    assert_eq!(git(&f.origin, &["rev-parse", "alpha"]), tip, "the remote topic keeps the commits");
    assert_eq!(git(&f.origin, &["rev-parse", "main"]), main_before, "abandoning neither merges nor pushes the target");
    assert!(wl(&f.main, &["status"])["tasks"].as_array().unwrap().is_empty());
}

#[test]
fn status_reports_work_that_no_task_owns() {
    let f = fixture();
    let slot = PathBuf::from(wl(&f.main, &["begin", "alpha", "--request", "r"])["worktree"].as_str().unwrap().to_owned());
    assert_eq!(wl(&f.main, &["status"])["unmanaged"], serde_json::json!([]), "slots and a clean main checkout are managed");

    // Work begun by hand in the main checkout, as legacy workspace-lifecycle#37 was, and a worktree wl did not make.
    git(&f.main, &["switch", "-q", "-c", "by-hand"]);
    std::fs::write(f.main.join("f0.txt"), "changed\n").unwrap();
    let foreign = f.main.parent().unwrap().join("foreign");
    git(&f.main, &["worktree", "add", "-q", "-b", "elsewhere", foreign.to_str().unwrap()]);

    let status = wl(&slot, &["status"]);
    let unmanaged = status["unmanaged"].as_array().unwrap_or_else(|| panic!("{status}"));
    assert_eq!(unmanaged.len(), 2, "{status}");
    assert_eq!(unmanaged[0]["branch"], "by-hand");
    assert_eq!(unmanaged[0]["changes"], true);
    assert_eq!(PathBuf::from(unmanaged[0]["path"].as_str().unwrap()), f.main);
    assert_eq!(unmanaged[1]["branch"], "elsewhere");
    assert_eq!(PathBuf::from(unmanaged[1]["path"].as_str().unwrap()).file_name().unwrap(), "foreign");
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

fn fixture_with_files(count: usize) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin.git");
    let seed = dir.path().join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "-q", "-b", "main"]);
    for i in 0..count {
        std::fs::write(seed.join(format!("f{i}.txt")), format!("{i}\n")).unwrap();
    }
    git(&seed, &["add", "-A"]);
    git(&seed, &["commit", "-q", "-m", "seed"]);
    git(dir.path(), &["clone", "-q", "--bare", seed.to_str().unwrap(), origin.to_str().unwrap()]);
    let main = dir.path().join("product");
    git(dir.path(), &["clone", "-q", origin.to_str().unwrap(), main.to_str().unwrap()]);
    Fixture { _dir: dir, main, origin }
}

fn wait_until_quiet(repo: &Path) -> Value {
    let start = Instant::now();
    loop {
        let status = wl(repo, &["status"]);
        if status["background"] == false {
            return status;
        }
        assert!(start.elapsed() < Duration::from_secs(60), "background work did not finish: {status}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn claim_files(repo: &Path) -> Vec<(String, String)> {
    let dir = repo.join(".git/wl/slots");
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out = Vec::new();
    for entry in entries {
        let path = entry.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) == Some("claim") {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let text = std::fs::read_to_string(&path).unwrap();
            out.push((name, text));
        }
    }
    out.sort();
    out
}

#[test]
fn begin_with_invalid_base_does_not_leak_a_slot_claim() {
    let f = fixture();
    let first = wl(&f.main, &["begin", "alpha", "--request", "r1"]);
    wait_until_quiet(&f.main);
    assert_eq!(wl(&f.main, &["retire", "alpha"])["ok"], true);
    wait_until_quiet(&f.main);

    let failed = wl(&f.main, &["begin", "beta", "--request", "r2", "--base", "refs/heads/no-such-ref"]);
    assert_eq!(failed["ok"], false, "{failed}");
    assert_eq!(claim_files(&f.main), Vec::<(String, String)>::new());
    let status = wait_until_quiet(&f.main);
    assert!(status["slots"]["free"].as_u64().unwrap() >= 1, "{status}");

    let gamma = wl(&f.main, &["begin", "gamma", "--request", "r3"]);
    assert_eq!(gamma["ok"], true, "{gamma}");
    assert_eq!(gamma["worktree"], first["worktree"]);
}

#[test]
fn stale_prepare_and_orphan_task_claims_are_reclaimed_by_housekeeping() {
    let f = fixture();
    let first = wl(&f.main, &["begin", "alpha", "--request", "r1"]);
    wait_until_quiet(&f.main);
    assert_eq!(wl(&f.main, &["retire", "alpha"])["ok"], true);
    wait_until_quiet(&f.main);

    std::fs::write(f.main.join(".git/wl/slots/1.claim"), "#prepare").unwrap();
    let status = wait_until_quiet(&f.main);
    assert!(status["slots"]["free"].as_u64().unwrap() >= 1, "{status}");
    assert!(claim_files(&f.main).is_empty(), "{:?}", claim_files(&f.main));

    assert_eq!(wl(&f.main, &["begin", "alpha", "--request", "r2"])["ok"], true);
    wait_until_quiet(&f.main);
    std::fs::remove_file(f.main.join(".git/wl/tasks/alpha.json")).unwrap();

    let status = wait_until_quiet(&f.main);
    assert!(status["tasks"].as_array().unwrap().is_empty(), "{status}");
    assert!(claim_files(&f.main).is_empty(), "{:?}", claim_files(&f.main));
    let beta = wl(&f.main, &["begin", "beta", "--request", "r3"]);
    assert_eq!(beta["worktree"], first["worktree"]);
}

#[test]
fn concurrent_begins_of_the_same_task_claim_only_one_slot() {
    let f = fixture();
    assert_eq!(wl(&f.main, &["begin", "seed1", "--request", "r"])["ok"], true);
    wait_until_quiet(&f.main);
    assert_eq!(wl(&f.main, &["begin", "seed2", "--request", "r"])["ok"], true);
    wait_until_quiet(&f.main);
    assert_eq!(wl(&f.main, &["retire", "seed1"])["ok"], true);
    assert_eq!(wl(&f.main, &["retire", "seed2"])["ok"], true);
    wait_until_quiet(&f.main);

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let results: Vec<Value> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let main = &f.main;
                let barrier = barrier.clone();
                s.spawn(move || {
                    barrier.wait();
                    wl(main, &["begin", "alpha", "--request", "r"])
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(results[0]["ok"], true, "{results:?}");
    assert_eq!(results[1]["ok"], true, "{results:?}");
    assert_eq!(results[0]["worktree"], results[1]["worktree"]);
    wait_until_quiet(&f.main);
    assert_eq!(claim_files(&f.main).len(), 1, "{:?}", claim_files(&f.main));
    assert_eq!(wl(&f.main, &["retire", "alpha"])["ok"], true);
    wait_until_quiet(&f.main);
    assert!(claim_files(&f.main).is_empty(), "{:?}", claim_files(&f.main));
}

#[test]
fn retire_during_begin_does_not_leave_two_tasks_sharing_a_slot() {
    let f = fixture_with_files(200);
    let task_path = f.main.join(".git/wl/tasks/alpha.json");
    let main = f.main.clone();
    let handle = std::thread::spawn(move || wl(&main, &["begin", "alpha", "--request", "r1"]));
    let start = Instant::now();
    let mut retired_res = None;
    while start.elapsed() < Duration::from_secs(10) {
        if task_path.exists() {
            retired_res = Some(wl(&f.main, &["retire", "alpha"]));
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let begin_alpha = handle.join().unwrap();
    assert_eq!(begin_alpha["ok"], true, "{begin_alpha}");
    let retired = retired_res.unwrap_or_else(|| wl(&f.main, &["retire", "alpha"]));
    assert_eq!(retired["ok"], true, "{retired}");
    let beta = wl(&f.main, &["begin", "beta", "--request", "r2"]);
    assert_eq!(beta["ok"], true, "{beta}");

    let status = wait_until_quiet(&f.main);
    let tasks = status["tasks"].as_array().unwrap();
    let worktrees: Vec<&str> = tasks.iter().map(|t| t["worktree"].as_str().unwrap()).collect();
    let unique: std::collections::HashSet<&str> = worktrees.iter().copied().collect();
    assert_eq!(worktrees.len(), unique.len(), "two tasks share one slot: {status}");
    let expected_claims: Vec<(String, String)> = tasks
        .iter()
        .map(|t| {
            let slot = Path::new(t["worktree"].as_str().unwrap()).file_name().unwrap().to_string_lossy();
            (format!("{slot}.claim"), t["task"].as_str().unwrap().to_owned())
        })
        .collect();
    assert_eq!(claim_files(&f.main), expected_claims);
}
