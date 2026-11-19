use crate::error::{Error, Result};
use crate::model::{HoldRecord, PREPARING, SlotRecord, TaskName, TaskRecord, TaskStage, VERSION};
use crate::repo::{self, Layout};
use crate::store::Store;
use crate::Args;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub(crate) struct Workspace {
    pub(crate) layout: Layout,
    pub(crate) store: Store,
}

impl Workspace {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let layout = repo::layout(path)?;
        let store = Store::new(&layout.common_dir);
        Ok(Self { layout, store })
    }

    pub(crate) fn slots_root(&self) -> PathBuf {
        let name = self.layout.main_worktree.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.layout.main_worktree.with_file_name(format!("{name}.slots"))
    }

    pub(crate) fn slot_numbers(&self, suffix: &str) -> Result<Vec<u32>> {
        let mut numbers: Vec<u32> = Store::list(&self.store.slots_dir(), suffix)?.iter().filter_map(|n| n.parse().ok()).collect();
        numbers.sort_unstable();
        Ok(numbers)
    }

    pub(crate) fn free_slots(&self) -> Result<Vec<u32>> {
        let claimed = self.slot_numbers(".claim")?;
        Ok(self.slot_numbers(".json")?.into_iter().filter(|n| !claimed.contains(n)).collect())
    }

    pub(crate) fn slot_path(&self, slot: u32) -> Result<PathBuf> {
        Ok(Store::read_json::<SlotRecord>(&self.store.slot(slot))?
            .map(|record| record.path)
            .unwrap_or_else(|| self.slots_root().join(slot.to_string())))
    }

    pub(crate) fn task(&self, task: &TaskName) -> Result<TaskRecord> {
        Store::read_json(&self.store.task(task.as_str()))?.ok_or_else(|| Error::new(format!("no task {task}")))
    }

    /// Claims a slot for `owner`: one it already claimed, else a free one, else a new number.
    pub(crate) fn claim_slot(&self, owner: &str) -> Result<u32> {
        let claims = self.slot_numbers(".claim")?;
        for &n in &claims {
            if Store::read_text(&self.store.claim(n))?.as_deref() == Some(owner) {
                return Ok(n);
            }
        }
        let slots = self.slot_numbers(".json")?;
        for &n in &slots {
            if !claims.contains(&n) && Store::create_new(&self.store.claim(n), owner.as_bytes())? {
                return Ok(n);
            }
        }
        self.claim_new_slot(owner)
    }

    /// Claims a slot number that has no checkout yet.
    pub(crate) fn claim_new_slot(&self, owner: &str) -> Result<u32> {
        let mut n = self.slot_numbers(".json")?.iter().chain(&self.slot_numbers(".claim")?).max().map_or(1, |m| m + 1);
        while !Store::create_new(&self.store.claim(n), owner.as_bytes())? {
            n += 1;
        }
        Ok(n)
    }

    /// Makes sure slot `n` has a checkout, creating one detached at `base` when it has none.
    pub(crate) fn ensure_slot_checkout(&self, n: u32, base: gix::ObjectId) -> Result<PathBuf> {
        if let Some(record) = Store::read_json::<SlotRecord>(&self.store.slot(n))? {
            return Ok(record.path);
        }
        let path = self.slots_root().join(n.to_string());
        if !path.join(".git").exists() {
            let target = path.to_string_lossy().into_owned();
            repo::git(&self.layout.main_worktree, &["worktree", "add", "--quiet", "--detach", &target, &base.to_string()])?;
        }
        Store::write_json(&self.store.slot(n), &SlotRecord { version: VERSION, slot: n, path: path.clone() })?;
        Ok(path)
    }

    pub(crate) fn report(&self, record: &TaskRecord) -> Result<Value> {
        let worktree = self.slot_path(record.slot)?;
        let hold = Store::read_json::<HoldRecord>(&self.store.hold(record.task.as_str()))?.map(|h| h.reason);
        Ok(json!({
            "task": record.task,
            "request": record.request,
            "branch": record.branch,
            "base": record.base,
            "stage": record.stage,
            "target": record.target,
            "worktree": worktree,
            "hold": hold,
            "finish": crate::finish::finish_report(self, &record.task)?,
        }))
    }
}

pub(crate) fn task_name(args: &mut Args) -> Result<TaskName> {
    TaskName::try_from(args.positional("TASK")?).map_err(Error::new)
}

pub fn begin(path: &Path, mut args: Args) -> Result<Value> {
    let task = task_name(&mut args)?;
    let request = args.required("request")?;
    let branch = args.option("branch")?.unwrap_or_else(|| task.to_string());
    let target = args.option("target")?;
    let base_spec = match (args.option("base")?, &target) {
        (Some(base), _) => Some(base),
        (None, Some(target)) => Some(format!("refs/remotes/origin/{target}")),
        (None, None) => None,
    };
    args.finish()?;
    gix::refs::FullName::try_from(format!("refs/heads/{branch}"))?;
    if let Some(target) = &target {
        gix::refs::FullName::try_from(format!("refs/heads/{target}"))?;
    }

    let ws = Workspace::open(path)?;
    crate::finish::housekeeping(&ws)?;
    let task_path = ws.store.task(task.as_str());
    let mut opened_slot = None;
    let mut record = match Store::read_json::<TaskRecord>(&task_path)? {
        Some(existing) => existing,
        None => {
            // A begin of the same task finds the claim it or an earlier attempt made, so both use one slot.
            let slot = ws.claim_slot(task.as_str())?;
            crate::trace::point("slot claimed");
            let base = match Store::read_json::<SlotRecord>(&ws.store.slot(slot))? {
                Some(existing) => {
                    let slot_repo = repo::open(&existing.path)?;
                    let base = repo::resolve_base(&slot_repo, base_spec.as_deref())?;
                    opened_slot = Some(slot_repo);
                    base
                }
                None => repo::resolve_base(&repo::open_refs_only(&ws.layout.main_worktree)?, base_spec.as_deref())?,
            };
            let fresh = TaskRecord {
                version: VERSION,
                task: task.clone(),
                request: request.clone(),
                branch: branch.clone(),
                base: base.to_string(),
                slot,
                stage: TaskStage::Claimed,
                target: target.clone(),
            };
            if Store::create_new_json(&task_path, &fresh)? {
                fresh
            } else {
                Store::read_json(&task_path)?.ok_or_else(|| Error::new(format!("task {task} is being created by another begin")))?
            }
        }
    };
    if record.request != request || record.branch != branch || record.target != target {
        return Err(Error::new(format!("task {task} already exists with another request or branch")));
    }
    if record.stage == TaskStage::Ready {
        return ws.report(&record);
    }
    let base: gix::ObjectId = record.base.parse().map_err(|e| Error::new(format!("recorded base: {e}")))?;
    let worktree = ws.ensure_slot_checkout(record.slot, base)?;
    let slot_repo = match opened_slot {
        Some(slot_repo) => slot_repo,
        None => repo::open(&worktree)?,
    };
    switch_slot(slot_repo, &worktree, &record.branch, base)?;
    record.stage = TaskStage::Ready;
    Store::write_json(&task_path, &record)?;

    crate::trace::point("task ready");
    if ws.free_slots()?.is_empty() {
        spawn_detached(&ws.layout.main_worktree, &["__prepare-slot"]);
    }
    ws.report(&record)
}

/// Puts the slot checkout on a new branch at `base`. A clean slot already on `base` only needs its refs moved.
fn switch_slot(mut slot: gix::Repository, worktree: &Path, branch: &str, base: gix::ObjectId) -> Result<()> {
    slot.committer_or_set_generic_fallback()?;
    let head = slot.head()?;
    if head.referent_name().is_some_and(|name| name.as_bstr() == format!("refs/heads/{branch}").as_str()) {
        return Ok(());
    }
    let dirty = repo::has_changes(&slot)?;
    crate::trace::point("slot dirty check");
    if dirty {
        return Err(Error::new(format!("slot {} has uncommitted changes", worktree.display())));
    }
    let branch_exists = slot.try_find_reference(format!("refs/heads/{branch}").as_str())?.is_some();
    if !branch_exists && head.id().map(|id| id.detach()) == Some(base) {
        let moved = repo::start_branch_in_place(&slot, branch, base);
        crate::trace::point("refs moved in place");
        return moved;
    }
    if branch_exists {
        repo::git(worktree, &["switch", "--quiet", branch])?;
    } else {
        repo::git(worktree, &["switch", "--quiet", "--create", branch, &base.to_string()])?;
    }
    Ok(())
}

/// Returns the task's slot to the free pool. P1 refuses anything that retiring would lose: changes in the
/// checkout, untracked files, and commits that are neither the base nor in the remote default branch.
pub fn retire(path: &Path, mut args: Args) -> Result<Value> {
    let task = task_name(&mut args)?;
    args.finish()?;
    let ws = Workspace::open(path)?;
    crate::finish::housekeeping(&ws)?;
    retire_task(&ws, &task, None)?;
    Ok(json!({ "task": task, "retired": true }))
}

/// Returns the task's slot to the free pool. Refuses anything that retiring would lose: changes in the checkout,
/// untracked files, commits that are neither the base nor in the remote default branch, a finish still running,
/// and with `expected_head` a checkout that moved past the commit a finish integrated.
pub(crate) fn retire_task(ws: &Workspace, task: &TaskName, expected_head: Option<&str>) -> Result<()> {
    let task_path = ws.store.task(task.as_str());
    let Some(record) = Store::read_json::<TaskRecord>(&task_path)? else {
        return Ok(());
    };
    if Store::read_json::<HoldRecord>(&ws.store.hold(task.as_str()))?.is_some() {
        return Err(Error::new(format!("task {task} is held; release the hold first")));
    }
    if expected_head.is_none() && crate::finish::running(ws, task)? {
        return Err(Error::new(format!("task {task} has a finish in progress")));
    }
    let n = record.slot;
    let worktree = ws.slot_path(n)?;
    if worktree.join(".git").exists() {
        let untracked_or_changed = repo::git(&worktree, &["status", "--porcelain", "--untracked-files=all"])?;
        if !untracked_or_changed.is_empty() {
            return Err(Error::new(format!("task {task} has uncommitted or untracked files in {}", worktree.display())));
        }
        let branch_ref = format!("refs/heads/{}", record.branch);
        if let Ok(tip) = repo::git(&worktree, &["rev-parse", "--verify", "--quiet", &branch_ref]) {
            if let Some(expected) = expected_head {
                if tip != expected {
                    return Err(Error::new(format!("task {task} has commits after the integrated {expected}; retire it after the next finish")));
                }
            }
            let integrated_into = record.target.as_ref().map_or("refs/remotes/origin/HEAD".to_owned(), |t| format!("refs/remotes/origin/{t}"));
            let published = tip == record.base || repo::git(&worktree, &["merge-base", "--is-ancestor", &tip, &integrated_into]).is_ok();
            if !published {
                return Err(Error::new(format!("task {task} has commits that are not in {integrated_into}")));
            }
            repo::git(&worktree, &["switch", "--quiet", "--detach", &tip])?;
            repo::git(&worktree, &["branch", "--quiet", "--delete", "--force", &record.branch])?;
        }
        clear_ignored(&worktree)?;
    }
    Store::remove(&ws.store.hold(task.as_str()))?;
    Store::remove(&ws.store.acceptance(task.as_str()))?;
    Store::remove(&ws.store.progress(task.as_str()))?;
    let _ = std::fs::remove_dir(ws.store.finishes_dir().join(task.as_str()));
    Store::remove(&task_path)?;
    if Store::read_text(&ws.store.claim(n))?.as_deref() == Some(task.as_str()) {
        Store::remove(&ws.store.claim(n))?;
    }
    spawn_detached(&ws.layout.main_worktree, &["__prepare-slot"]);
    Ok(())
}

/// Removes the slot's ignored files except the slot cache its `.lifecycle.toml` declares (ADR 0015), so the next
/// task starts with only what the repository chose to keep.
fn clear_ignored(worktree: &Path) -> Result<()> {
    let slot = repo::open(worktree)?;
    let head = slot.head_id()?.detach();
    let cache = crate::config::repo_config(&slot, head)?.slot_cache;
    // A pathspec exclusion does not keep an ignored directory from `clean -X`; an un-ignoring pattern does.
    let mut args: Vec<String> = ["clean", "--quiet", "-d", "--force", "--force", "-X"].map(String::from).to_vec();
    for path in &cache {
        args.extend(["-e".into(), format!("!/{}", path.trim_matches('/'))]);
    }
    repo::git(worktree, &args.iter().map(String::as_str).collect::<Vec<_>>())?;
    Ok(())
}

pub fn status(path: &Path, mut args: Args) -> Result<Value> {
    let only = args.optional_positional().map(TaskName::try_from).transpose().map_err(Error::new)?;
    let only_requested = only.is_some();
    args.finish()?;
    let ws = Workspace::open(path)?;
    crate::finish::housekeeping(&ws)?;
    let names = match only {
        Some(task) => vec![task],
        None => Store::list(&ws.store.tasks_dir(), ".json")?
            .into_iter()
            .filter_map(|n| TaskName::try_from(n).ok())
            .collect(),
    };
    let mut tasks = Vec::new();
    for name in &names {
        match Store::read_json::<TaskRecord>(&ws.store.task(name.as_str()))? {
            Some(record) => tasks.push(ws.report(&record)?),
            None if only_requested => return Err(Error::new(format!("no task {name}"))),
            None => {}
        }
    }
    let total = ws.slot_numbers(".json")?.len();
    let workers = ws.store.prepare_lock().with_file_name("");
    let background = lock_held(&ws.store.prepare_lock())
        || Store::list(&workers, ".lock")?.iter().any(|name| lock_held(&workers.join(format!("{name}.lock"))));
    Ok(json!({ "tasks": tasks, "slots": { "total": total, "free": ws.free_slots()?.len() }, "background": background }))
}

pub(crate) fn lock_held(lock_path: &Path) -> bool {
    match std::fs::OpenOptions::new().write(true).open(lock_path) {
        Ok(file) => matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
        Err(_) => false,
    }
}

pub fn hold(path: &Path, mut args: Args) -> Result<Value> {
    let task = task_name(&mut args)?;
    let reason = args.required("reason")?;
    args.finish()?;
    let ws = Workspace::open(path)?;
    crate::finish::housekeeping(&ws)?;
    ws.task(&task)?;
    Store::write_json(&ws.store.hold(task.as_str()), &HoldRecord { version: VERSION, reason: reason.clone() })?;
    Ok(json!({ "task": task, "hold": reason }))
}

pub fn release_hold(path: &Path, mut args: Args) -> Result<Value> {
    let task = task_name(&mut args)?;
    let decision = args.required("decision")?;
    args.finish()?;
    let ws = Workspace::open(path)?;
    crate::finish::housekeeping(&ws)?;
    let hold_path = ws.store.hold(task.as_str());
    let hold = Store::read_json::<HoldRecord>(&hold_path)?.ok_or_else(|| Error::new(format!("task {task} has no hold")))?;
    Store::remove(&hold_path)?;
    Ok(json!({ "task": task, "released": hold.reason, "decision": decision }))
}

/// Background: fetches the remote default branch and keeps one free slot on it for the next begin.
/// One preparation runs at a time; the OS lock it holds is how `status` reports background work.
pub fn prepare_slot(path: &Path, args: Args) -> Result<Value> {
    args.finish()?;
    let ws = Workspace::open(path)?;
    let lock_path = ws.store.prepare_lock();
    std::fs::create_dir_all(lock_path.parent().unwrap()).map_err(|e| Error::io(&lock_path, e))?;
    let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&lock_path).map_err(|e| Error::io(&lock_path, e))?;
    if lock.try_lock().is_err() {
        return Ok(json!({ "skipped": "another preparation is running" }));
    }
    let fetched = repo::git(&ws.layout.main_worktree, &["fetch", "--quiet", "origin"]).is_ok();
    let ws = Workspace::open(path)?;
    let base = repo::resolve_base(&repo::open_refs_only(&ws.layout.main_worktree)?, None)?;
    for n in ws.free_slots()? {
        let worktree = ws.slot_path(n)?;
        if repo::open(&worktree)?.head_id().ok().map(|id| id.detach()) == Some(base) {
            return Ok(json!({ "slot": n, "advanced": false, "fetched": fetched }));
        }
    }
    if let Some(&n) = ws.free_slots()?.first() {
        let worktree = ws.slot_path(n)?;
        if Store::create_new(&ws.store.claim(n), PREPARING.as_bytes())? {
            let moved = !repo::has_changes(&repo::open(&worktree)?)?
                && repo::git(&worktree, &["checkout", "--quiet", "--detach", &base.to_string()]).is_ok();
            if moved {
                settle_index(&worktree);
            }
            Store::remove(&ws.store.claim(n))?;
            return Ok(json!({ "slot": n, "advanced": moved, "fetched": fetched }));
        }
    }
    let n = ws.claim_new_slot(PREPARING)?;
    let created = ws.ensure_slot_checkout(n, base);
    if let Ok(worktree) = &created {
        settle_index(worktree);
    }
    Store::remove(&ws.store.claim(n))?;
    created?;
    Ok(json!({ "slot": n, "created": true, "fetched": fetched }))
}

struct SpawnTrace;

impl Drop for SpawnTrace {
    fn drop(&mut self) {
        crate::trace::point("background started");
    }
}

/// A checkout writes its files in the same instant as its index, so every entry is "racy" and the next
/// dirty check hashes each file. Rewriting the index a moment later lets that check compare stat data only.
fn settle_index(worktree: &Path) {
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let _ = repo::git(worktree, &["update-index", "-q", "--refresh"]);
}

pub(crate) fn spawn_detached(repo: &Path, args: &[&str]) {
    let _spawned = SpawnTrace;
    let Ok(exe) = std::env::current_exe() else { return };
    let mut command = Command::new(exe);
    command.arg("--repo").arg(repo).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation};
        use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
        // Windows hands every inheritable handle to a child. Without this, the caller reading our stdout
        // would wait until the detached child exits.
        for std_handle in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            // SAFETY: GetStdHandle and SetHandleInformation only read and flag this process's own handles.
            unsafe {
                let handle = GetStdHandle(std_handle);
                if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                    SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
                }
            }
        }
        const DETACHED_PROCESS: u32 = 0x8;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        // Background work must never take the CPU from a foreground command.
        const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x4000;
        const FLAGS: u32 = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | BELOW_NORMAL_PRIORITY_CLASS;
        command.creation_flags(FLAGS | CREATE_BREAKAWAY_FROM_JOB);
        if command.spawn().is_ok() {
            return;
        }
        // A job that forbids breakaway refuses the flag; the child then stays in the caller's job.
        command.creation_flags(FLAGS);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let _ = command.spawn();
}
