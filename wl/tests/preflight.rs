//! `wl preflight` as a caller sees it: one decision on stdout and its exit status.
use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

fn git(args: &[&str]) -> String {
    let out = Command::new("git").args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

fn preflight(repo: &Path, extra: &[&str]) -> (i32, Value) {
    let out: Output = Command::new(env!("CARGO_BIN_EXE_wl")).arg("preflight").arg(repo).args(extra).output().unwrap();
    let decision = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("not json: {}", String::from_utf8_lossy(&out.stdout)));
    (out.status.code().unwrap(), decision)
}

#[test]
fn a_repository_without_a_remote_asks_and_a_hold_holds() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_str().unwrap();
    git(&["init", "-q", d]);
    git(&["-C", d, "symbolic-ref", "HEAD", "refs/heads/feature"]);
    let (code, decision) = preflight(dir.path(), &[]);
    assert_eq!((code, decision["decision"].as_str()), (11, Some("ask")), "{decision}");
    let (code, decision) = preflight(dir.path(), &["--user-intent", "hold"]);
    assert_eq!((code, decision["decision"].as_str()), (10, Some("hold")), "{decision}");
    let out = Command::new(env!("CARGO_BIN_EXE_wl")).args(["preflight", d, "--user-intent", "maybe"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2), "invalid usage exits 2");
    let help = Command::new(env!("CARGO_BIN_EXE_wl")).args(["preflight", "--help"]).output().unwrap();
    let text = String::from_utf8(help.stdout).unwrap();
    assert_eq!(help.status.code(), Some(0));
    assert!(text.contains("push 0, hold 10") && text.contains("--policy"), "{text}");
}

/// Runs `line` in cmd.exe on Windows and /bin/sh elsewhere.
fn shell(line: &str) -> Output {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        Command::new("cmd").raw_arg(format!("/S /C \"{line}\"")).output().unwrap()
    }
    #[cfg(not(windows))]
    {
        Command::new("sh").arg("-c").arg(line).output().unwrap()
    }
}

#[test]
fn a_shell_pushes_only_after_a_push_decision() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    let bare = dir.path().join("remote.git");
    let (w, b) = (work.to_str().unwrap(), bare.to_str().unwrap());
    git(&["init", "-q", "--bare", b]);
    git(&["init", "-q", w]);
    git(&["-C", w, "symbolic-ref", "HEAD", "refs/heads/feature"]);
    git(&["-C", w, "-c", "user.name=Test", "-c", "user.email=test@example.invalid", "commit", "-q", "--allow-empty", "-m", "work"]);
    git(&["-C", w, "remote", "add", "origin", b]);
    let wl = env!("CARGO_BIN_EXE_wl");
    for (extra, decision, code) in [
        (&[][..], "ask", 11),
        (&["--user-intent", "hold"][..], "hold", 10),
        (&["--temporary"][..], "hold", 10),
        (&["--user-intent", "push"][..], "push", 0),
    ] {
        let line = format!("\"{wl}\" preflight \"{w}\" {} && git -C \"{w}\" push -q origin HEAD:refs/heads/feature", extra.join(" "));
        let out = shell(&line);
        let printed: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!((out.status.code().unwrap(), printed["decision"].as_str().unwrap()), (code, decision), "{line}");
        let pushed = git(&["--git-dir", b, "for-each-ref", "--format=%(refname)"]);
        let expected: Vec<&str> = if decision == "push" { vec!["refs/heads/feature"] } else { vec![] };
        assert_eq!(pushed.split_whitespace().collect::<Vec<_>>(), expected, "{decision}");
    }
}
