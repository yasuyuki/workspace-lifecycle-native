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
    pub fn hold(&self, task: &str) -> PathBuf { self.tasks_dir().join(format!("{task}.hold.json")) }
    pub fn slot(&self, slot: u32) -> PathBuf { self.slots_dir().join(format!("{slot}.json")) }
    pub fn prepare_lock(&self) -> PathBuf { self.root.join("workers").join("prepare.lock") }
    pub fn claim(&self, slot: u32) -> PathBuf { self.slots_dir().join(format!("{slot}.claim")) }

    pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
        match fs::read(path) {
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
        let dir = path.parent().expect("state paths have a parent");
        fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        let temp = dir.join(format!(
            ".{}.{}.{}.tmp",
            path.file_name().unwrap().to_string_lossy(),
            std::process::id(),
            TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = fs::File::create(&temp).map_err(|e| Error::io(&temp, e))?;
        // Not flushed to disk: a foreground write lost to a crash is rebuilt by running the command again.
        file.write_all(bytes).map_err(|e| Error::io(&temp, e))?;
        Ok(temp)
    }

    /// Replaces `path` with `value` in one step; a reader sees the old or the new content.
    pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
        let temp = Self::temp_beside(path, &to_json(value))?;
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
    pub fn create_new(path: &Path, bytes: &[u8]) -> Result<bool> {
        let temp = Self::temp_beside(path, bytes)?;
        let linked = fs::hard_link(&temp, path);
        let _ = fs::remove_file(&temp);
        match linked {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(Error::io(path, e)),
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
