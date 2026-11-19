use crate::error::{Error, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

static SET_EXEPATH: AtomicBool = AtomicBool::new(false);

/// gix finds Git for Windows' system configuration and attributes by launching git once per process,
/// unless `EXEPATH` names the Git root as it does inside Git Bash. Sets it from a per-user cache.
/// Must run before any thread starts.
pub fn prepare_environment() {
    #[cfg(windows)]
    {
        if std::env::var_os("EXEPATH").is_some() {
            return;
        }
        let Some(cache) = std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("wl").join("git-root")) else { return };
        let usable = |root: &Path| ["clangarm64", "ucrt64", "mingw64", "mingw32"].iter().filter(|c| root.join(c).is_dir()).count() == 1;
        let cached = std::fs::read_to_string(&cache).ok().map(|s| PathBuf::from(s.trim())).filter(|r| usable(r));
        let root = cached.or_else(|| {
            let exec_path = Command::new("git").arg("--exec-path").stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
            let exec_path = PathBuf::from(String::from_utf8(exec_path.stdout).ok()?.trim());
            let root = exec_path.ancestors().find(|p| p.file_name().is_some_and(|n| n == "libexec"))?.parent()?.parent()?.to_path_buf();
            if !usable(&root) {
                return None;
            }
            let _ = std::fs::create_dir_all(cache.parent()?);
            let _ = std::fs::write(&cache, root.to_string_lossy().as_bytes());
            Some(root)
        });
        if let Some(root) = root {
            // SAFETY: called at the start of main, before any other thread exists.
            unsafe { std::env::set_var("EXEPATH", root) };
            SET_EXEPATH.store(true, Ordering::Relaxed);
        }
    }
}

/// Opens a repository with the configuration files Git reads, but without configuration from environment variables.
pub fn open(path: &Path) -> Result<gix::Repository> {
    let mut permissions = gix::open::Permissions::default();
    permissions.config = gix::open::permissions::Config {
        git_binary: false,
        system: true,
        git: true,
        user: true,
        env: false,
        includes: true,
    };
    Ok(gix::open_opts(path, gix::open::Options::default().permissions(permissions))?)
}

pub struct Layout {
    pub common_dir: PathBuf,
    pub main_worktree: PathBuf,
}

pub fn layout(repo: &gix::Repository) -> Result<Layout> {
    let common_dir = gix::path::realpath(repo.common_dir())
        .map_err(|e| Error::new(format!("cannot resolve the Git common dir: {e}")))?;
    let main_worktree = common_dir
        .parent()
        .filter(|_| common_dir.file_name().is_some_and(|n| n == ".git"))
        .ok_or_else(|| Error::new("wl needs a repository whose main worktree holds the .git directory"))?
        .to_path_buf();
    Ok(Layout { common_dir, main_worktree })
}

/// The commit a new task starts from: `spec`, or the remote default branch as last fetched.
pub fn resolve_base(repo: &gix::Repository, spec: Option<&str>) -> Result<gix::ObjectId> {
    match spec {
        Some(spec) => Ok(repo.rev_parse_single(spec)?.object()?.peel_to_commit()?.id),
        None => {
            let mut head = repo
                .find_reference("refs/remotes/origin/HEAD")
                .map_err(|_| Error::new("origin/HEAD is not set; run `git remote set-head origin --auto` once or pass --base"))?;
            Ok(head.peel_to_id()?.detach())
        }
    }
}

pub fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(dir).args(args).stdin(Stdio::null());
    if SET_EXEPATH.load(Ordering::Relaxed) {
        command.env_remove("EXEPATH");
    }
    let output = command
        .output()
        .map_err(|e| Error::new(format!("cannot start git: {e}")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim_end().to_owned())
    } else {
        Err(Error::new(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

/// Points a clean checkout that sits on `base` at a new branch `branch`, without touching its files.
pub fn start_branch_in_place(slot: &gix::Repository, branch: &str, base: gix::ObjectId) -> Result<()> {
    use gix::refs::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};
    use gix::refs::Target;
    let name: gix::refs::FullName = format!("refs/heads/{branch}").try_into()?;
    let log = |message: &str| LogChange { mode: RefLog::AndReference, force_create_reflog: false, message: message.into() };
    slot.edit_references([
        RefEdit {
            change: Change::Update { log: log("wl begin"), expected: PreviousValue::MustNotExist, new: Target::Object(base) },
            name: name.clone(),
            deref: false,
        },
        RefEdit {
            change: Change::Update { log: log("wl begin: checkout"), expected: PreviousValue::Any, new: Target::Symbolic(name) },
            name: "HEAD".try_into()?,
            deref: false,
        },
    ])?;
    Ok(())
}
