use crate::error::{Error, Result};
use crate::model::{HoldRecord, PREPARING, SlotRecord, TaskName, TaskRecord, TaskStage, VERSION};
use crate::repo::{self, Layout};
use crate::store::Store;
use crate::Args;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

struct Workspace {
    repo: gix::Repository,
    layout: Layout,
    store: Store,
}

impl Workspace {
    fn open(path: &Path) -> Result<Self> {
        let repo = repo::open(path)?;
        let layout = repo::layout(&repo)?;
        let store = Store::new(&layout.common_dir);
        Ok(Self { repo, layout, store })
    }

    fn slots_root(&self) -> PathBuf {
        let name = self.layout.main_worktree.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.layout.main_worktree.with_file_name(format!("{name}.slots"))
    }

    fn slot_numbers(&self, suffix: &str) -> Result<Vec<u32>> {
        let mut numbers: Vec<u32> = Store::list(&self.store.slots_dir(), suffix)?.iter().filter_map(|n| n.parse().ok()).collect();
        numbers.sort_unstable();
        Ok(numbers)
    }

    fn free_slots(&self) -> Result<Vec<u32>> {
        let claimed = self.slot_numbers(".claim")?;
        Ok(self.slot_numbers(".json")?.into_iter().filter(|n| !claimed.contains(n)).collect())
    }

    fn slot_path(&self, slot: u32) -> Result<PathBuf> {
        Ok(Store::read_json::<SlotRecord>(&self.store.slot(slot))?
            .map(|record| record.path)
            .unwrap_or_else(|| self.slots_root().join(slot.to_string())))
    }

    fn task(&self, task: &TaskName) -> Result<TaskRecord> {
        Store::read_json(&self.store.task(task.as_str()))?.ok_or_else(|| Error::new(format!("no task {task}")))
    }

    /// Claims a slot for `owner`: one it already claimed, else a free one, else a new number.
    fn claim_slot(&self, owner: &str) -> Result<u32> {
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
        let mut n = slots.iter().chain(&claims).max().map_or(1, |m| m + 1);
        while !Store::create_new(&self.store.claim(n), owner.as_bytes())? {
            n += 1;
        }
        Ok(n)
    }

    /// Makes sure slot `n` has a checkout, creating one detached at `base` when it has none.
    fn ensure_slot_checkout(&self, n: u32, base: gix::ObjectId) -> Result<PathBuf> {
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

    fn report(&self, record: &TaskRecord) -> Result<Value> {
        let worktree = match record.slot {
            Some(n) => Some(self.slot_path(n)?),
            None => None,
        };
        let hold = Store::read_json::<HoldRecord>(&self.store.hold(record.task.as_str()))?.map(|h| h.reason);
        Ok(json!({
            "task": record.task,
            "request": record.request,
            "branch": record.branch,
            "base": record.base,
            "stage": record.stage,
            "worktree": worktree,
            "hold": hold,
        }))
    }
}

fn task_name(args: &mut Args) -> Result<TaskName> {
    TaskName::try_from(args.positional("TASK")?).map_err(Error::new)
}

pub fn begin(path: &Path, mut args: Args) -> Result<Value> {
    let task = task_name(&mut args)?;
    let request = args.required("request")?;
    let branch = args.option("branch")?.unwrap_or_else(|| task.to_string());
    let base_spec = args.option("base")?;
    args.finish()?;
    gix::refs::FullName::try_from(format!("refs/heads/{branch}"))?;

    let ws = Workspace::open(path)?;
    let base = repo::resolve_base(&ws.repo, base_spec.as_deref())?;
    let task_path = ws.store.task(task.as_str());
    let fresh = TaskRecord {
        version: VERSION,
        task: task.clone(),
        request,
        branch,
        base: base.to_string(),
        slot: None,
        stage: TaskStage::Claimed,
    };
    let mut record = if Store::create_new_json(&task_path, &fresh)? {
        fresh
    } else {
        let existing = ws.task(&task)?;
        if existing.request != fresh.request || existing.branch != fresh.branch {
            return Err(Error::new(format!("task {task} already exists with another request or branch")));
        }
        existing
    };
    if record.stage == TaskStage::Ready {
        return ws.report(&record);
    }
    let base: gix::ObjectId = record.base.parse().map_err(|e| Error::new(format!("recorded base: {e}")))?;

    let slot = match record.slot {
        Some(n) => n,
        None => {
            let n = ws.claim_slot(task.as_str())?;
            record.slot = Some(n);
            Store::write_json(&task_path, &record)?;
            n
        }
    };
    crate::trace::point("slot claimed");
    let worktree = ws.ensure_slot_checkout(slot, base)?;
    switch_slot(&worktree, &record.branch, base)?;
    record.stage = TaskStage::Ready;
    Store::write_json(&task_path, &record)?;

    crate::trace::point("task ready");
    if ws.free_slots()?.is_empty() {
        spawn_detached(&ws.layout.main_worktree, &["__prepare-slot"]);
    }
    ws.report(&record)
}

/// Puts the slot checkout on a new branch at `base`. A clean slot already on `base` only needs its refs moved.
fn switch_slot(worktree: &Path, branch: &str, base: gix::ObjectId) -> Result<()> {
    let mut slot = repo::open(worktree)?;
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
    let task_path = ws.store.task(task.as_str());
    let Some(record) = Store::read_json::<TaskRecord>(&task_path)? else {
        return Ok(json!({ "task": task, "retired": true }));
    };
    if Store::read_json::<HoldRecord>(&ws.store.hold(task.as_str()))?.is_some() {
        return Err(Error::new(format!("task {task} is held; release the hold first")));
    }
    if let Some(n) = record.slot {
        let worktree = ws.slot_path(n)?;
        if worktree.join(".git").exists() {
            let untracked_or_changed = repo::git(&worktree, &["status", "--porcelain", "--untracked-files=all"])?;
            if !untracked_or_changed.is_empty() {
                return Err(Error::new(format!("task {task} has uncommitted or untracked files in {}", worktree.display())));
            }
            let branch_ref = format!("refs/heads/{}", record.branch);
            if let Ok(tip) = repo::git(&worktree, &["rev-parse", "--verify", "--quiet", &branch_ref]) {
                let published = tip == record.base
                    || repo::git(&worktree, &["merge-base", "--is-ancestor", &tip, "refs/remotes/origin/HEAD"]).is_ok();
                if !published {
                    return Err(Error::new(format!("task {task} has commits that are not in the remote default branch")));
                }
                repo::git(&worktree, &["switch", "--quiet", "--detach", &tip])?;
                repo::git(&worktree, &["branch", "--quiet", "--delete", "--force", &record.branch])?;
            }
        }
        Store::remove(&ws.store.hold(task.as_str()))?;
        Store::remove(&task_path)?;
        if Store::read_text(&ws.store.claim(n))?.as_deref() == Some(task.as_str()) {
            Store::remove(&ws.store.claim(n))?;
        }
    } else {
        Store::remove(&task_path)?;
    }
    spawn_detached(&ws.layout.main_worktree, &["__prepare-slot"]);
    Ok(json!({ "task": task, "retired": true }))
}

pub fn status(path: &Path, mut args: Args) -> Result<Value> {
    let only = args.optional_positional().map(TaskName::try_from).transpose().map_err(Error::new)?;
    args.finish()?;
    let ws = Workspace::open(path)?;
    let names = match only {
        Some(task) => vec![task],
        None => Store::list(&ws.store.tasks_dir(), ".json")?
            .into_iter()
            .filter(|n| !n.ends_with(".hold"))
            .filter_map(|n| TaskName::try_from(n).ok())
            .collect(),
    };
    let tasks = names.iter().map(|t| ws.task(t).and_then(|r| ws.report(&r))).collect::<Result<Vec<_>>>()?;
    let total = ws.slot_numbers(".json")?.len();
    Ok(json!({ "tasks": tasks, "slots": { "total": total, "free": ws.free_slots()?.len() } }))
}

pub fn hold(path: &Path, mut args: Args) -> Result<Value> {
    let task = task_name(&mut args)?;
    let reason = args.required("reason")?;
    args.finish()?;
    let ws = Workspace::open(path)?;
    ws.task(&task)?;
    Store::write_json(&ws.store.hold(task.as_str()), &HoldRecord { version: VERSION, reason: reason.clone() })?;
    Ok(json!({ "task": task, "hold": reason }))
}

pub fn release_hold(path: &Path, mut args: Args) -> Result<Value> {
    let task = task_name(&mut args)?;
    let decision = args.required("decision")?;
    args.finish()?;
    let ws = Workspace::open(path)?;
    let hold_path = ws.store.hold(task.as_str());
    let hold = Store::read_json::<HoldRecord>(&hold_path)?.ok_or_else(|| Error::new(format!("task {task} has no hold")))?;
    Store::remove(&hold_path)?;
    Ok(json!({ "task": task, "released": hold.reason, "decision": decision }))
}

/// Background: fetches the remote default branch and keeps one free slot on it for the next begin.
pub fn prepare_slot(path: &Path, args: Args) -> Result<Value> {
    args.finish()?;
    let ws = Workspace::open(path)?;
    let fetched = repo::git(&ws.layout.main_worktree, &["fetch", "--quiet", "origin"]).is_ok();
    let ws = Workspace::open(path)?;
    let base = repo::resolve_base(&ws.repo, None)?;
    if let Some(&n) = ws.free_slots()?.first() {
        let worktree = ws.slot_path(n)?;
        if Store::create_new(&ws.store.claim(n), PREPARING.as_bytes())? {
            let slot = repo::open(&worktree)?;
            let moved = if slot.head_id().ok().map(|id| id.detach()) != Some(base) && !repo::has_changes(&slot)? {
                repo::git(&worktree, &["checkout", "--quiet", "--detach", &base.to_string()]).is_ok()
            } else {
                false
            };
            if moved {
                settle_index(&worktree);
            }
            Store::remove(&ws.store.claim(n))?;
            return Ok(json!({ "slot": n, "advanced": moved, "fetched": fetched }));
        }
    }
    let n = ws.claim_slot(PREPARING)?;
    let created = ws.ensure_slot_checkout(n, base);
    if let Ok(worktree) = &created {
        settle_index(worktree);
    }
    Store::remove(&ws.store.claim(n))?;
    created?;
    Ok(json!({ "slot": n, "created": true, "fetched": fetched }))
}

/// A checkout writes its files in the same instant as its index, so every entry is "racy" and the next
/// dirty check hashes each file. Rewriting the index a moment later lets that check compare stat data only.
fn settle_index(worktree: &Path) {
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let _ = repo::git(worktree, &["update-index", "-q", "--refresh"]);
}

fn spawn_detached(repo: &Path, args: &[&str]) {
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
