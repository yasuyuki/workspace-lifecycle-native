//! `wl finish` accepts a commit and returns; a detached worker integrates it (ADR 0005, 0006, 0011).
use crate::commands::{Workspace, lock_held, retire_task, spawn_detached, task_name};
use crate::config::{self, RequiredCheck, Where};
use crate::error::{Error, Result};
use crate::model::{Acceptance, HoldRecord, Notice, Progress, Stage, TaskName, TaskRecord, TaskStage, VERSION};
use crate::repo;
use crate::store::Store;
use crate::Args;
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CHECK_POLL: Duration = Duration::from_secs(15);
/// GitHub Actions' default job timeout; a check still pending after it will not finish.
const CHECK_DEADLINE: Duration = Duration::from_secs(6 * 60 * 60);
const QUEUE_POLL: Duration = Duration::from_secs(2);

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis())
}

pub fn finish(path: &Path, mut args: Args) -> Result<Value> {
    let task = task_name(&mut args)?;
    let retire = args.flag("retire");
    args.finish()?;
    let ws = Workspace::open(path)?;
    housekeeping(&ws)?;
    crate::trace::point("housekeeping");
    let record = ws.task(&task)?;
    if record.stage != TaskStage::Ready {
        return Err(Error::new(format!("task {task} has not finished beginning; run begin again")));
    }
    if let Some(hold) = Store::read_json::<HoldRecord>(&ws.store.hold(task.as_str()))? {
        return Err(Error::new(format!("task {task} is held: {}", hold.reason)));
    }
    // Only refs are read here: checking the checkout for uncommitted changes costs about 20 ms on Windows,
    // so the worker does it and reports it with the result.
    let slot = repo::open_refs_only(&ws.slot_path(record.slot)?)?;
    let on_branch = slot.head()?.referent_name().is_some_and(|n| n.as_bstr() == format!("refs/heads/{}", record.branch).as_str());
    if !on_branch {
        return Err(Error::new(format!("the checkout of task {task} is not on branch {}", record.branch)));
    }
    let commit = slot.head_id()?.detach().to_string();
    crate::trace::point("slot read");

    let previous = Store::read_json::<Acceptance>(&ws.store.acceptance(task.as_str()))?;
    if previous.is_none() {
        // Progress left without an acceptance belongs to an earlier task of the same name.
        Store::remove(&ws.store.progress(task.as_str()))?;
    }
    let progress = Store::read_json::<Progress>(&ws.store.progress(task.as_str()))?;
    if let Some(previous) = &previous {
        let stage = progress.as_ref().filter(|p| p.attempt == previous.attempt).map(|p| p.stage);
        if !stage.is_some_and(|s| s.is_terminal(previous.retire)) {
            if previous.commit == commit {
                return Ok(json!({ "task": task, "attempt": previous.attempt, "commit": commit, "accepted": true }));
            }
            return Err(Error::new(format!("finish attempt {} of task {task} is still running", previous.attempt)));
        }
        if stage == Some(Stage::Integrated) && previous.commit == commit && !retire {
            return Err(Error::new(format!("{commit} is already integrated; commit more work or pass --retire")));
        }
    }
    let target = match &record.target {
        Some(target) => target.clone(),
        None => default_branch(&slot)?,
    };
    let acceptance = Acceptance {
        version: VERSION,
        task: task.clone(),
        attempt: previous.map_or(1, |p| p.attempt + 1),
        commit: commit.clone(),
        target: target.clone(),
        retire,
        accepted_at_ms: now_ms(),
    };
    Store::write_json(&ws.store.acceptance(task.as_str()), &acceptance)?;
    Store::create_new(&ws.store.queue_dir(&target).join(format!("{:020}-{task}", acceptance.accepted_at_ms)), b"")?;
    crate::trace::point("accepted");
    spawn_detached(&ws.layout.main_worktree, &["__work", task.as_str()]);
    Ok(json!({
        "task": task,
        "attempt": acceptance.attempt,
        "commit": commit,
        "target": target,
        "accepted": true,
    }))
}

/// The branch `origin/HEAD` points at.
fn default_branch(repo: &gix::Repository) -> Result<String> {
    let reference = repo
        .find_reference("refs/remotes/origin/HEAD")
        .map_err(|_| Error::new("origin/HEAD is not set; run `git remote set-head origin --auto` once"))?;
    match reference.target().try_name() {
        Some(name) => Ok(name.as_bstr().to_string().trim_start_matches("refs/remotes/origin/").to_owned()),
        None => Err(Error::new("origin/HEAD is not a symbolic reference")),
    }
}

/// Whether the task has a finish attempt that has not reached its end.
pub(crate) fn running(ws: &Workspace, task: &TaskName) -> Result<bool> {
    let Some(acceptance) = Store::read_json::<Acceptance>(&ws.store.acceptance(task.as_str()))? else { return Ok(false) };
    let progress = Store::read_json::<Progress>(&ws.store.progress(task.as_str()))?;
    Ok(!progress.filter(|p| p.attempt == acceptance.attempt).is_some_and(|p| p.stage.is_terminal(acceptance.retire)))
}

static NOTICES: std::sync::Mutex<Vec<Notice>> = std::sync::Mutex::new(Vec::new());

/// The notices housekeeping handed over; the command prints them with its own result.
pub(crate) fn take_notices() -> Vec<Notice> {
    std::mem::take(&mut NOTICES.lock().unwrap())
}

/// Run by every command: restarts the worker of a finish whose worker is gone, and hands over the notices.
pub(crate) fn housekeeping(ws: &Workspace) -> Result<()> {
    for task in Store::list_dirs(&ws.store.finishes_dir())? {
        let Ok(task) = TaskName::try_from(task) else { continue };
        if running(ws, &task)? && !lock_held(&ws.store.worker_lock(task.as_str())) {
            spawn_detached(&ws.layout.main_worktree, &["__work", task.as_str()]);
        }
    }
    let mut notices = Vec::new();
    for name in Store::list(&ws.store.notices_dir(), ".json")? {
        let path = ws.store.notices_dir().join(format!("{name}.json"));
        if let Some(notice) = Store::read_json::<Notice>(&path)? {
            Store::remove(&path)?;
            notices.push(notice);
        }
    }
    NOTICES.lock().unwrap().extend(notices);
    Ok(())
}

pub(crate) fn finish_report(ws: &Workspace, task: &TaskName) -> Result<Option<Value>> {
    let Some(acceptance) = Store::read_json::<Acceptance>(&ws.store.acceptance(task.as_str()))? else { return Ok(None) };
    let progress = Store::read_json::<Progress>(&ws.store.progress(task.as_str()))?.filter(|p| p.attempt == acceptance.attempt);
    Ok(Some(json!({
        "attempt": acceptance.attempt,
        "commit": acceptance.commit,
        "target": acceptance.target,
        "stage": progress.as_ref().map_or(Stage::Accepted, |p| p.stage),
        "merged": progress.as_ref().and_then(|p| p.merged.clone()),
        "uncommitted": progress.as_ref().and_then(|p| p.uncommitted),
        "detail": progress.and_then(|p| p.detail),
    })))
}

/// Background: drives one finish attempt to its end. One worker per task holds the task's worker lock.
pub fn work(path: &Path, mut args: Args) -> Result<Value> {
    let task = task_name(&mut args)?;
    args.finish()?;
    let ws = Workspace::open(path)?;
    let lock_path = ws.store.worker_lock(task.as_str());
    std::fs::create_dir_all(lock_path.parent().unwrap()).map_err(|e| Error::io(&lock_path, e))?;
    let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&lock_path).map_err(|e| Error::io(&lock_path, e))?;
    if lock.try_lock().is_err() {
        return Ok(json!({ "skipped": "another worker runs this task" }));
    }
    let acceptance: Acceptance = Store::read_json(&ws.store.acceptance(task.as_str()))?.ok_or_else(|| Error::new(format!("task {task} has no accepted finish")))?;
    let progress_path = ws.store.progress(task.as_str());
    let mut progress = Store::read_json::<Progress>(&progress_path)?
        .filter(|p| p.attempt == acceptance.attempt)
        .unwrap_or_else(|| Progress {
            version: VERSION,
            task: task.clone(),
            attempt: acceptance.attempt,
            stage: Stage::Accepted,
            merged: None,
            target_base: None,
            detail: None,
            uncommitted: None,
        });
    let worker = Worker { ws: &ws, acceptance: &acceptance };
    while !progress.stage.is_terminal(acceptance.retire) {
        let before = progress.stage;
        if let Err(error) = worker.step(&mut progress) {
            progress.stage = Stage::NeedsAction;
            progress.detail = Some(error.to_string());
            worker.withdraw();
        }
        // Retiring removed the task's finish records; writing progress back would make a later task of the same name
        // read its first attempt as already retired.
        if progress.stage != Stage::Retired {
            Store::write_durable(&progress_path, &progress)?;
        }
        if progress.stage != before && matches!(progress.stage, Stage::Integrated | Stage::Retired | Stage::NeedsAction) {
            worker.notify(&progress)?;
        }
    }
    Ok(json!({ "task": task, "attempt": acceptance.attempt, "stage": progress.stage }))
}

struct Worker<'a> {
    ws: &'a Workspace,
    acceptance: &'a Acceptance,
}

impl Worker<'_> {
    fn task(&self) -> &TaskName {
        &self.acceptance.task
    }

    fn record(&self) -> Result<TaskRecord> {
        self.ws.task(self.task())
    }

    fn step(&self, progress: &mut Progress) -> Result<()> {
        match progress.stage {
            Stage::Accepted => {
                // Untracked files count too: a new file left out of the commit is the change most easily missed.
                // No optional locks: the person may be working in the checkout meanwhile.
                let worktree = self.ws.slot_path(self.record()?.slot)?;
                progress.uncommitted = Some(!repo::git(&worktree, &["--no-optional-locks", "status", "--porcelain", "--untracked-files=all"])?.is_empty());
                self.push_topic()?;
                progress.stage = Stage::TopicPushed;
            }
            Stage::TopicPushed => {
                self.wait_for_turn()?;
                let (base, merged) = self.prepare()?;
                progress.target_base = Some(base);
                progress.merged = Some(merged);
                progress.stage = Stage::Prepared;
            }
            Stage::Prepared => {
                progress.stage = self.push_target(progress)?;
            }
            Stage::TargetPushed => {
                let merged: gix::ObjectId = progress.merged.as_deref().unwrap_or_default().parse().map_err(|e| Error::new(format!("recorded merge: {e}")))?;
                let main = repo::open_refs_only(&self.ws.layout.main_worktree)?;
                self.wait_for_checks(merged, &config::required_checks(&main, merged)?)?;
                progress.stage = Stage::Integrated;
                progress.detail = None;
            }
            Stage::Integrated => {
                retire_task(self.ws, self.task(), Some(&self.acceptance.commit))?;
                progress.stage = Stage::Retired;
            }
            Stage::Retired | Stage::NeedsAction => {}
        }
        Ok(())
    }

    fn preflight(&self, checkout: &Path) -> Result<(String, String)> {
        let machine = config::machine_config()?;
        if machine.preflight.is_empty() {
            let at = config::machine_config_path().map(|p| p.display().to_string()).unwrap_or_default();
            return Err(Error::new(format!("no push preflight is configured in {at}")));
        }
        let argv: Vec<String> = machine.preflight.iter().map(|a| if a == "{repo}" { checkout.display().to_string() } else { a.clone() }).collect();
        let output = crate::repo::command(&argv[0]).args(&argv[1..]).stdin(Stdio::null()).output().map_err(|e| Error::new(format!("cannot start the push preflight: {e}")))?;
        let decision: Value = serde_json::from_slice(&output.stdout)
            .map_err(|_| Error::new(format!("the push preflight did not print JSON: {}", String::from_utf8_lossy(&output.stderr).trim())))?;
        if decision["decision"] != "push" {
            return Err(Error::new(format!("the push preflight decided {}: {}", decision["decision"], decision["reason"])));
        }
        match (decision["remote"].as_str(), decision["destination"].as_str()) {
            (Some(remote), Some(destination)) => Ok((remote.to_owned(), destination.to_owned())),
            _ => Err(Error::new(format!("the push preflight named no remote or destination: {decision}"))),
        }
    }

    /// Pushes the accepted commit, not the checkout's HEAD, to the destination the preflight approved for the task branch.
    fn push_topic(&self) -> Result<()> {
        let worktree = self.ws.slot_path(self.record()?.slot)?;
        let (remote, destination) = self.preflight(&worktree)?;
        repo::git(&worktree, &["push", "--quiet", &remote, &format!("{}:refs/heads/{destination}", self.acceptance.commit)])?;
        Ok(())
    }

    fn queue(&self) -> PathBuf {
        self.ws.store.queue_dir(&self.acceptance.target)
    }

    /// Waits until this task's entry is the oldest for its target. An entry whose finish has ended is removed.
    /// A worker resumed after a crash puts its entry back at its acceptance time, keeping its place.
    fn wait_for_turn(&self) -> Result<()> {
        let suffix = format!("-{}", self.task());
        let own = format!("{:020}{suffix}", self.acceptance.accepted_at_ms);
        loop {
            let entries = Store::list(&self.queue(), "")?;
            if !entries.iter().any(|e| e.ends_with(&suffix)) {
                Store::create_new(&self.queue().join(&own), b"")?;
                continue;
            }
            let first = &entries[0];
            if first.ends_with(&suffix) {
                return Ok(());
            }
            let other = first.split_once('-').map(|(_, t)| t.to_owned()).unwrap_or_default();
            if let Ok(other) = TaskName::try_from(other) {
                if !running(self.ws, &other)? {
                    Store::remove(&self.queue().join(first))?;
                    continue;
                }
            }
            std::thread::sleep(QUEUE_POLL);
        }
    }

    fn leave_queue(&self) {
        let suffix = format!("-{}", self.task());
        if let Ok(entries) = Store::list(&self.queue(), "") {
            for entry in entries.iter().filter(|e| e.ends_with(&suffix)) {
                let _ = Store::remove(&self.queue().join(entry));
            }
        }
    }

    fn integration_worktree(&self) -> Result<PathBuf> {
        let target = &self.acceptance.target;
        let path = self.ws.slots_root().join(format!("integrate-{}", target.replace('/', "-")));
        let branch = format!("wl-integrate-{}", target.replace('/', "-"));
        let main = &self.ws.layout.main_worktree;
        repo::git(main, &["fetch", "--quiet", "origin", &format!("+refs/heads/{target}:refs/remotes/origin/{target}")])?;
        if !path.join(".git").exists() {
            // The branch keeps this upstream when later checkouts reset it; the push preflight reads the target from it.
            repo::git(main, &["worktree", "add", "--quiet", "--track", "-B", &branch, &path.display().to_string(), &format!("refs/remotes/origin/{target}")])?;
        } else if repo::open_refs_only(&path)?.git_dir().join("MERGE_HEAD").exists() {
            repo::git(&path, &["merge", "--abort"]).ok();
        }
        Ok(path)
    }

    /// Puts the integration worktree's branch on `start` with no other files, ready for validation or a merge.
    fn reset_integration_worktree(&self, path: &Path, start: &str) -> Result<()> {
        let branch = format!("wl-integrate-{}", self.acceptance.target.replace('/', "-"));
        repo::git(path, &["checkout", "--quiet", "--force", "--no-track", "-B", &branch, start])?;
        repo::git(path, &["clean", "--quiet", "-d", "--force"])?;
        Ok(())
    }

    /// Merges the accepted commit onto the target in the integration worktree and runs the validation there.
    /// Reads go through gix; git runs only to fetch, check out, clean and merge.
    fn prepare(&self) -> Result<(String, String)> {
        let path = self.integration_worktree()?;
        let target_ref = format!("refs/remotes/origin/{}", self.acceptance.target);
        let main = repo::open_refs_only(&self.ws.layout.main_worktree)?;
        let base_id = repo::resolve(&main, &target_ref)?.ok_or_else(|| Error::new(format!("{target_ref} does not exist after fetching")))?;
        let base = base_id.to_string();
        let commit = &self.acceptance.commit;
        let commit_id: gix::ObjectId = commit.parse().map_err(|e| Error::new(format!("accepted commit: {e}")))?;
        if repo::is_ancestor(&main, commit_id, base_id)? {
            // Already in the target, which was validated when it got there.
            return Ok((base.clone(), base));
        }
        let merged = if repo::is_ancestor(&main, base_id, commit_id)? {
            self.reset_integration_worktree(&path, commit)?;
            commit_id
        } else {
            self.reset_integration_worktree(&path, &base)?;
            let message = format!("Merge {} into {}", self.record()?.branch, self.acceptance.target);
            if let Err(error) = repo::git(&path, &["merge", "--quiet", "--no-ff", "--no-edit", "-m", &message, commit]) {
                repo::git(&path, &["merge", "--abort"]).ok();
                return Err(Error::new(format!("{commit} does not merge onto {} at {base}: {error}", self.acceptance.target)));
            }
            repo::open_refs_only(&path)?.head_id()?.detach()
        };
        let id = merged;
        let merged = merged.to_string();
        let steps = config::repo_config(&repo::open_refs_only(&self.ws.layout.main_worktree)?, id)?.validation;
        let log_path = self.ws.store.log(self.task().as_str(), self.acceptance.attempt);
        let offload = config::machine_config()?.offload;
        let mut exported: Option<String> = None;
        for (i, step) in steps.iter().enumerate() {
            let ran = match (&offload, step.r#where) {
                (Some(offload), Where::Any) => {
                    let dir = match &exported {
                        Some(dir) => dir.clone(),
                        None => exported.insert(self.export(offload, &merged)?).clone(),
                    };
                    let mut argv = offload.prefix.clone();
                    argv.extend(["sh".into(), "-c".into(), "cd \"$1\" && shift && exec \"$@\"".into(), "sh".into(), dir]);
                    argv.extend(step.argv.iter().cloned());
                    run_logged(&path, &argv, &log_path)
                }
                _ => run_logged(&path, &step.argv, &log_path),
            };
            ran.map_err(|e| Error::new(format!("validation step {} {:?} failed: {e}", i + 1, step.argv)))?;
        }
        Ok((base, merged))
    }

    /// Writes the prepared commit's files into the offload host's directory for this repository and target. The
    /// directory's path stays the same so the host's build caches keep it; `tar -m` gives every file the current
    /// time, because the commit time is older than the last build and cargo would take changed sources as fresh.
    fn export(&self, offload: &config::Offload, commit: &str) -> Result<String> {
        let name = self.ws.layout.main_worktree.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let dir = format!("{}/{}-{}", offload.root.trim_end_matches('/'), name, self.acceptance.target.replace('/', "-"));
        let mut archive = crate::repo::command("git")
            .args(["-c", "gc.auto=0", "-C"])
            .arg(&self.ws.layout.main_worktree)
            .args(["archive", "--format=tar", commit])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| Error::new(format!("cannot start git archive: {e}")))?;
        let mut unpack = crate::repo::command(&offload.prefix[0]);
        unpack
            .args(&offload.prefix[1..])
            .args(["sh", "-c", "rm -rf \"$1\" && mkdir -p \"$1\" && tar -x -m -C \"$1\"", "sh", &dir])
            .stdin(archive.stdout.take().expect("piped stdout"));
        let output = unpack.output().map_err(|e| Error::new(format!("cannot start the offload: {e}")))?;
        let archived = archive.wait().map_err(|e| Error::new(format!("git archive: {e}")))?;
        if !archived.success() {
            return Err(Error::new(format!("git archive {commit} failed")));
        }
        if !output.status.success() {
            return Err(Error::new(format!("cannot write the commit to the offload host: {}", String::from_utf8_lossy(&output.stderr).trim())));
        }
        Ok(dir)
    }

    /// Pushes the prepared commit once the topic checks pass and the target has not moved; else prepares again.
    fn push_target(&self, progress: &Progress) -> Result<Stage> {
        let commit: gix::ObjectId = self.acceptance.commit.parse().map_err(|e| Error::new(format!("{e}")))?;
        let main = repo::open_refs_only(&self.ws.layout.main_worktree)?;
        self.wait_for_checks(commit, &config::required_checks(&main, commit)?)?;
        if let Some(hold) = Store::read_json::<HoldRecord>(&self.ws.store.hold(self.task().as_str()))? {
            return Err(Error::new(format!("held before integration: {}", hold.reason)));
        }
        let path = self.integration_worktree_unchanged()?;
        let target = &self.acceptance.target;
        repo::git(&path, &["fetch", "--quiet", "origin", &format!("+refs/heads/{target}:refs/remotes/origin/{target}")])?;
        let remote_tip = repo::resolve(&repo::open_refs_only(&path)?, &format!("refs/remotes/origin/{target}"))?.map(|id| id.to_string()).unwrap_or_default();
        let merged = progress.merged.clone().unwrap_or_default();
        if remote_tip == merged {
            self.leave_queue();
            return Ok(Stage::TargetPushed);
        }
        if Some(&remote_tip) != progress.target_base.as_ref() {
            return Ok(Stage::TopicPushed);
        }
        let (remote, destination) = self.preflight(&path)?;
        if destination != *target {
            return Err(Error::new(format!("the push preflight chose {destination}, not {target}")));
        }
        if repo::git(&path, &["push", "--quiet", &remote, &format!("{merged}:refs/heads/{target}")]).is_err() {
            return Ok(Stage::TopicPushed);
        }
        self.leave_queue();
        Ok(Stage::TargetPushed)
    }

    fn integration_worktree_unchanged(&self) -> Result<PathBuf> {
        let target = &self.acceptance.target;
        Ok(self.ws.slots_root().join(format!("integrate-{}", target.replace('/', "-"))))
    }

    fn wait_for_checks(&self, commit: gix::ObjectId, checks: &[RequiredCheck]) -> Result<()> {
        if checks.is_empty() {
            return Ok(());
        }
        // The configured URL, before any insteadOf rewriting, names the GitHub repository.
        let main = repo::open(&self.ws.layout.main_worktree)?;
        let url = main.config_snapshot().string("remote.origin.url").map(|u| u.to_string()).ok_or_else(|| Error::new("origin has no URL"))?;
        let slug = github_slug(&url).ok_or_else(|| Error::new(format!("origin {url} is not a GitHub repository")))?;
        let start = Instant::now();
        let mut failures = 0;
        let mut github: Option<GitHub> = None;
        loop {
            let states = match &github {
                Some(github) => Ok(github),
                None => GitHub::connect().map(|g| &*github.insert(g)),
            }
            .and_then(|github| check_states(github, &slug, &commit.to_string(), checks));
            if states.is_err() {
                // A token that expired is read again on the next poll.
                github = None;
            }
            match states {
                Ok(states) => {
                    failures = 0;
                    if let Some((check, state)) = states.iter().find(|(_, s)| matches!(s, CheckState::Failed(_))) {
                        return Err(Error::new(format!("required check {} on {commit}: {state:?}", check.name)));
                    }
                    if states.iter().all(|(_, s)| *s == CheckState::Succeeded) {
                        return Ok(());
                    }
                }
                Err(error) => {
                    failures += 1;
                    if failures >= 20 {
                        return Err(Error::new(format!("cannot read the checks of {commit}: {error}")));
                    }
                }
            }
            if start.elapsed() > CHECK_DEADLINE {
                return Err(Error::new(format!("required checks on {commit} did not finish within {} hours", CHECK_DEADLINE.as_secs() / 3600)));
            }
            std::thread::sleep(CHECK_POLL);
        }
    }

    fn withdraw(&self) {
        if let Ok(path) = self.integration_worktree_unchanged() {
            if path.join(".git").exists() {
                repo::git(&path, &["merge", "--abort"]).ok();
            }
        }
        self.leave_queue();
    }

    fn notify(&self, progress: &Progress) -> Result<()> {
        let mut message = match progress.stage {
            Stage::Integrated => format!("{} is integrated into {}", self.acceptance.commit, self.acceptance.target),
            Stage::Retired => format!("{} is integrated into {} and the task is retired", self.acceptance.commit, self.acceptance.target),
            _ => format!("needs action: {}", progress.detail.clone().unwrap_or_default()),
        };
        if progress.uncommitted == Some(true) && progress.stage == Stage::Integrated {
            message.push_str("; the checkout had uncommitted changes or untracked files when the finish started, and they are not in it");
        }
        let notice = Notice { task: self.task().clone(), attempt: progress.attempt, stage: progress.stage, message: message.clone() };
        let name = format!("{}-{}-{:?}.json", self.task(), progress.attempt, progress.stage);
        Store::create_new_json(&self.ws.store.notices_dir().join(name), &notice)?;
        if progress.stage == Stage::NeedsAction {
            toast(&format!("wl: {}", self.task()), &message);
        }
        Ok(())
    }
}

/// Runs `argv` in `dir`, appending its output to `log`; the error carries the exit status and the output's last lines.
fn run_logged(dir: &Path, argv: &[String], log: &Path) -> Result<()> {
    std::fs::create_dir_all(log.parent().unwrap()).map_err(|e| Error::io(log, e))?;
    let mut command = crate::repo::command(argv.first().ok_or_else(|| Error::new("empty validation argv"))?);
    command.args(&argv[1..]).current_dir(dir).stdin(Stdio::null());
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().to_ascii_uppercase().starts_with("WL_") {
            command.env_remove(key);
        }
    }
    let output = command.output().map_err(|e| Error::new(format!("cannot start: {e}")))?;
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(log).map_err(|e| Error::io(log, e))?;
    let _ = writeln!(file, "$ {argv:?}");
    let _ = file.write_all(&output.stdout);
    let _ = file.write_all(&output.stderr);
    if output.status.success() {
        return Ok(());
    }
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let tail: Vec<&str> = text.lines().rev().take(15).collect();
    Err(Error::new(format!("{}; log {}; last lines:\n{}", output.status, log.display(), tail.into_iter().rev().collect::<Vec<_>>().join("\n"))))
}

#[derive(Debug, PartialEq, Eq)]
enum CheckState {
    Pending,
    Succeeded,
    Failed(String),
}

fn github_slug(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://github.com/").or_else(|| url.strip_prefix("git@github.com:")).or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    let slug = rest.trim_end_matches('/').trim_end_matches(".git");
    (slug.split('/').count() == 2).then(|| slug.to_owned())
}

/// The GitHub API, read in this process: a poll that started `gh` every few seconds would start a process each time,
/// and on Windows every process start slows the foreground commands that start meanwhile.
struct GitHub {
    agent: ureq::Agent,
    api: String,
    token: String,
}

impl GitHub {
    /// Takes the token from `gh`, once per wait. `WL_GITHUB_API` replaces the API's address, for tests.
    fn connect() -> Result<GitHub> {
        let output = crate::repo::command("gh")
            .args(["auth", "token"])
            .stdin(Stdio::null())
            .output()
            .map_err(|e| Error::new(format!("cannot start gh: {e}")))?;
        if !output.status.success() {
            return Err(Error::new(format!("gh auth token: {}", String::from_utf8_lossy(&output.stderr).trim())));
        }
        let agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(60))).build().into();
        Ok(GitHub {
            agent,
            api: std::env::var("WL_GITHUB_API").unwrap_or_else(|_| "https://api.github.com".to_owned()),
            token: String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        })
    }

    fn get(&self, path: &str) -> Result<Value> {
        let mut response = self
            .agent
            .get(format!("{}/{path}", self.api.trim_end_matches('/')))
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "wl")
            .call()
            .map_err(|e| Error::new(format!("GitHub API {path}: {e}")))?;
        let body = response.body_mut().read_to_string().map_err(|e| Error::new(format!("GitHub API {path}: {e}")))?;
        Ok(serde_json::from_str(&body)?)
    }
}

/// The state of each required check on `sha`: the latest run with the same app and name, as the legacy lifecycle reads it.
fn check_states<'a>(github: &GitHub, slug: &str, sha: &str, checks: &'a [RequiredCheck]) -> Result<Vec<(&'a RequiredCheck, CheckState)>> {
    let body = github.get(&format!("repos/{slug}/commits/{sha}/check-runs?per_page=100"))?;
    let runs = body["check_runs"].as_array().cloned().unwrap_or_default();
    Ok(checks
        .iter()
        .map(|check| {
            let latest = runs
                .iter()
                .filter(|r| r["name"] == check.name.as_str() && r["app"]["slug"] == check.app.as_str())
                .max_by_key(|r| r["id"].as_u64().unwrap_or(0));
            let state = match latest {
                None => CheckState::Pending,
                Some(run) if run["status"] != "completed" => CheckState::Pending,
                Some(run) if run["conclusion"] == "success" => CheckState::Succeeded,
                Some(run) => CheckState::Failed(format!("{} {}", run["conclusion"], run["html_url"])),
            };
            (check, state)
        })
        .collect())
}

/// Best effort: a person away from the terminal learns that a finish needs them.
fn toast(title: &str, message: &str) {
    #[cfg(windows)]
    {
        let escape = |s: &str| s.replace('\'', "''").replace('<', "&lt;").replace('>', "&gt;").replace('&', "&amp;");
        let script = format!(
            "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null; \
             $x = [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime]::new(); \
             $x.LoadXml('<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>'); \
             [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('{{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}}\\WindowsPowerShell\\v1.0\\powershell.exe').Show([Windows.UI.Notifications.ToastNotification]::new($x))",
            escape(title),
            escape(&message.chars().take(200).collect::<String>())
        );
        let _ = crate::repo::command("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    #[cfg(not(windows))]
    {
        let _ = (title, message);
    }
}
