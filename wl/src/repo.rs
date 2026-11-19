use crate::error::{Error, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
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
