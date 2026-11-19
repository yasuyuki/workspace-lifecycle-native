use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

pub const VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[serde(try_from = "String", into = "String")]
pub struct TaskName(String);

impl TryFrom<String> for TaskName {
    type Error = String;
    fn try_from(value: String) -> Result<Self, String> {
        let valid = !value.is_empty()
            && value.len() <= 100
            && !value.starts_with(['.', '-'])
            && !value.ends_with(".lock")
            && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        if valid { Ok(Self(value)) } else { Err(format!("invalid task name: {value:?}")) }
    }
}

impl From<TaskName> for String {
    fn from(name: TaskName) -> String { name.0 }
}

impl fmt::Display for TaskName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.0) }
}

impl TaskName {
    pub fn as_str(&self) -> &str { &self.0 }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TaskStage {
    Claimed,
    Ready,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TaskRecord {
    pub version: u32,
    pub task: TaskName,
    pub request: String,
    pub branch: String,
    pub base: String,
    pub slot: u32,
    pub stage: TaskStage,
    /// The branch finish integrates into; absent, the remote default branch.
    #[serde(default)]
    pub target: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SlotRecord {
    pub version: u32,
    pub slot: u32,
    pub path: PathBuf,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HoldRecord {
    pub version: u32,
    pub reason: String,
}

/// Content of a slot claim file: the task that owns the slot, or a slot still being prepared.
pub const PREPARING: &str = "#prepare";

/// Written only by `wl finish`: the commit one finish attempt integrates.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Acceptance {
    pub version: u32,
    pub task: TaskName,
    pub attempt: u32,
    pub commit: String,
    pub target: String,
    pub retire: bool,
    pub accepted_at_ms: u128,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    Accepted,
    TopicPushed,
    Prepared,
    TargetPushed,
    Integrated,
    Retired,
    NeedsAction,
}

impl Stage {
    pub fn is_terminal(self, retire: bool) -> bool {
        match self {
            Stage::Retired | Stage::NeedsAction => true,
            Stage::Integrated => !retire,
            _ => false,
        }
    }
}

/// Written only by the worker of the task: how far the attempt got.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    pub version: u32,
    pub task: TaskName,
    pub attempt: u32,
    pub stage: Stage,
    /// The commit prepared for the target: the accepted commit when it fast-forwards, else a merge commit.
    pub merged: Option<String>,
    /// The target commit the preparation started from.
    pub target_base: Option<String>,
    pub detail: Option<String>,
}

/// Created by the worker; the next command that reports it deletes it.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Notice {
    pub task: TaskName,
    pub attempt: u32,
    pub stage: Stage,
    pub message: String,
}
