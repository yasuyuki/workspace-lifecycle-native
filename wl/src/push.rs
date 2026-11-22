//! The push decision: whether a normal, single-branch `git push` may run, and to where.
//!
//! This is the decision table of the legacy `workspace_lifecycle.push` (workspace-lifecycle 0.8.27), moved here
//! unchanged so that `wl` no longer starts Python (ADR 0012, 0016). `decide` reads only its arguments; `collect_state`
//! reads Git and the GitHub API and never writes. Invalid state and unknown evidence fail closed.

use crate::error::{Error, Result};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;

const OSI_LICENSES: [&str; 4] = ["MIT", "Apache-2.0", "BSD-3-Clause", "ISC"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Intent {
    Auto,
    Push,
    Hold,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Oss {
    Auto,
    Yes,
    No,
}

/// One decision. `push_argv` is present only for `push`.
pub fn decision(decision: &str, reason: &str, remote: Option<&str>, destination: Option<&str>) -> Value {
    let mut value = json!({ "decision": decision, "reason": reason, "remote": remote, "destination": destination });
    if decision == "push" {
        let (remote, destination) = (remote.unwrap_or_default(), destination.unwrap_or_default());
        value["push_argv"] = json!(["git", "push", remote, format!("HEAD:refs/heads/{destination}")]);
    }
    value
}

/// The exit status of a decision: push 0, hold 10, ask 11.
pub fn exit_code(decision: &Value) -> u8 {
    match decision["decision"].as_str() {
        Some("push") => 0,
        Some("hold") => 10,
        _ => 11,
    }
}

/// Python truthiness of a JSON value.
fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// `value or {}` for an object, and `value or []` for a list.
fn or_object(value: Option<&Value>) -> Map<String, Value> {
    match value {
        Some(Value::Object(o)) if !o.is_empty() => o.clone(),
        _ => Map::new(),
    }
}

fn or_list(value: Option<&Value>) -> Vec<Value> {
    match value {
        Some(Value::Array(a)) if !a.is_empty() => a.clone(),
        _ => Vec::new(),
    }
}

fn valid_name(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::String(s)) if !s.is_empty() && !s.starts_with('-') && !s.contains('\0'))
}

fn choose_remote(state: &Value) -> Option<Value> {
    let candidates = or_object(state.get("candidates"));
    for key in ["branch_push_remote", "remote_push_default", "branch_remote"] {
        if let Some(value) = candidates.get(key) {
            if !value.is_null() {
                return Some(value.clone());
            }
        }
    }
    let remotes = or_object(state.get("remotes"));
    if remotes.len() == 1 {
        remotes.keys().next().map(|k| Value::String(k.clone()))
    } else {
        None
    }
}

/// The destination branch for `branch`: its upstream on `remote`, otherwise a branch of the same name.
fn destination_for(state: &Value, remote: &str, branch: &Value) -> Option<Value> {
    let upstream = or_object(state.get("upstream"));
    if upstream.get("remote").and_then(Value::as_str) != Some(remote) {
        return Some(branch.clone());
    }
    match upstream.get("merge") {
        None | Some(Value::Null) => Some(branch.clone()),
        Some(Value::String(merge)) if merge.len() > "refs/heads/".len() && merge.starts_with("refs/heads/") => {
            Some(Value::String(merge["refs/heads/".len()..].to_owned()))
        }
        Some(_) => None,
    }
}

/// Returns the decision for `state`, the shape `collect_state` produces.
pub fn decide(state: &Value, intent: Intent, temporary: bool, oss: Oss, policy: Option<&Value>) -> Value {
    if intent == Intent::Hold {
        return decision("hold", "USER_HOLD", None, None);
    }
    if temporary && intent != Intent::Push {
        return decision("hold", "TEMPORARY_COMMIT", None, None);
    }
    let mut allowed: BTreeSet<Identity> = BTreeSet::new();
    if intent == Intent::Auto {
        if let Some(policy) = policy {
            match policy_repositories(policy) {
                Ok(found) => allowed = found,
                Err(()) => return decision("ask", "INVALID_PUSH_POLICY", None, None),
            }
        }
    }
    if truthy(state.get("collection_error")) {
        return decision("ask", "COLLECTION_ERROR", None, None);
    }

    let branch = state.get("current_branch").cloned().unwrap_or(Value::Null);
    let Some(remote_value) = choose_remote(state) else {
        return decision("ask", "NO_PUSH_REMOTE", None, None);
    };
    let remotes = or_object(state.get("remotes"));
    let remote = match remote_value.as_str() {
        Some(r) if valid_name(Some(&remote_value)) && r != "." && remotes.contains_key(r) => r.to_owned(),
        _ => return decision("ask", "INVALID_PUSH_REMOTE", None, None),
    };
    let remote = remote.as_str();
    let info = or_object(remotes.get(remote));
    let push_urls = or_list(info.get("push_urls"));
    let fetch_urls = or_list(info.get("fetch_urls"));
    if push_urls.len() > 1 {
        return decision("ask", "AMBIGUOUS_PUSH_URL", Some(remote), None);
    }
    let effective_fetch = match info.get("effective_fetch_urls") {
        Some(v) => or_list(Some(v)),
        None => fetch_urls.clone(),
    };
    let effective_push = match info.get("effective_push_urls") {
        Some(v) => or_list(Some(v)),
        None if !push_urls.is_empty() => push_urls.clone(),
        None => fetch_urls.clone(),
    };
    if effective_push.len() != 1 || effective_fetch.len() != 1 {
        return decision("ask", "AMBIGUOUS_PUSH_URL", Some(remote), None);
    }
    let push_identity = effective_push[0].as_str().and_then(repo_identity);
    let fetch_identity = effective_fetch[0].as_str().and_then(repo_identity);
    let (Some(push_identity), Some(fetch_identity)) = (push_identity, fetch_identity) else {
        return decision("ask", "INVALID_REMOTE_URL", Some(remote), None);
    };
    if push_identity != fetch_identity {
        return decision("ask", "PUSH_REPOSITORY_MISMATCH", Some(remote), None);
    }
    if push_urls.is_empty() && fetch_urls.len() != 1 {
        return decision("ask", "AMBIGUOUS_FETCH_URL", Some(remote), None);
    }
    if truthy(info.get("push_refspecs")) {
        return decision("ask", "CUSTOM_PUSH_REFSPEC", Some(remote), None);
    }
    if truthy(info.get("mirror_invalid")) {
        return decision("ask", "INVALID_MIRROR_SETTING", Some(remote), None);
    }
    if truthy(info.get("mirror")) {
        return decision("ask", "MIRROR_REMOTE", Some(remote), None);
    }

    let repo = or_object(state.get("repo"));
    let visibility = repo.get("visibility").and_then(Value::as_str);
    if intent == Intent::Auto {
        if truthy(repo.get("metadata_error")) || !matches!(visibility, Some("private" | "public")) {
            return decision("ask", "METADATA_UNAVAILABLE", Some(remote), None);
        }
        if visibility == Some("public") {
            if oss == Oss::No {
                return decision("hold", "NON_OSS_PUBLIC_REPOSITORY", Some(remote), None);
            }
            let license = repo.get("license_spdx_id").and_then(Value::as_str);
            if oss != Oss::Yes && !license.is_some_and(|l| OSI_LICENSES.contains(&l)) {
                return decision("ask", "UNKNOWN_OSS_LICENSE", Some(remote), None);
            }
        }
    }
    if !valid_name(Some(&branch)) {
        return decision("ask", "DETACHED_HEAD", Some(remote), None);
    }
    let upstream = or_object(state.get("upstream"));
    if let Some(Value::Array(values)) = upstream.get("merge_values") {
        if values.len() > 1 {
            return decision("ask", "INVALID_UPSTREAM", Some(remote), None);
        }
    }
    let destination = match destination_for(state, remote, &branch) {
        Some(Value::String(d)) if valid_name(Some(&Value::String(d.clone()))) && !d.starts_with("refs/") => d,
        _ => return decision("ask", "INVALID_UPSTREAM", Some(remote), None),
    };
    let destination = destination.as_str();
    if intent == Intent::Push {
        return decision("push", "PUSH_ALLOWED", Some(remote), Some(destination));
    }

    let default_branch = match repo.get("default_branch") {
        Some(Value::String(d)) if valid_name(Some(&Value::String(d.clone()))) => d.clone(),
        _ => return decision("ask", "DEFAULT_BRANCH_UNAVAILABLE", Some(remote), Some(destination)),
    };
    if branch.as_str() == Some(default_branch.as_str()) || destination == default_branch {
        let configured = policy.and_then(Value::as_object).cloned().unwrap_or_default();
        let excluded: Vec<Option<Identity>> = configured
            .get("default_branch_push_excluded_repositories")
            .and_then(Value::as_array)
            .map(|urls| urls.iter().map(|u| u.as_str().and_then(repo_identity)).collect())
            .unwrap_or_default();
        if excluded.contains(&Some(push_identity.clone())) {
            return decision("hold", "DEFAULT_BRANCH_PUSH_EXCLUDED", Some(remote), Some(destination));
        }
        let category = if visibility == Some("private") { "private" } else { "oss" };
        if allowed.contains(&push_identity) || truthy(configured.get(&format!("default_branch_push_{category}"))) {
            return decision("push", "DEFAULT_BRANCH_PUSH_ALLOWED", Some(remote), Some(destination));
        }
        return decision("hold", "DEFAULT_BRANCH_PROTECTED", Some(remote), Some(destination));
    }
    decision("push", "PUSH_ALLOWED", Some(remote), Some(destination))
}

/// A repository identity: `("local", path)`, `("opaque", url)` or `(github.com, owner/repo)`.
pub type Identity = (String, String);

/// The parts of a URL, as Python's `urllib.parse.urlsplit` splits them.
struct Split {
    scheme: String,
    netloc: String,
    path: String,
    query: String,
    fragment: String,
}

impl Split {
    fn parse(url: &str) -> Option<Split> {
        let mut rest = url;
        let mut scheme = String::new();
        if let Some(i) = url.find(':') {
            let candidate = &url[..i];
            let mut chars = candidate.chars();
            if chars.next().is_some_and(|c| c.is_ascii_alphabetic())
                && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
            {
                scheme = candidate.to_ascii_lowercase();
                rest = &url[i + 1..];
            }
        }
        let mut netloc = String::new();
        if let Some(after) = rest.strip_prefix("//") {
            let end = after.find(['/', '?', '#']).unwrap_or(after.len());
            netloc = after[..end].to_owned();
            rest = &after[end..];
            if netloc.contains('[') != netloc.contains(']') {
                return None; // Python raises "Invalid IPv6 URL".
            }
        }
        let (rest, fragment) = rest.split_once('#').map_or((rest, ""), |(a, b)| (a, b));
        let (path, query) = rest.split_once('?').map_or((rest, ""), |(a, b)| (a, b));
        Some(Split { scheme, netloc, path: path.to_owned(), query: query.to_owned(), fragment: fragment.to_owned() })
    }

    fn host_and_port(&self) -> (&str, &str) {
        let hostinfo = self.netloc.rsplit_once('@').map_or(self.netloc.as_str(), |(_, h)| h);
        match hostinfo.split_once('[') {
            Some((_, bracketed)) => {
                let (host, after) = bracketed.split_once(']').unwrap_or((bracketed, ""));
                (host, after.split_once(':').map_or("", |(_, p)| p))
            }
            None => hostinfo.split_once(':').unwrap_or((hostinfo, "")),
        }
    }

    /// `hostname`: lowercased, or None when empty.
    fn hostname(&self) -> Option<String> {
        let host = self.host_and_port().0;
        (!host.is_empty()).then(|| host.to_lowercase())
    }

    /// `port`: Err where Python raises ValueError.
    fn port(&self) -> std::result::Result<Option<u32>, ()> {
        let port = self.host_and_port().1;
        if port.is_empty() {
            return Ok(None);
        }
        if !port.chars().all(|c| c.is_ascii_digit()) {
            return Err(());
        }
        match port.parse::<u64>() {
            Ok(p) if p <= 65535 => Ok(Some(p as u32)),
            _ => Err(()),
        }
    }
}

fn is_abs(path: &str) -> bool {
    Path::new(path).is_absolute()
}

/// Lexical normalization, as `os.path.normpath`. Only used to compare two local paths.
fn normpath(path: &str) -> String {
    let sep = if cfg!(windows) { '\\' } else { '/' };
    let path = if cfg!(windows) { path.replace('/', "\\") } else { path.to_owned() };
    let (prefix, rest) = if cfg!(windows) {
        if path.len() >= 2 && path.as_bytes()[1] == b':' {
            path.split_at(2)
        } else if path.starts_with("\\\\") {
            let parts: Vec<&str> = path[2..].splitn(3, '\\').collect();
            let len = if parts.len() >= 2 { 2 + parts[0].len() + 1 + parts[1].len() } else { path.len() };
            path.split_at(len)
        } else {
            ("", path.as_str())
        }
    } else {
        ("", path.as_str())
    };
    let root = rest.starts_with(sep);
    let leading = if !cfg!(windows) && rest.starts_with("//") && !rest.starts_with("///") { "//" } else if root { if cfg!(windows) { "\\" } else { "/" } } else { "" };
    let mut parts: Vec<&str> = Vec::new();
    for part in rest.split(sep) {
        match part {
            "" | "." => {}
            ".." if parts.last().is_some_and(|p| *p != "..") => {
                parts.pop();
            }
            ".." if root => {}
            other => parts.push(other),
        }
    }
    let joined = parts.join(&sep.to_string());
    let result = format!("{prefix}{leading}{joined}");
    if result.is_empty() { ".".to_owned() } else { result }
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 3 <= bytes.len() {
            let hex = &bytes[i + 1..i + 3];
            if let Some(b) = hex.iter().all(u8::is_ascii_hexdigit).then(|| std::str::from_utf8(hex).ok()).flatten().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// `urllib.request.url2pathname` of a `file:` URL path.
fn url2pathname(path: &str) -> Option<String> {
    let decoded = percent_decode(path)?;
    if cfg!(windows) {
        let b = decoded.as_bytes();
        if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && matches!(b[2], b':' | b'|') {
            return Some(format!("{}:{}", &decoded[1..2], decoded[3..].replace('/', "\\")));
        }
        Some(decoded.replace('/', "\\"))
    } else {
        Some(decoded)
    }
}

/// Normalizes a supported remote URL without returning credentials.
pub fn repo_identity(url: &str) -> Option<Identity> {
    if url.is_empty() || url.contains('\0') {
        return None;
    }
    let trimmed = url.trim();
    let value = trimmed.strip_prefix("git+").unwrap_or(trimmed);
    // Git accepts both native absolute paths and local file URLs. Preserve path case and symlink spelling.
    if is_abs(value) {
        return Some(("local".into(), normpath(value)));
    }
    if value.starts_with("file://") {
        let parsed = Split::parse(value)?;
        if !parsed.netloc.is_empty() || !parsed.query.is_empty() || !parsed.fragment.is_empty() {
            return None;
        }
        let path = url2pathname(&parsed.path)?;
        return is_abs(&path).then(|| ("local".into(), normpath(&path)));
    }
    let (host, path) = match scp_like(value) {
        Some((host, path)) if !value.contains("://") => (host.to_owned(), path.to_owned()),
        _ => {
            let parsed = Split::parse(value)?;
            if !parsed.query.is_empty() || !parsed.fragment.is_empty() || (parsed.scheme == "file" && !parsed.netloc.is_empty()) {
                return None;
            }
            let default_port = match parsed.scheme.as_str() {
                "http" => Some(80),
                "https" => Some(443),
                "ssh" => Some(22),
                "git" => Some(9418),
                _ => None,
            };
            match (default_port, parsed.hostname()) {
                (Some(default), Some(hostname)) => {
                    let port = parsed.port().ok()?;
                    let host = match port {
                        None => hostname,
                        Some(p) if p == default => hostname,
                        Some(p) => format!("{hostname}:{p}"),
                    };
                    (host, parsed.path.clone())
                }
                _ if parsed.scheme == "file" => ("file".to_owned(), parsed.path.clone()),
                _ if parsed.scheme.is_empty() && value.starts_with('/') => ("file".to_owned(), value.to_owned()),
                _ => return None,
            }
        }
    };
    // Only GitHub establishes cross-transport repository identity here.
    if host.to_lowercase() != "github.com" {
        return Some(("opaque".into(), value.to_owned()));
    }
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if path.is_empty() {
        return None;
    }
    Some((host.to_lowercase(), path.to_owned()))
}

/// SCP-like SSH syntax `user@host:owner/repo.git`: `(?:[^@/:]+@)?([^/:]+):(.+)`.
fn scp_like(value: &str) -> Option<(&str, &str)> {
    let colon = value.find(':')?;
    let (head, path) = (&value[..colon], &value[colon + 1..]);
    if path.is_empty() {
        return None;
    }
    let host = match head.split_once('@') {
        Some((user, host)) => {
            if user.is_empty() || user.contains(['/', ':']) {
                return None;
            }
            host
        }
        None => head,
    };
    (!host.is_empty() && !host.contains(['/', ':', '@'])).then_some((host, path))
}

/// Validates the default-branch policy: exact repository URLs, never local paths or patterns. Returns the allowed identities.
pub fn policy_repositories(policy: &Value) -> std::result::Result<BTreeSet<Identity>, ()> {
    const KEY: &str = "default_branch_push_repositories";
    const EXCLUDED: &str = "default_branch_push_excluded_repositories";
    const FLAGS: [&str; 2] = ["default_branch_push_private", "default_branch_push_oss"];
    let object = policy.as_object().ok_or(())?;
    let repositories = object.get(KEY).and_then(Value::as_array).ok_or(())?;
    if object.keys().any(|k| k != KEY && k != EXCLUDED && !FLAGS.contains(&k.as_str())) {
        return Err(());
    }
    let excluded = match object.get(EXCLUDED) {
        None => &Vec::new(),
        Some(Value::Array(a)) => a,
        Some(_) => return Err(()),
    };
    if FLAGS.iter().any(|f| object.get(*f).is_some_and(|v| !v.is_boolean())) {
        return Err(());
    }
    let mut identities = BTreeSet::new();
    for url in repositories.iter().chain(excluded.iter()) {
        let url = url.as_str().filter(|u| !u.is_empty()).ok_or(())?;
        if url.chars().any(|c| c.is_whitespace() || "*?[]".contains(c)) {
            return Err(());
        }
        let value = url.strip_prefix("git+").unwrap_or(url);
        if value.contains("://") {
            let parsed = Split::parse(value).ok_or(())?;
            if !matches!(parsed.scheme.as_str(), "http" | "https" | "ssh" | "git")
                || parsed.hostname().is_none()
                || parsed.path.trim_matches('/').is_empty()
            {
                return Err(());
            }
        } else {
            let (user, rest) = value.split_once('@').ok_or(())?;
            let (host, path) = rest.split_once(':').ok_or(())?;
            if user.is_empty() || user.contains(['/', ':']) || host.is_empty() || host.contains(['/', '@']) || path.is_empty() {
                return Err(());
            }
        }
        let identity = repo_identity(url).ok_or(())?;
        if repositories.iter().any(|r| r.as_str() == Some(url)) {
            identities.insert(identity);
        }
    }
    Ok(identities)
}

/// Reads the policy file. A file that cannot be read or is invalid becomes `{}`, which `decide` refuses.
pub fn load_policy(path: &Path) -> Value {
    let parsed = std::fs::read(path)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    match parsed {
        Some(policy) if policy_repositories(&policy).is_ok() => policy,
        _ => json!({}),
    }
}

/// `git -C repo args`, its output decoded as UTF-8 with universal newlines and the final newlines removed.
fn run_git(repo: &Path, args: &[&str], required: bool) -> std::result::Result<Option<String>, ()> {
    let output = crate::repo::command("git").arg("-C").arg(repo).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().map_err(|_| ())?;
    if !output.status.success() {
        return if required { Err(()) } else { Ok(None) };
    }
    let text = String::from_utf8(output.stdout).map_err(|_| ())?;
    Ok(Some(text.replace("\r\n", "\n").replace('\r', "\n").trim_end_matches('\n').to_owned()))
}

/// Python's `str.splitlines()` for text whose line breaks are `\n` (universal newlines already applied).
fn splitlines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    text.strip_suffix('\n').unwrap_or(text).split('\n').map(str::to_owned).collect()
}

/// The case-insensitive parts of a config key lowercased; a subsection keeps its case.
fn config_key(key: &str) -> String {
    let (section, rest) = key.split_once('.').unwrap_or((key, ""));
    match rest.rsplit_once('.') {
        Some((subsection, variable)) => format!("{}.{}.{}", section.to_lowercase(), subsection, variable.to_lowercase()),
        None => format!("{}.{}", section.to_lowercase(), rest.to_lowercase()),
    }
}

struct Config(std::collections::HashMap<String, Vec<String>>);

impl Config {
    fn read(repo: &Path) -> std::result::Result<Config, ()> {
        let output = crate::repo::command("git").arg("-C").arg(repo).args(["config", "--list", "-z"]).stdin(Stdio::null()).stderr(Stdio::null()).output().map_err(|_| ())?;
        if !output.status.success() {
            return Err(());
        }
        let mut entries: std::collections::HashMap<String, Vec<String>> = Default::default();
        for entry in output.stdout.split(|b| *b == 0).filter(|e| !e.is_empty()) {
            let (key, value) = match entry.iter().position(|b| *b == b'\n') {
                Some(i) => (&entry[..i], &entry[i + 1..]),
                None => (entry, &[][..]),
            };
            let key = String::from_utf8_lossy(key).into_owned();
            let value = String::from_utf8(value.to_vec()).map_err(|_| ())?;
            entries.entry(config_key(&key)).or_default().push(value);
        }
        Ok(Config(entries))
    }

    fn values(&self, key: &str) -> Vec<String> {
        self.0.get(&config_key(key)).cloned().unwrap_or_default()
    }

    /// `git config --get`: the last value.
    fn last(&self, key: &str) -> Option<String> {
        self.values(key).pop()
    }

    /// `git config --get-all` split into lines.
    fn lines(&self, key: &str) -> Vec<String> {
        splitlines(&self.values(key).join("\n"))
    }
}

fn github_repo(url: &str) -> Option<String> {
    let (host, path) = repo_identity(url)?;
    let pieces: Vec<&str> = path.split('/').collect();
    (host == "github.com" && pieces.len() == 2 && pieces.iter().all(|p| !p.is_empty())).then_some(path)
}

fn default_from_ls_remote(repo: &Path, remote: &str) -> Option<String> {
    let text = run_git(repo, &["ls-remote", "--symref", remote, "HEAD"], false).ok()??;
    text.lines().find_map(|line| {
        let rest = line.strip_prefix("ref: refs/heads/")?;
        let (name, head) = rest.split_once('\t')?;
        (head == "HEAD" && !name.is_empty() && !name.contains('\t')).then(|| name.to_owned())
    })
}

/// Reads `repos/<slug>` from the GitHub API. None when it cannot be read, which `decide` treats as unknown metadata.
fn github_metadata(slug: &str) -> Option<Value> {
    crate::finish::GitHub::connect().and_then(|github| github.get(&format!("repos/{slug}"))).ok()
}

/// Collects the read-only Git and GitHub data `decide` reads. Errors become `{"collection_error": true}`.
pub fn collect_state(repo: &Path, need_metadata: bool) -> Value {
    collect(repo, need_metadata, &github_metadata).unwrap_or_else(|()| json!({ "collection_error": true }))
}

fn collect(repo: &Path, need_metadata: bool, metadata: &dyn Fn(&str) -> Option<Value>) -> std::result::Result<Value, ()> {
    let root = PathBuf::from(run_git(repo, &["rev-parse", "--show-toplevel"], true)?.unwrap_or_default());
    let branch = run_git(&root, &["symbolic-ref", "--quiet", "--short", "HEAD"], false)?;
    let names = splitlines(&run_git(&root, &["remote"], true)?.unwrap_or_default());
    let config = Config::read(&root)?;
    let urls = |name: &str, push: bool| -> std::result::Result<Vec<String>, ()> {
        let mut args = vec!["remote", "get-url", "--all"];
        if push {
            args.push("--push");
        }
        args.push(name);
        Ok(splitlines(&run_git(&root, &args, false)?.unwrap_or_default()))
    };
    let mut remotes = Map::new();
    for name in &names {
        let mirror = config.last(&format!("remote.{name}.mirror"));
        remotes.insert(
            name.clone(),
            json!({
                "fetch_urls": config.lines(&format!("remote.{name}.url")),
                "push_urls": config.lines(&format!("remote.{name}.pushurl")),
                "effective_fetch_urls": urls(name, false)?,
                "effective_push_urls": urls(name, true)?,
                "push_refspecs": config.lines(&format!("remote.{name}.push")),
                "mirror": mirror.as_deref() == Some("true"),
                "mirror_invalid": !matches!(mirror.as_deref(), None | Some("true" | "false")),
            }),
        );
    }
    let mut candidates = json!({ "branch_push_remote": null, "remote_push_default": config.last("remote.pushDefault"), "branch_remote": null });
    let mut upstream = json!({ "remote": null, "merge": null });
    if let Some(branch) = branch.as_deref().filter(|b| !b.is_empty()) {
        candidates["branch_push_remote"] = json!(config.last(&format!("branch.{branch}.pushRemote")));
        candidates["branch_remote"] = json!(config.last(&format!("branch.{branch}.remote")));
        let merges = config.lines(&format!("branch.{branch}.merge"));
        upstream = json!({
            "remote": candidates["branch_remote"].clone(),
            "merge": if merges.len() == 1 { Value::String(merges[0].clone()) } else { Value::Null },
            "merge_values": merges,
        });
    }
    let mut state = json!({
        "current_branch": branch,
        "candidates": candidates,
        "remotes": remotes,
        "upstream": upstream,
        "repo": { "host": null, "visibility": null, "default_branch": null, "license_spdx_id": null, "metadata_error": false },
    });
    let remote = match choose_remote(&state) {
        Some(Value::String(r)) if need_metadata && state["remotes"].get(&r).is_some() && valid_name(Some(&Value::String(r.clone()))) => r,
        _ => return Ok(state),
    };
    let info = &state["remotes"][&remote];
    let chosen = if truthy(info.get("effective_push_urls")) { info["effective_push_urls"].clone() } else { info["effective_fetch_urls"].clone() };
    let chosen = chosen.as_array().cloned().unwrap_or_default();
    if chosen.len() != 1 {
        return Ok(state);
    }
    let Some(slug) = chosen[0].as_str().and_then(github_repo) else {
        state["repo"]["host"] = json!("other");
        state["repo"]["default_branch"] = json!(default_from_ls_remote(&root, &remote));
        return Ok(state);
    };
    state["repo"]["host"] = json!("github.com");
    match metadata(&slug) {
        None => state["repo"]["metadata_error"] = json!(true),
        Some(data) => {
            let data = data.as_object().ok_or(())?;
            state["repo"]["visibility"] = match data.get("private") {
                Some(Value::Bool(true)) => json!("private"),
                Some(Value::Bool(false)) => json!("public"),
                _ => Value::Null,
            };
            state["repo"]["default_branch"] = data.get("default_branch").cloned().unwrap_or(Value::Null);
            let license = match data.get("license") {
                Some(Value::Object(o)) => o.clone(),
                Some(v) if !truthy(Some(v)) => Map::new(),
                Some(_) => return Err(()),
                None => Map::new(),
            };
            state["repo"]["license_spdx_id"] = license.get("spdx_id").cloned().unwrap_or(Value::Null);
        }
    }
    if !truthy(state["repo"].get("default_branch")) {
        state["repo"]["default_branch"] = json!(default_from_ls_remote(&root, &remote));
    }
    Ok(state)
}

/// The whole check for `repo`: collect, decide, and hold a push from a repository the pre-runtime-split tool still owns.
pub fn preflight(repo: &Path, intent: Intent, temporary: bool, oss: Oss, policy_path: Option<&Path>) -> Result<Value> {
    let need_metadata = intent == Intent::Auto && !temporary;
    let policy = match policy_path {
        Some(path) if need_metadata => Some(load_policy(path)),
        _ => None,
    };
    let mut output = decide(&collect_state(repo, need_metadata), intent, temporary, oss, policy.as_ref());
    if output["decision"] == "push" {
        let common = run_git(repo, &["rev-parse", "--path-format=absolute", "--git-common-dir"], true)
            .map_err(|()| Error::new(format!("cannot read the Git directory of {}", repo.display())))?
            .unwrap_or_default();
        if Path::new(&common).join("agent-branches").join("state.json").exists() {
            output = decision("hold", "LEGACY_CONSUMER: use its pinned preflight until explicit migration", None, None);
        }
    }
    Ok(output)
}

const HELP: &str = "usage: wl preflight [REPO] [--user-intent auto|push|hold] [--temporary] [--oss auto|yes|no] [--policy PATH]

Decide whether to make a normal, single-branch push without changing Git state.

  REPO           target repository (default: --repo, else the current directory)
  --user-intent  explicit user push/hold instruction; default auto applies repository and branch policy
  --temporary    documented temporary-save intent, not inferred from a WIP subject; auto holds the push
  --oss          explicit OSS/non-OSS classification; auto uses hosting license metadata
  --policy       environment-supplied default-branch push policy JSON; never omit a configured file

Outputs JSON decision (push/hold/ask), reason, remote and destination. Exit status: push 0, hold 10,
ask 11; 2 is invalid usage. Only push includes push_argv; execute it in the target repository. For hold
report the reason. For ask resolve the missing information or obtain the user's decision before running
the preflight again. Normal Git hooks and history protections still apply.";

/// `wl preflight [REPO] [--user-intent auto|push|hold] [--temporary] [--oss auto|yes|no] [--policy PATH]`. Prints one
/// decision; exits 0 for push, 10 for hold, 11 for ask and 2 for invalid usage, so `wl preflight && git push ...`
/// pushes nothing after hold or ask. Run the printed `push_argv` rather than a hand-written push.
pub fn cli(repo: &Path, mut args: crate::Args) -> std::process::ExitCode {
    if args.flag("help") {
        println!("{HELP}");
        return std::process::ExitCode::SUCCESS;
    }
    let run = |args: &mut crate::Args| -> Result<Value> {
        let intent = match args.option("user-intent")?.as_deref() {
            None | Some("auto") => Intent::Auto,
            Some("push") => Intent::Push,
            Some("hold") => Intent::Hold,
            Some(other) => return Err(Error::new(format!("--user-intent must be auto, push or hold, not {other:?}"))),
        };
        let temporary = args.flag("temporary");
        let oss = match args.option("oss")?.as_deref() {
            None | Some("auto") => Oss::Auto,
            Some("yes") => Oss::Yes,
            Some("no") => Oss::No,
            Some(other) => return Err(Error::new(format!("--oss must be auto, yes or no, not {other:?}"))),
        };
        let policy = args.option("policy")?.map(PathBuf::from);
        let target = args.optional_positional().map(PathBuf::from).unwrap_or_else(|| repo.to_path_buf());
        std::mem::replace(args, crate::Args { rest: Vec::new() }).finish()?;
        preflight(&target, intent, temporary, oss, policy.as_deref())
    };
    match run(&mut args) {
        Ok(value) => {
            println!("{}", to_python_json(&value));
            std::process::ExitCode::from(exit_code(&value))
        }
        Err(error) => {
            eprintln!("wl preflight: {error}");
            std::process::ExitCode::from(2)
        }
    }
}

/// JSON as Python's `json.dumps(..., sort_keys=True, separators=(",", ":"))` writes it, non-ASCII escaped.
pub fn to_python_json(value: &Value) -> String {
    let text = serde_json::to_string(value).expect("a decision serializes");
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut units = [0u16; 2];
            for unit in c.encode_utf16(&mut units) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    //! The legacy `tests/test_push_preflight.py` cases, with the same fixtures and expectations.
    use super::*;

    const FIXTURES: &str = include_str!("../tests/fixtures/push_preflight.json");

    fn base() -> Value {
        json!({
            "current_branch": "feature",
            "candidates": {"branch_push_remote": null, "remote_push_default": null, "branch_remote": null},
            "remotes": {"origin": {"fetch_urls": ["https://github.com/example/project.git"],
                                   "push_urls": [], "push_refspecs": [], "mirror": false}},
            "upstream": {"remote": null, "merge": null},
            "repo": {"host": "github", "visibility": "private", "default_branch": "main",
                     "license_spdx_id": null, "metadata_error": false},
        })
    }

    fn reason(state: &Value, policy: Option<&Value>) -> String {
        decide(state, Intent::Auto, false, Oss::Auto, policy)["reason"].as_str().unwrap().to_owned()
    }

    struct Inputs {
        intent: Intent,
        temporary: bool,
        oss: Oss,
    }

    fn inputs(fixture: &Value) -> Inputs {
        let i = &fixture["inputs"];
        Inputs {
            intent: match i["user_intent"].as_str() {
                Some("push") => Intent::Push,
                Some("hold") => Intent::Hold,
                _ => Intent::Auto,
            },
            temporary: i["temporary"].as_bool().unwrap_or(false),
            oss: match i["oss"].as_str() {
                Some("yes") => Oss::Yes,
                Some("no") => Oss::No,
                _ => Oss::Auto,
            },
        }
    }

    fn fixture_state(fixture: &Value) -> Value {
        let mut state = base();
        if let Some(changes) = fixture["state"].as_object() {
            for (key, value) in changes {
                match value {
                    Value::Object(fields) => {
                        for (k, v) in fields {
                            state[key][k] = v.clone();
                        }
                    }
                    other => state[key] = other.clone(),
                }
            }
        }
        state
    }

    fn fixtures() -> Vec<Value> {
        serde_json::from_str::<Value>(FIXTURES).unwrap().as_array().unwrap().clone()
    }

    #[test]
    fn policy_fixtures() {
        let fixtures = fixtures();
        assert_eq!(fixtures.len(), 28);
        for fixture in fixtures {
            let state = fixture_state(&fixture);
            let before = state.clone();
            let i = inputs(&fixture);
            let result = decide(&state, i.intent, i.temporary, i.oss, None);
            let name = &fixture["name"];
            assert_eq!(result["decision"], fixture["decision"], "{name}: {result}");
            assert_eq!(result["reason"], fixture["reason"], "{name}");
            assert_eq!(state, before);
            if result["decision"] == "push" {
                let remote = fixture.get("remote").and_then(Value::as_str).unwrap_or("origin");
                let destination = fixture.get("destination").and_then(Value::as_str).unwrap_or("feature");
                assert_eq!(result["remote"], remote, "{name}");
                assert_eq!(result["destination"], destination, "{name}");
                assert_eq!(result["push_argv"], json!(["git", "push", remote, format!("HEAD:refs/heads/{destination}")]));
            } else {
                assert!(result.get("push_argv").is_none(), "{name}");
            }
        }
    }

    #[test]
    fn default_branch_allowlist() {
        let mut state = base();
        state["current_branch"] = json!("main");
        let policy = json!({"default_branch_push_repositories": ["git@github.com:example/project.git"]});
        for configured in [None, Some(json!({"default_branch_push_repositories": []})),
                           Some(json!({"default_branch_push_repositories": ["https://github.com/other/project"]}))] {
            assert_eq!(reason(&state, configured.as_ref()), "DEFAULT_BRANCH_PROTECTED");
        }
        let allowed = decide(&state, Intent::Auto, false, Oss::Auto, Some(&policy));
        assert_eq!(allowed["reason"], "DEFAULT_BRANCH_PUSH_ALLOWED");
        assert_eq!(allowed["push_argv"], json!(["git", "push", "origin", "HEAD:refs/heads/main"]));
        state["current_branch"] = json!("feature");
        assert_eq!(reason(&state, Some(&policy)), "PUSH_ALLOWED");
        state["upstream"] = json!({"remote": "origin", "merge": "refs/heads/main"});
        assert_eq!(decide(&state, Intent::Auto, false, Oss::Auto, Some(&policy))["destination"], "main");
        state["current_branch"] = json!("trunk");
        state["repo"]["default_branch"] = json!("trunk");
        state["upstream"] = json!({});
        assert_eq!(reason(&state, Some(&policy)), "DEFAULT_BRANCH_PUSH_ALLOWED");
        state["remotes"]["fork"] = json!({"fetch_urls": ["https://github.com/fork/project.git"]});
        state["candidates"]["branch_push_remote"] = json!("fork");
        assert_eq!(reason(&state, Some(&policy)), "DEFAULT_BRANCH_PROTECTED");
    }

    fn grants_preserve_restrictions(policy: Value) {
        for fixture in fixtures() {
            if fixture["reason"] == "DEFAULT_BRANCH_PROTECTED" {
                continue;
            }
            let i = inputs(&fixture);
            let actual = decide(&fixture_state(&fixture), i.intent, i.temporary, i.oss, Some(&policy));
            assert_eq!(actual["reason"], fixture["reason"], "{}", fixture["name"]);
            assert_eq!(actual["decision"], fixture["decision"], "{}", fixture["name"]);
        }
    }

    #[test]
    fn allowlist_preserves_existing_restrictions() {
        grants_preserve_restrictions(json!({"default_branch_push_repositories": ["https://github.com/example/project"]}));
    }

    #[test]
    fn category_grants_preserve_existing_restrictions() {
        grants_preserve_restrictions(json!({"default_branch_push_repositories": [],
            "default_branch_push_private": true, "default_branch_push_oss": true}));
    }

    #[test]
    fn allowlist_uses_existing_repository_identity() {
        let mut state = base();
        state["current_branch"] = json!("main");
        state["remotes"]["origin"]["fetch_urls"] = json!(["https://example.com/team/repo"]);
        for (url, expected) in [("git+https://example.com/team/repo", "DEFAULT_BRANCH_PUSH_ALLOWED"),
                                ("ssh://git@example.com/team/repo", "DEFAULT_BRANCH_PROTECTED"),
                                ("https://example.com/team/repo.git", "DEFAULT_BRANCH_PROTECTED")] {
            assert_eq!(reason(&state, Some(&json!({"default_branch_push_repositories": [url]}))), expected, "{url}");
        }
    }

    #[test]
    fn category_grants_and_exclusions() {
        let mut state = base();
        state["current_branch"] = json!("main");
        let mut policy = json!({"default_branch_push_repositories": [],
            "default_branch_push_private": true, "default_branch_push_oss": true});
        assert_eq!(reason(&state, Some(&policy)), "DEFAULT_BRANCH_PUSH_ALLOWED");
        state["repo"]["visibility"] = json!("public");
        state["repo"]["license_spdx_id"] = json!("MIT");
        assert_eq!(reason(&state, Some(&policy)), "DEFAULT_BRANCH_PUSH_ALLOWED");
        policy["default_branch_push_oss"] = json!(false);
        assert_eq!(reason(&state, Some(&policy)), "DEFAULT_BRANCH_PROTECTED");
        policy["default_branch_push_oss"] = json!(true);
        state["repo"]["license_spdx_id"] = Value::Null;
        assert_eq!(reason(&state, Some(&policy)), "UNKNOWN_OSS_LICENSE");
        assert_eq!(decide(&state, Intent::Auto, false, Oss::Yes, Some(&policy))["reason"], "DEFAULT_BRANCH_PUSH_ALLOWED");
        assert_eq!(decide(&state, Intent::Auto, false, Oss::No, Some(&policy))["reason"], "NON_OSS_PUBLIC_REPOSITORY");
        state["repo"]["visibility"] = json!("private");
        policy["default_branch_push_private"] = json!(false);
        assert_eq!(reason(&state, Some(&policy)), "DEFAULT_BRANCH_PROTECTED");
        policy["default_branch_push_private"] = json!(true);
        policy["default_branch_push_repositories"] = json!(["https://github.com/example/project"]);
        policy["default_branch_push_excluded_repositories"] = json!(["git@github.com:example/project.git"]);
        assert_eq!(reason(&state, Some(&policy)), "DEFAULT_BRANCH_PUSH_EXCLUDED");
        assert_eq!(decide(&state, Intent::Push, false, Oss::Auto, Some(&policy))["reason"], "PUSH_ALLOWED");
        state["current_branch"] = json!("feature");
        assert_eq!(reason(&state, Some(&policy)), "PUSH_ALLOWED");
        state["upstream"] = json!({"remote": "origin", "merge": "refs/heads/main"});
        assert_eq!(reason(&state, Some(&policy)), "DEFAULT_BRANCH_PUSH_EXCLUDED");
        policy["default_branch_push_excluded_repositories"] = json!(["https://github.com/other/project"]);
        assert_eq!(reason(&state, Some(&policy)), "DEFAULT_BRANCH_PUSH_ALLOWED");
    }

    #[test]
    fn invalid_policy_and_explicit_intent() {
        let mut invalid = vec![json!([]), json!({}), json!({"default_branch_push_repositories": null}),
                               json!({"default_branch_push_repositories": [], "extra": true})];
        for url in [Value::Null, json!("origin"), json!("/tmp/repo"), json!("file:///tmp/repo"), json!("../repo"),
                    json!("C:/repo"), json!("https://github.com/example/*"), json!("https://github.com/example/pro?ject"),
                    json!("https://[broken"), json!("https://github.com"), json!("git@github.com:example/[repo]"),
                    json!("https://github.com/example/repo#fragment")] {
            invalid.push(json!({"default_branch_push_repositories": [url]}));
        }
        for (key, value) in [("default_branch_push_private", json!("true")), ("default_branch_push_oss", json!(1)),
                             ("default_branch_push_oss", Value::Null),
                             ("default_branch_push_excluded_repositories", json!("origin")),
                             ("default_branch_push_excluded_repositories", json!(["*"]))] {
            let mut policy = json!({"default_branch_push_repositories": []});
            policy[key] = value;
            invalid.push(policy);
        }
        let state = base();
        for policy in &invalid {
            assert_eq!(reason(&state, Some(policy)), "INVALID_PUSH_POLICY", "{policy}");
            assert_eq!(decide(&state, Intent::Hold, false, Oss::Auto, Some(policy))["reason"], "USER_HOLD");
            assert_eq!(decide(&state, Intent::Auto, true, Oss::Auto, Some(policy))["reason"], "TEMPORARY_COMMIT");
            assert_eq!(decide(&state, Intent::Push, false, Oss::Auto, Some(policy))["reason"], "PUSH_ALLOWED");
        }
    }

    #[test]
    fn policy_file_loading() {
        let mut state = base();
        state["current_branch"] = json!("main");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.json");
        let decided = |path: &Path| reason(&state, Some(&load_policy(path)));
        assert_eq!(decided(&path), "INVALID_PUSH_POLICY", "a missing file is invalid");
        for contents in [&b"{"[..], b"null", b"\xff", br#"{"default_branch_push_repositories": [false]}"#] {
            std::fs::write(&path, contents).unwrap();
            assert_eq!(decided(&path), "INVALID_PUSH_POLICY");
        }
        std::fs::write(&path, r#"{"default_branch_push_repositories": []}"#).unwrap();
        assert_eq!(decided(&path), "DEFAULT_BRANCH_PROTECTED");
        std::fs::write(&path, r#"{"default_branch_push_repositories": ["https://github.com/example/project"]}"#).unwrap();
        assert_eq!(decided(&path), "DEFAULT_BRANCH_PUSH_ALLOWED");
    }

    #[test]
    fn distinct_remote_endpoints() {
        for (fetch, push) in [
            ("https://github.com/example/project.git", "ssh://git@github.com:2222/example/project.git"),
            ("file:///srv/Project.git", "file:///srv/project.git"),
            ("file:///srv/project.git", "file://other/srv/project.git"),
            ("file:///srv/project.git", "file:///srv/project"),
            ("https://github.com/a/b.git?route=one", "https://github.com/a/b.git?route=two"),
            ("ssh://one@host/project.git", "ssh://two@host/project.git"),
        ] {
            let mut state = base();
            state["remotes"]["origin"]["fetch_urls"] = json!([fetch]);
            state["remotes"]["origin"]["push_urls"] = json!([push]);
            assert_eq!(decide(&state, Intent::Push, false, Oss::Auto, None)["decision"], "ask", "{push}");
        }
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    }

    fn private_main(_: &str) -> Option<Value> {
        Some(json!({"private": true, "default_branch": "main", "license": null}))
    }

    #[test]
    fn real_git_remote_rewrites() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        git(d, &["init", "-q"]);
        git(d, &["symbolic-ref", "HEAD", "refs/heads/feature"]);
        git(d, &["remote", "add", "origin", "https://github.com/example/project.git"]);
        let state = collect(d, true, &private_main).unwrap();
        assert_eq!(decide(&state, Intent::Auto, false, Oss::Auto, None)["decision"], "push", "{state}");
        git(d, &["config", "url.https://github.com/other/.pushInsteadOf", "https://github.com/example/"]);
        let state = collect(d, true, &private_main).unwrap();
        let result = decide(&state, Intent::Auto, false, Oss::Auto, None);
        assert_eq!(result["decision"], "ask", "{result}");
    }

    #[test]
    fn collected_settings_match_git_config_lookups() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        git(d, &["init", "-q"]);
        git(d, &["symbolic-ref", "HEAD", "refs/heads/Topic.v2"]);
        let included = d.join("included.config");
        std::fs::write(&included, "[remote \"origin\"]\n\tpushurl = https://example.invalid/included.git\n").unwrap();
        let config = d.join(git(d, &["rev-parse", "--git-path", "config"]).trim());
        let mut text = std::fs::read_to_string(&config).unwrap();
        text.push_str(&format!(
            "[remote \"origin\"]\n\turl = https://example.invalid/a.git\n\turl = https://example.invalid/b.git\n\
             \tMirror\n\tpush = +refs/heads/*:refs/heads/*\n\
             [remote \"Origin\"]\n\turl = https://example.invalid/upper.git\n\tmirror = false\n\
             [remote \"empty\"]\n\turl =\n\
             [include]\n\tpath = {}\n\
             [remote]\n\tPushDefault = Origin\n\
             [branch \"Topic.v2\"]\n\tremote = origin\n\tmerge = refs/heads/one\n\tmerge = refs/heads/two\n\tPushRemote = empty\n",
            included.display().to_string().replace('\\', "/")
        ));
        std::fs::write(&config, text).unwrap();
        let state = collect(d, false, &private_main).unwrap();
        let names: Vec<&String> = state["remotes"].as_object().unwrap().keys().collect();
        assert_eq!(names, ["Origin", "empty", "origin"]);
        let get_all = |key: &str| -> Vec<String> {
            let out = std::process::Command::new("git").arg("-C").arg(d).args(["config", "--get-all", key]).output().unwrap();
            splitlines(String::from_utf8(out.stdout).unwrap().trim_end_matches('\n'))
        };
        let get = |key: &str| -> Option<String> {
            let out = std::process::Command::new("git").arg("-C").arg(d).args(["config", "--get", key]).output().unwrap();
            out.status.success().then(|| String::from_utf8(out.stdout).unwrap().trim_end_matches('\n').to_owned())
        };
        for (name, remote) in state["remotes"].as_object().unwrap() {
            assert_eq!(remote["fetch_urls"], json!(get_all(&format!("remote.{name}.url"))), "{name}");
            assert_eq!(remote["push_urls"], json!(get_all(&format!("remote.{name}.pushurl"))), "{name}");
            assert_eq!(remote["push_refspecs"], json!(get_all(&format!("remote.{name}.push"))), "{name}");
            let mirror = get(&format!("remote.{name}.mirror"));
            assert_eq!(remote["mirror"], json!(mirror.as_deref() == Some("true")), "{name}");
            assert_eq!(remote["mirror_invalid"], json!(!matches!(mirror.as_deref(), None | Some("true" | "false"))), "{name}");
        }
        assert_eq!(state["remotes"]["origin"]["push_urls"], json!(["https://example.invalid/included.git"]));
        assert_eq!(state["remotes"]["origin"]["mirror_invalid"], true);
        assert_eq!(state["candidates"], json!({"branch_push_remote": "empty", "remote_push_default": "Origin", "branch_remote": "origin"}));
        assert_eq!(state["upstream"], json!({"remote": "origin", "merge": null, "merge_values": ["refs/heads/one", "refs/heads/two"]}));
    }

    #[test]
    fn github_metadata_with_non_ascii_text_and_invalid_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        git(d, &["init", "-q"]);
        git(d, &["remote", "add", "origin", "https://github.com/example/project.git"]);
        let japanese = |_: &str| Some(json!({"private": true, "default_branch": "main", "description": "日本語の説明", "license": null}));
        let state = collect(d, true, &japanese).unwrap();
        assert!(state.get("collection_error").is_none(), "{state}");
        assert_eq!(state["repo"]["visibility"], "private");
        assert_eq!(state["repo"]["default_branch"], "main");
        assert!(collect(d, true, &|_: &str| Some(json!("not an object"))).is_err(), "metadata that is not an object fails closed");
        let unavailable = collect(d, true, &|_: &str| None).unwrap();
        assert_eq!(unavailable["repo"]["metadata_error"], true);
    }

    #[test]
    fn python_json_escapes_non_ascii_and_sorts_keys() {
        let value = decision("push", "PUSH_ALLOWED", Some("origin"), Some("機能"));
        assert_eq!(
            to_python_json(&value),
            r#"{"decision":"push","destination":"\u6a5f\u80fd","push_argv":["git","push","origin","HEAD:refs/heads/\u6a5f\u80fd"],"reason":"PUSH_ALLOWED","remote":"origin"}"#
        );
    }
}
