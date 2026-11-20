use crate::error::{Error, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::ffi::OsString;
use std::sync::OnceLock;

/// The caller's `EXEPATH`, kept when `wl` replaced it, so that the processes `wl` starts see the caller's environment.
static CALLER_EXEPATH: OnceLock<Option<OsString>> = OnceLock::new();

/// gix finds Git for Windows' system configuration and attributes by launching git once per process,
/// unless `EXEPATH` names the Git root. Git Bash sets `EXEPATH` to its `bin` directory, which is not the root,
/// so a value that does not look like the root is replaced from a per-user cache. Must run before any thread starts.
pub fn prepare_environment() {
    #[cfg(windows)]
    {
        let usable = |root: &Path| ["clangarm64", "ucrt64", "mingw64", "mingw32"].iter().filter(|c| root.join(c).is_dir()).count() == 1;
        let caller = std::env::var_os("EXEPATH");
        if caller.as_ref().is_some_and(|v| usable(Path::new(v))) {
            return;
        }
        let Some(cache) = std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("wl").join("git-root")) else { return };
        let cached = std::fs::read_to_string(&cache).ok().map(|s| PathBuf::from(s.trim())).filter(|r| usable(r));
        crate::trace::point(if cached.is_some() { "git root from cache" } else { "git root not cached" });
        let root = cached.or_else(|| {
            let exec_path = command("git").arg("--exec-path").stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
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
            let _ = CALLER_EXEPATH.set(caller);
            unsafe { std::env::set_var("EXEPATH", root) };
        }
    }
}

/// Opens a repository for reading refs and its layout; no configuration file is read.
pub fn open_refs_only(path: &Path) -> Result<gix::Repository> {
    let repo = gix::open_opts(path, gix::open::Options::isolated())?;
    crate::trace::point("open refs only");
    Ok(repo)
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
    let repo = gix::open_opts(path, gix::open::Options::default().permissions(permissions))?;
    crate::trace::point("open");
    Ok(repo)
}

pub struct Layout {
    pub common_dir: PathBuf,
    pub main_worktree: PathBuf,
}

/// Finds the Git common dir and the main worktree from the files Git keeps, without opening the repository.
/// `path` is the main worktree or a linked worktree such as a slot.
pub fn layout(path: &Path) -> Result<Layout> {
    let root = std::path::absolute(path).map_err(|e| Error::io(path, e))?;
    let dot_git = root.join(".git");
    let meta = std::fs::metadata(&dot_git).map_err(|_| Error::new(format!("{} is not the top of a Git checkout", root.display())))?;
    let common_dir = if meta.is_dir() {
        dot_git
    } else {
        let text = std::fs::read_to_string(&dot_git).map_err(|e| Error::io(&dot_git, e))?;
        let git_dir = text.trim().strip_prefix("gitdir:").map(str::trim).ok_or_else(|| Error::new(format!("{} is not a gitdir file", dot_git.display())))?;
        let git_dir = lexical(&root.join(git_dir));
        match std::fs::read_to_string(git_dir.join("commondir")) {
            Ok(common) => lexical(&git_dir.join(common.trim())),
            Err(_) => git_dir,
        }
    };
    let main_worktree = common_dir
        .parent()
        .filter(|_| common_dir.file_name().is_some_and(|n| n == ".git"))
        .ok_or_else(|| Error::new("wl needs a repository whose main worktree holds the .git directory"))?
        .to_path_buf();
    Ok(Layout { common_dir, main_worktree })
}

/// Removes `.` and `..` components without touching the file system.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
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

/// A process without a console, such as the detached finish worker, gives every console program it starts
/// a new console, and Windows shows that console as a window until the program exits. Those programs get
/// a console without a window instead. A process with a console keeps sharing it with its children.
#[cfg(windows)]
fn hide_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // SAFETY: GetConsoleWindow only reads which console this process is attached to.
    if unsafe { windows_sys::Win32::System::Console::GetConsoleWindow() }.is_null() {
        command.creation_flags(CREATE_NO_WINDOW);
    }
}

/// Starts a program other than `wl` with the caller's environment.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    #[cfg(windows)]
    hide_console(&mut command);
    match CALLER_EXEPATH.get() {
        Some(Some(value)) => {
            command.env("EXEPATH", value);
        }
        Some(None) => {
            command.env_remove("EXEPATH");
        }
        None => {}
    }
    command
}

/// Whether `ancestor` is `descendant` or one of its ancestors, as `git merge-base --is-ancestor` answers it.
pub fn is_ancestor(repo: &gix::Repository, ancestor: gix::ObjectId, descendant: gix::ObjectId) -> Result<bool> {
    if ancestor == descendant {
        return Ok(true);
    }
    Ok(repo.merge_base(ancestor, descendant)?.is_some_and(|base| base.detach() == ancestor))
}

/// The object a reference points at after peeling, or `None` when the reference does not exist.
pub fn resolve(repo: &gix::Repository, name: &str) -> Result<Option<gix::ObjectId>> {
    match repo.try_find_reference(name)? {
        Some(mut reference) => Ok(Some(reference.peel_to_id()?.detach())),
        None => Ok(None),
    }
}

pub fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let mut command = command("git");
    // Automatic gc and maintenance would hold ref locks that a foreground command then waits for.
    command.args(["-c", "gc.auto=0", "-c", "maintenance.auto=false", "-C"]).arg(dir).args(args).stdin(Stdio::null());
    crate::trace::point(&format!("git {} start", args.first().unwrap_or(&"")));
    let output = command
        .output()
        .map_err(|e| Error::new(format!("cannot start git: {e}")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim_end().to_owned())
    } else {
        // A merge reports its conflicts on stdout.
        Err(Error::new(format!(
            "git {} failed: {} {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim(),
            String::from_utf8_lossy(&output.stdout).trim()
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

/// Whether the checkout differs from its HEAD in the index or in tracked files; untracked files do not count.
/// Same as gix's `is_dirty`, but on one thread: starting a thread per CPU costs more than checking a slot.
pub fn has_changes(repo: &gix::Repository) -> Result<bool> {
    let head_tree = repo.head_tree_id_or_empty()?;
    let index = repo.index_or_empty()?;
    let mut pathspec = repo.pathspec(
        false,
        None::<&str>,
        false,
        &gix::index::State::new(repo.object_hash()),
        gix::worktree::stack::state::attributes::Source::IdMapping,
    )?;
    let mut staged = false;
    repo.tree_index_status(
        &head_tree,
        &index,
        Some(&mut pathspec),
        gix::status::tree_index::TrackRenames::Disabled,
        |_, _, _| {
            staged = true;
            Ok(std::ops::ControlFlow::Break(()))
        },
    )?;
    if staged {
        return Ok(true);
    }
    let modified = repo
        .status(gix::progress::Discard)?
        .index_worktree_rewrites(None)
        .index_worktree_submodules(gix::status::Submodule::AsConfigured { check_dirty: true })
        .index_worktree_options_mut(|options| {
            options.dirwalk_options = None;
            options.thread_limit = Some(1);
        })
        .into_index_worktree_iter(Vec::new())?
        .take_while(std::result::Result::is_ok)
        .next()
        .is_some();
    Ok(modified)
}
