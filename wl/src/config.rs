use crate::error::{Error, Result};
use serde::Deserialize;
use std::path::PathBuf;

/// `.lifecycle.toml` as it is in one commit. The worker reads the accepted commit's version.
#[derive(Deserialize, Default, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct RepoConfig {
    #[serde(default)]
    pub validation: Vec<ValidationStep>,
    /// Ignored paths a slot keeps for the next task, such as `target`.
    #[serde(default)]
    pub slot_cache: Vec<String>,
}

#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ValidationStep {
    pub argv: Vec<String>,
    #[serde(default)]
    pub r#where: Where,
}

#[derive(Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Where {
    #[default]
    Windows,
    Any,
}

#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct RequiredCheck {
    pub app: String,
    pub name: String,
}

fn blob(repo: &gix::Repository, commit: gix::ObjectId, path: &str) -> Result<Option<Vec<u8>>> {
    let tree = repo.find_commit(commit)?.tree()?;
    match tree.lookup_entry_by_path(path)? {
        Some(entry) => Ok(Some(entry.object()?.detach().data)),
        None => Ok(None),
    }
}

pub fn repo_config(repo: &gix::Repository, commit: gix::ObjectId) -> Result<RepoConfig> {
    match blob(repo, commit, ".lifecycle.toml")? {
        Some(bytes) => {
            let text = String::from_utf8(bytes).map_err(|_| Error::new(".lifecycle.toml is not UTF-8"))?;
            toml::from_str(&text).map_err(|e| Error::new(format!(".lifecycle.toml: {e}")))
        }
        None => Ok(RepoConfig::default()),
    }
}

/// The checks `.github/required-checks.json` declares in the commit, the file the legacy lifecycle also reads.
pub fn required_checks(repo: &gix::Repository, commit: gix::ObjectId) -> Result<Vec<RequiredCheck>> {
    match blob(repo, commit, ".github/required-checks.json")? {
        Some(bytes) => serde_json::from_slice(&bytes).map_err(|e| Error::new(format!(".github/required-checks.json: {e}"))),
        None => Ok(Vec::new()),
    }
}

/// Per-user settings that must not live in a repository, such as the push preflight with its private policy path.
#[derive(Deserialize, Default, Debug)]
#[serde(deny_unknown_fields)]
pub struct MachineConfig {
    #[serde(default)]
    pub preflight: Vec<String>,
    /// Where `any` validation steps run (ADR 0007, 0014). Absent, they run on this machine.
    #[serde(default)]
    pub offload: Option<Offload>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Offload {
    /// Runs the argv that follows it in the offload host, such as `["wsl.exe", "-d", "ubuntu-26-development", "-u", "wl", "--"]`.
    pub prefix: Vec<String>,
    /// A directory on the offload host's own file system, never under `/mnt/c`.
    pub root: String,
}

pub fn machine_config_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("WL_CONFIG") {
        return Some(PathBuf::from(path));
    }
    let dir = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }?;
    Some(dir.join("wl").join("config.json"))
}

pub fn machine_config() -> Result<MachineConfig> {
    let Some(path) = machine_config_path() else { return Ok(MachineConfig::default()) };
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| Error::new(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(MachineConfig::default()),
        Err(e) => Err(Error::io(&path, e)),
    }
}
