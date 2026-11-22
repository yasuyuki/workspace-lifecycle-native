use crate::error::{Error, Result};
use crate::model::{FinishId, HoldRecord, PREPARING, SlotRecord, TaskName, TaskRecord, TaskStage, VERSION};
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

    pub(crate) fn lock_task(&self, task: &TaskName) -> Result<std::fs::File> {
        Store::lock_exclusive(&self.store.task_lock(task.as_str()))
    }

    pub(crate) fn release_claims_for(&self, owner: &str) -> Result<()> {
        for n in self.slot_numbers(".claim")? {
            if Store::read_text(&self.store.claim(n))?.as_deref() == Some(owner) {
                Store::remove(&self.store.claim(n))?;
            }
        }
        Ok(())
    }

    pub(crate) fn reclaim_stale_prepare_claims_locked(&self) -> Result<()> {
        self.release_claims_for(PREPARING)
    }

    pub(crate) fn reclaim_orphan_task_claims(&self) -> Result<()> {
        for n in self.slot_numbers(".claim")? {
            let claim_path = self.store.claim(n);
            let Some(owner) = Store::read_text(&claim_path)? else { continue };
            if owner == PREPARING {
                continue;
            }
            let Ok(task) = TaskName::try_from(owner.clone()) else { continue };
            let task_path = self.store.task(task.as_str());
            if !task_path.exists() {
                if let Some(_lock) = Store::try_lock_exclusive(&self.store.task_lock(task.as_str()))? {
                    if !task_path.exists() && Store::read_text(&claim_path)?.as_deref() == Some(owner.as_str()) {
                        Store::remove(&claim_path)?;
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn reclaim_orphan_claims(&self) -> Result<()> {
        self.reclaim_orphan_task_claims()?;
        let has_prepare = self
            .slot_numbers(".claim")?
            .into_iter()
            .any(|n| matches!(Store::read_text(&self.store.claim(n)), Ok(Some(ref c)) if c == PREPARING));
        if has_prepare {
            if let Some(_lock) = Store::try_lock_exclusive(&self.store.prepare_lock())? {
                self.reclaim_stale_prepare_claims_locked()?;
            }
        }
        Ok(())
    }

    /// Claims a slot for `owner`: one it already claimed, else a free one, else a new number.
    pub(crate) fn claim_slot(&self, owner: &str) -> Result<u32> {
        let _lock = Store::lock_exclusive(&self.store.retirement_lock())?;
        let claims = self.slot_numbers(".claim")?;
        for &n in &claims {
            if Store::read_text(&self.store.claim(n))?.as_deref() == Some(owner) {
                return Ok(n);
            }
        }
        let mut free = Vec::new();
        for n in self.slot_numbers(".json")? {
            if !claims.contains(&n) {
                let order = Store::read_json::<SlotRecord>(&self.store.slot(n))?
                    .and_then(|slot| slot.retirement_order);
                free.push((order.is_none(), order, n));
            }
        }
        free.sort_unstable();
        for (_, _, n) in free {
            if Store::create_new(&self.store.claim(n), owner.as_bytes())? {
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
        let record = SlotRecord { version: VERSION, slot: n, path: path.clone(), retirement_order: None };
        Store::write_json(&self.store.slot(n), &record)?;
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
    let _task_lock = ws.lock_task(&task)?;
    crate::finish::finalize_retired_finish_locked(&ws, &task)?;
    let task_path = ws.store.task(task.as_str());
    let mut record = match Store::read_json::<TaskRecord>(&task_path)? {
        Some(existing) => existing,
        None => {
            let base = repo::resolve_base(&repo::open_refs_only(&ws.layout.main_worktree)?, base_spec.as_deref())?;
            // A begin of the same task finds the claim it or an earlier attempt made, so both use one slot.
            let slot = ws.claim_slot(task.as_str())?;
            crate::trace::point("slot claimed");
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
            match Store::create_new_json(&task_path, &fresh) {
                Ok(true) => fresh,
                Ok(false) => Store::read_json(&task_path)?
                    .ok_or_else(|| Error::new(format!("task {task} is being created by another begin")))?,
                Err(error) => {
                    ws.release_claims_for(task.as_str()).ok();
                    return Err(error);
                }
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
    let slot_repo = repo::open(&worktree)?;
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

/// Whether `origin/<branch>`, read from the remote now, contains `tip`.
fn on_remote_topic(worktree: &Path, branch: &str, tip: gix::ObjectId) -> Result<bool> {
    let tracking = format!("refs/remotes/origin/{branch}");
    if repo::git(worktree, &["fetch", "--quiet", "origin", &format!("+refs/heads/{branch}:{tracking}")]).is_err() {
        return Ok(false);
    }
    let slot = repo::open_refs_only(worktree)?;
    match repo::resolve(&slot, &tracking)? {
        Some(remote) => repo::is_ancestor(&slot, tip, remote),
        None => Ok(false),
    }
}

/// Returns the task's slot to the free pool. P1 refuses anything that retiring would lose: changes in the
/// checkout, untracked files, and commits that are neither the base nor in the remote default branch.
pub fn retire(path: &Path, mut args: Args) -> Result<Value> {
    let task = task_name(&mut args)?;
    let abandon = args.flag("abandon");
    let decision = args.option("decision")?;
    args.finish()?;
    if abandon != decision.is_some() {
        return Err(Error::new("--abandon and --decision REF go together: the decision not to integrate the task"));
    }
    let ws = Workspace::open(path)?;
    crate::finish::housekeeping(&ws)?;
    retire_task(&ws, &task, None, abandon)?;
    match decision {
        Some(decision) => Ok(json!({ "task": task, "retired": true, "abandoned": true, "decision": decision })),
        None => Ok(json!({ "task": task, "retired": true })),
    }
}

/// Returns the task's slot to the free pool. Refuses anything that retiring would lose: changes in the checkout,
/// untracked files, commits that are neither the base nor in the remote default branch, a finish still running,
/// and with `finish_retire` a checkout that moved past the commit a finish integrated. With `abandon`, commits that
/// are not in the target are kept when the remote topic `origin/<branch>` holds them, since nothing is then lost.
pub(crate) fn retire_task(
    ws: &Workspace,
    task: &TaskName,
    finish_retire: Option<crate::finish::FinishRetire<'_>>,
    abandon: bool,
) -> Result<()> {
    let _task_lock = ws.lock_task(task)?;
    let task_path = ws.store.task(task.as_str());
    let Some(record) = Store::read_json::<TaskRecord>(&task_path)? else {
        if let Some(fr) = finish_retire {
            ws.release_claims_for(task.as_str())?;
            crate::finish::write_retired_run(ws, fr.acceptance, Some(fr.progress))?;
            crate::finish::clear_finish_files(ws, task)?;
        } else {
            crate::finish::finalize_retired_finish_locked(ws, task)?;
            ws.release_claims_for(task.as_str())?;
        }
        return Ok(());
    };
    if Store::read_json::<HoldRecord>(&ws.store.hold(task.as_str()))?.is_some() {
        return Err(Error::new(format!("task {task} is held; release the hold first")));
    }
    if finish_retire.is_none() && crate::finish::running(ws, task)? {
        return Err(Error::new(format!("task {task} has a finish in progress")));
    }
    let expected_head = finish_retire.as_ref().map(|fr| fr.acceptance.commit.as_str());
    let n = record.slot;
    let worktree = ws.slot_path(n)?;
    if worktree.join(".git").exists() {
        let untracked_or_changed = repo::git(&worktree, &["status", "--porcelain", "--untracked-files=all"])?;
        if !untracked_or_changed.is_empty() {
            return Err(Error::new(format!("task {task} has uncommitted or untracked files in {}", worktree.display())));
        }
        let branch_ref = format!("refs/heads/{}", record.branch);
        let slot = repo::open_refs_only(&worktree)?;
        if let Some(tip_id) = repo::resolve(&slot, &branch_ref)? {
            let tip = tip_id.to_string();
            if let Some(expected) = expected_head {
                if tip != expected {
                    return Err(Error::new(format!("task {task} has commits after the integrated {expected}; retire it after the next finish")));
                }
            }
            let integrated_into = record.target.as_ref().map_or("refs/remotes/origin/HEAD".to_owned(), |t| format!("refs/remotes/origin/{t}"));
            let published = tip == record.base
                || match repo::resolve(&slot, &integrated_into)? {
                    Some(into) => repo::is_ancestor(&slot, tip_id, into)?,
                    None => false,
                };
            if !published && !abandon {
                return Err(Error::new(format!("task {task} has commits that are not in {integrated_into}")));
            }
            if !published && !on_remote_topic(&worktree, &record.branch, tip_id)? {
                return Err(Error::new(format!(
                    "task {task} has commits that are neither in {integrated_into} nor on origin/{}; push the topic before abandoning it",
                    record.branch
                )));
            }
            repo::git(&worktree, &["switch", "--quiet", "--detach", &tip])?;
            slot.find_reference(branch_ref.as_str())?.delete()?;
        }
        clear_ignored(&worktree)?;
    }
    Store::remove(&ws.store.hold(task.as_str()))?;
    if finish_retire.is_none() {
        crate::finish::clear_finish_files(ws, task)?;
    }
    {
        // Keep the claim until retirement is recorded. Once the task file is removed,
        // orphan recovery can finish releasing the claim with the same order.
        let _lock = Store::lock_exclusive(&ws.store.retirement_lock())?;
        let slot_path = ws.store.slot(n);
        if let Some(mut slot) = Store::read_json::<SlotRecord>(&slot_path)? {
            let mut last = 0;
            for number in ws.slot_numbers(".json")? {
                if let Some(order) = Store::read_json::<SlotRecord>(&ws.store.slot(number))?
                    .and_then(|record| record.retirement_order)
                {
                    last = last.max(order);
                }
            }
            slot.retirement_order = Some(last + 1);
            Store::write_json(&slot_path, &slot)?;
        }
        Store::remove(&task_path)?;
        if Store::read_text(&ws.store.claim(n))?.as_deref() == Some(task.as_str()) {
            Store::remove(&ws.store.claim(n))?;
        }
    }
    if let Some(fr) = finish_retire {
        crate::finish::write_retired_run(ws, fr.acceptance, Some(fr.progress))?;
        crate::finish::clear_finish_files(ws, task)?;
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
    let finish_id = args
        .option("finish")?
        .map(FinishId::try_from)
        .transpose()
        .map_err(Error::new)?;
    let only = args.optional_positional().map(TaskName::try_from).transpose().map_err(Error::new)?;
    let only_requested = only.is_some();
    args.finish()?;
    let ws = Workspace::open(path)?;
    crate::finish::housekeeping(&ws)?;
    let total = ws.slot_numbers(".json")?.len();
    let workers = ws.store.prepare_lock().with_file_name("");
    let background = lock_held(&ws.store.prepare_lock())
        || Store::list(&workers, ".lock")?.iter().any(|name| lock_held(&workers.join(format!("{name}.lock"))));
    let slots = json!({ "total": total, "free": ws.free_slots()?.len() });
    if let Some(id) = finish_id {
        let finish = crate::finish::finish_status(&ws, &id, only.as_ref())?;
        return Ok(json!({ "finish": finish, "slots": slots, "background": background }));
    }
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
    Ok(json!({ "tasks": tasks, "slots": slots, "background": background }))
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
    let Some(_lock) = Store::try_lock_exclusive(&ws.store.prepare_lock())? else {
        return Ok(json!({ "skipped": "another preparation is running" }));
    };
    ws.reclaim_stale_prepare_claims_locked()?;
    ws.reclaim_orphan_task_claims()?;
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
            let outcome: Result<bool> = (|| {
                let moved = !repo::has_changes(&repo::open(&worktree)?)?
                    && repo::git(&worktree, &["checkout", "--quiet", "--detach", &base.to_string()]).is_ok();
                if moved {
                    settle_index(&worktree);
                }
                Ok(moved)
            })();
            Store::remove(&ws.store.claim(n))?;
            let moved = outcome?;
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
