use crate::error::{Error, Result};
use serde::{Serialize, de::DeserializeOwned};
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// The `wl/` directory in a repository's Git common dir. Every file in it has exactly one writer.
pub struct Store {
    root: PathBuf,
}

static TEMP_SEQ: AtomicU32 = AtomicU32::new(0);

impl Store {
    pub fn new(common_dir: &Path) -> Self { Self { root: common_dir.join("wl") } }

    pub fn tasks_dir(&self) -> PathBuf { self.root.join("tasks") }
    pub fn slots_dir(&self) -> PathBuf { self.root.join("slots") }
    pub fn task(&self, task: &str) -> PathBuf { self.tasks_dir().join(format!("{task}.json")) }
    pub fn hold(&self, task: &str) -> PathBuf { self.root.join("holds").join(format!("{task}.json")) }
    pub fn slot(&self, slot: u32) -> PathBuf { self.slots_dir().join(format!("{slot}.json")) }
    pub fn retirement_lock(&self) -> PathBuf { self.slots_dir().join("retirement.lock") }
    pub fn finishes_dir(&self) -> PathBuf { self.root.join("finishes") }
    pub fn acceptance(&self, task: &str) -> PathBuf { self.finishes_dir().join(task).join("accept.json") }
    pub fn progress(&self, task: &str) -> PathBuf { self.finishes_dir().join(task).join("progress.json") }
    pub fn worker_lock(&self, task: &str) -> PathBuf { self.root.join("workers").join(format!("{task}.lock")) }
    pub fn queue_dir(&self, target: &str) -> PathBuf { self.root.join("queue").join(target.replace('/', "%2F")) }
    pub fn task_lock(&self, task: &str) -> PathBuf { self.root.join("locks").join(format!("{task}.lock")) }
    pub fn integration_lock(&self, target: &str) -> PathBuf { self.root.join("queue").join(format!("{}.lock", target.replace('/', "%2F"))) }
    pub fn notices_dir(&self) -> PathBuf { self.root.join("notices") }
    pub fn log(&self, task: &str, attempt: u32) -> PathBuf { self.root.join("logs").join(format!("{task}-{attempt}.log")) }
    pub fn prepare_lock(&self) -> PathBuf { self.root.join("workers").join("prepare.lock") }
    pub fn claim(&self, slot: u32) -> PathBuf { self.slots_dir().join(format!("{slot}.claim")) }

    pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
        match fs::read(path) {
            // A file created in one step can be read before its content is written: it is still being created.
            Ok(bytes) if bytes.is_empty() => Ok(None),
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| Error::new(format!("unreadable state {}: {e}", path.display()))),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Error::io(path, e)),
        }
    }

    pub fn read_text(path: &Path) -> Result<Option<String>> {
        match fs::read_to_string(path) {
            Ok(text) => Ok(Some(text)),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Error::io(path, e)),
        }
    }

    fn temp_beside(path: &Path, bytes: &[u8]) -> Result<PathBuf> {
        Self::temp_beside_flushed(path, bytes, false)
    }

    fn temp_beside_flushed(path: &Path, bytes: &[u8], flush: bool) -> Result<PathBuf> {
        let dir = path.parent().expect("state paths have a parent");
        fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        let temp = dir.join(format!(
            ".{}.{}.{}.tmp",
            path.file_name().unwrap().to_string_lossy(),
            std::process::id(),
            TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = fs::File::create(&temp).map_err(|e| Error::io(&temp, e))?;
        // A foreground write is not flushed: one lost to a crash is rebuilt by running the command again.
        file.write_all(bytes).map_err(|e| Error::io(&temp, e))?;
        if flush {
            file.sync_all().map_err(|e| Error::io(&temp, e))?;
        }
        Ok(temp)
    }

    /// Replaces `path` with `value` in one step; a reader sees the old or the new content.
    pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
        Self::replace(path, Self::temp_beside(path, &to_json(value))?)
    }

    /// As `write_json`, flushed to disk first: background progress has no command to run again.
    pub fn write_durable<T: Serialize>(path: &Path, value: &T) -> Result<()> {
        Self::replace(path, Self::temp_beside_flushed(path, &to_json(value), true)?)
    }

    fn replace(path: &Path, temp: PathBuf) -> Result<()> {
        let mut attempt = 0;
        loop {
            match fs::rename(&temp, path) {
                Ok(()) => return Ok(()),
                // Windows refuses to replace a file another process has open for reading.
                Err(e) if e.kind() == ErrorKind::PermissionDenied && attempt < 50 => {
                    attempt += 1;
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                Err(e) => {
                    let _ = fs::remove_file(&temp);
                    return Err(Error::io(path, e));
                }
            }
        }
    }

    /// Creates `path` with `bytes` only if it does not exist yet. Returns false when it already exists.
    /// A reader may see the file empty until its content is written; `read_json` treats that as absent.
    pub fn create_new(path: &Path, bytes: &[u8]) -> Result<bool> {
        let open = || fs::OpenOptions::new().write(true).create_new(true).open(path);
        let mut file = match open() {
            Err(e) if e.kind() == ErrorKind::NotFound => {
                let dir = path.parent().expect("state paths have a parent");
                fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
                open()
            }
            other => other,
        };
        match &mut file {
            Ok(file) => file.write_all(bytes).map(|_| true).map_err(|e| Error::io(path, e)),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(Error::new(format!("{}: {e}", path.display()))),
        }
    }

    pub fn create_new_json<T: Serialize>(path: &Path, value: &T) -> Result<bool> {
        Self::create_new(path, &to_json(value))
    }

    pub fn remove(path: &Path) -> Result<()> {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::io(path, e)),
        }
    }

    pub fn lock_exclusive(path: &Path) -> Result<fs::File> {
        let dir = path.parent().expect("lock paths have a parent");
        fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        let file = fs::OpenOptions::new().create(true).truncate(false).write(true).open(path).map_err(|e| Error::io(path, e))?;
        file.lock().map_err(|e| Error::io(path, e))?;
        Ok(file)
    }

    pub fn try_lock_exclusive(path: &Path) -> Result<Option<fs::File>> {
        let dir = path.parent().expect("lock paths have a parent");
        fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        let file = fs::OpenOptions::new().create(true).truncate(false).write(true).open(path).map_err(|e| Error::io(path, e))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(file)),
            Err(fs::TryLockError::WouldBlock) => Ok(None),
            Err(fs::TryLockError::Error(e)) => Err(Error::io(path, e)),
        }
    }

    /// Names of the directories in `dir`. A missing directory has none.
    pub fn list_dirs(dir: &Path) -> Result<Vec<String>> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::io(dir, e)),
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| Error::io(dir, e))?;
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        names.sort();
        Ok(names)
    }

    /// Names in `dir` ending with `suffix`, without the suffix. A missing directory has no entries.
    pub fn list(dir: &Path, suffix: &str) -> Result<Vec<String>> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::io(dir, e)),
        };
        let mut names = Vec::new();
        for entry in entries {
            let name = entry.map_err(|e| Error::io(dir, e))?.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with('.') {
                if let Some(stem) = name.strip_suffix(suffix) {
                    names.push(stem.to_owned());
                }
            }
        }
        names.sort();
        Ok(names)
    }
}

fn to_json<T: Serialize>(value: &T) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(value).expect("state serializes");
    bytes.push(b'\n');
    bytes
}
