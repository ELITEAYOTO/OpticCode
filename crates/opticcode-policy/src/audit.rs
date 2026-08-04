use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::{ActionOrigin, RiskLevel};

pub const AUDIT_SCHEMA_VERSION: u32 = 1;
pub const MAX_AUDIT_EVENTS: usize = 2_048;
pub const MAX_AUDIT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_AUDIT_RECORD_BYTES: u64 = 64 * 1024;
const POLICY_STATE_DIRECTORY: &str = "OpticCode";
const POLICY_DIRECTORY: &str = "policy-v1";

static STORAGE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditEvent {
    pub schema_version: u32,
    pub event_id: String,
    pub timestamp_unix_ms: u64,
    pub request_id: String,
    pub action_id_hash: String,
    pub action_kind: String,
    pub action_hash: String,
    pub rule_id: String,
    pub decision: String,
    pub risk: RiskLevel,
    pub workspace_hash: String,
    pub origin: ActionOrigin,
    pub approval_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaction_hash: Option<String>,
    pub result: String,
    pub duration_us: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuditQuery {
    pub limit: usize,
    pub workspace_hash: Option<String>,
    pub action_kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuditStoreReport {
    pub schema_version: u32,
    pub storage: PathBuf,
    pub events: Vec<AuditEvent>,
    pub ignored_partial_records: usize,
    pub bounded: bool,
}

#[derive(Debug, Clone)]
pub struct AuditStore {
    state_root: PathBuf,
    workspaces_dir: PathBuf,
}

impl AuditStore {
    pub fn open(state_root: impl Into<PathBuf>) -> Result<Self> {
        let state_root = state_root.into();
        ensure_controlled_directory(&state_root)?;
        let policy_root = state_root.join(POLICY_DIRECTORY);
        ensure_controlled_child(&state_root, &policy_root)?;
        let audit_root = policy_root.join("audit");
        ensure_controlled_child(&policy_root, &audit_root)?;
        let workspaces_dir = audit_root.join("workspaces");
        ensure_controlled_child(&audit_root, &workspaces_dir)?;
        Ok(Self {
            state_root,
            workspaces_dir,
        })
    }

    pub fn default_store() -> Result<Self> {
        Self::open(default_policy_state_dir()?)
    }

    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    pub fn record(&self, mut event: AuditEvent) -> Result<String> {
        validate_event(&event)?;
        if event.event_id.is_empty() {
            event.event_id = new_storage_id("audit")?;
        }
        validate_storage_id(&event.event_id)?;
        let mut bytes =
            serde_json::to_vec(&event).context("failed to serialize policy audit event")?;
        bytes.push(b'\n');
        if bytes.len() as u64 > MAX_AUDIT_RECORD_BYTES {
            bail!("policy audit event exceeds the bounded record size");
        }
        let events_dir = self.workspace_events_dir(&event.workspace_hash, true)?;
        let path = events_dir.join(format!("{}.json", event.event_id));
        atomic_create(&events_dir, &path, &bytes)?;
        self.rotate(&events_dir)?;
        Ok(event.event_id)
    }

    pub fn list(&self, query: &AuditQuery) -> Result<AuditStoreReport> {
        let limit = query.limit.clamp(1, MAX_AUDIT_EVENTS);
        let (storage, mut paths) = if let Some(workspace_hash) = query.workspace_hash.as_deref() {
            validate_hash(workspace_hash, "audit workspace digest")?;
            match self.workspace_events_dir(workspace_hash, false) {
                Ok(events) => (events.clone(), controlled_json_files(&events)?),
                Err(error) if is_not_found_chain(&error) => (
                    self.workspaces_dir.join(workspace_hash).join("events"),
                    Vec::new(),
                ),
                Err(error) => return Err(error),
            }
        } else {
            let mut paths = Vec::new();
            for events in controlled_workspace_event_directories(&self.workspaces_dir)? {
                paths.extend(controlled_json_files(&events)?);
            }
            (self.workspaces_dir.clone(), paths)
        };
        paths.sort();
        let mut events = Vec::new();
        let mut ignored_partial_records = 0usize;
        for path in paths.into_iter().rev() {
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata)
                    if metadata.is_file()
                        && !metadata_is_link_or_reparse(&metadata)
                        && metadata.len() <= MAX_AUDIT_RECORD_BYTES =>
                {
                    metadata
                }
                _ => {
                    ignored_partial_records += 1;
                    continue;
                }
            };
            let mut bytes = Vec::with_capacity(metadata.len() as usize);
            if File::open(&path)
                .and_then(|mut file| file.read_to_end(&mut bytes))
                .is_err()
            {
                ignored_partial_records += 1;
                continue;
            }
            let event = match serde_json::from_slice::<AuditEvent>(&bytes) {
                Ok(event) if validate_event(&event).is_ok() => event,
                _ => {
                    ignored_partial_records += 1;
                    continue;
                }
            };
            if query
                .workspace_hash
                .as_ref()
                .is_some_and(|expected| expected != &event.workspace_hash)
                || query
                    .action_kind
                    .as_ref()
                    .is_some_and(|expected| expected != &event.action_kind)
            {
                continue;
            }
            events.push(event);
            if events.len() == limit {
                break;
            }
        }
        events.sort_by(|left, right| {
            left.timestamp_unix_ms
                .cmp(&right.timestamp_unix_ms)
                .then_with(|| left.event_id.cmp(&right.event_id))
        });
        Ok(AuditStoreReport {
            schema_version: AUDIT_SCHEMA_VERSION,
            storage,
            events,
            ignored_partial_records,
            bounded: true,
        })
    }

    pub fn list_scoped(
        &self,
        authority_workspace_hash: &str,
        query: &AuditQuery,
    ) -> Result<AuditStoreReport> {
        validate_hash(authority_workspace_hash, "audit authority workspace digest")?;
        if query.workspace_hash.as_deref() != Some(authority_workspace_hash) {
            bail!("audit query is not scoped to the authorized workspace");
        }
        self.list(query)
    }

    fn workspace_events_dir(&self, workspace_hash: &str, create: bool) -> Result<PathBuf> {
        validate_hash(workspace_hash, "audit workspace digest")?;
        let workspace = self.workspaces_dir.join(workspace_hash);
        let events = workspace.join("events");
        if create {
            ensure_controlled_child(&self.workspaces_dir, &workspace)?;
            ensure_controlled_child(&workspace, &events)?;
            return Ok(events);
        }
        let canonical_workspace = fs::canonicalize(&workspace).with_context(|| {
            format!("audit workspace namespace is unavailable: {workspace_hash}")
        })?;
        if canonical_workspace.parent() != Some(fs::canonicalize(&self.workspaces_dir)?.as_path()) {
            bail!("audit workspace namespace escaped its controlled parent");
        }
        let metadata = fs::symlink_metadata(&canonical_workspace)?;
        if !metadata.is_dir() || metadata_is_link_or_reparse(&metadata) {
            bail!("audit workspace namespace is unsafe");
        }
        let canonical_events = fs::canonicalize(&events)
            .with_context(|| format!("audit event namespace is unavailable: {workspace_hash}"))?;
        if canonical_events.parent() != Some(canonical_workspace.as_path()) {
            bail!("audit event namespace escaped its workspace");
        }
        Ok(canonical_events)
    }

    fn rotate(&self, events_dir: &Path) -> Result<()> {
        rotate_to_limits(events_dir, MAX_AUDIT_EVENTS, MAX_AUDIT_BYTES)
    }
}

fn rotate_to_limits(events_dir: &Path, max_events: usize, max_bytes: u64) -> Result<()> {
    let mut entries = controlled_json_files(events_dir)?
        .into_iter()
        .filter_map(|path| {
            fs::symlink_metadata(&path)
                .ok()
                .filter(|metadata| metadata.is_file() && !metadata_is_link_or_reparse(metadata))
                .map(|metadata| (path, metadata.len()))
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let mut total_bytes = entries
        .iter()
        .fold(0u64, |sum, (_, bytes)| sum.saturating_add(*bytes));
    let mut remove_count = entries.len().saturating_sub(max_events);
    for (path, bytes) in &entries {
        if remove_count == 0 && total_bytes <= max_bytes {
            break;
        }
        if let Err(error) = fs::remove_file(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(error).with_context(|| {
                    format!("failed to rotate policy audit record: {}", path.display())
                });
            }
        }
        total_bytes = total_bytes.saturating_sub(*bytes);
        remove_count = remove_count.saturating_sub(1);
    }
    Ok(())
}

pub(crate) fn default_policy_state_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("OPTICCODE_POLICY_STATE_DIR") {
        if path.is_empty() {
            bail!("OPTICCODE_POLICY_STATE_DIR cannot be empty");
        }
        return Ok(PathBuf::from(path));
    }
    #[cfg(windows)]
    {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .context("LOCALAPPDATA is required for OpticCode policy state")?;
        Ok(base.join(POLICY_STATE_DIRECTORY))
    }
    #[cfg(not(windows))]
    {
        if let Some(base) = std::env::var_os("XDG_STATE_HOME") {
            return Ok(PathBuf::from(base).join("opticcode"));
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .context("HOME is required for OpticCode policy state")?;
        Ok(home.join(".local").join("state").join("opticcode"))
    }
}

pub(crate) fn policy_subdirectory(state_root: &Path, name: &str) -> Result<PathBuf> {
    validate_directory_name(name)?;
    ensure_controlled_directory(state_root)?;
    let policy_root = state_root.join(POLICY_DIRECTORY);
    ensure_controlled_child(state_root, &policy_root)?;
    let directory = policy_root.join(name);
    ensure_controlled_child(&policy_root, &directory)?;
    Ok(directory)
}

pub(crate) fn atomic_create(parent: &Path, final_path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = fs::canonicalize(parent)
        .with_context(|| format!("failed to resolve state directory: {}", parent.display()))?;
    let final_parent = final_path
        .parent()
        .context("state record has no parent directory")?;
    let canonical_final_parent = fs::canonicalize(final_parent).with_context(|| {
        format!(
            "failed to resolve state record parent: {}",
            final_parent.display()
        )
    })?;
    if canonical_final_parent != parent {
        bail!("state record parent escaped its controlled directory");
    }
    let temp_id = new_storage_id("tmp")?;
    let temp_path = parent.join(format!(".{temp_id}.tmp"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
        .with_context(|| {
            format!(
                "failed to create temporary state record: {}",
                temp_path.display()
            )
        })?;
    let result = (|| -> Result<()> {
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp_path, final_path)
            .with_context(|| format!("failed to publish state record {}", final_path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

pub(crate) fn new_storage_id(prefix: &str) -> Result<String> {
    validate_directory_name(prefix)?;
    let mut random = [0u8; 12];
    getrandom::fill(&mut random)
        .map_err(|error| anyhow::anyhow!("failed to obtain OS randomness: {error}"))?;
    let counter = STORAGE_COUNTER.fetch_add(1, Ordering::Relaxed);
    Ok(format!(
        "{:020}-{:010}-{:016x}-{}",
        unix_millis(),
        std::process::id(),
        counter,
        hex(&random)
    ))
}

pub(crate) fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

pub(crate) fn validate_storage_id(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 160
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("invalid policy state identifier");
    }
    Ok(())
}

fn validate_event(event: &AuditEvent) -> Result<()> {
    if event.schema_version != AUDIT_SCHEMA_VERSION {
        bail!("unsupported policy audit schema");
    }
    for value in [
        event.request_id.as_str(),
        event.action_id_hash.as_str(),
        event.action_kind.as_str(),
        event.action_hash.as_str(),
        event.rule_id.as_str(),
        event.decision.as_str(),
        event.workspace_hash.as_str(),
        event.approval_state.as_str(),
        event.result.as_str(),
    ] {
        if value.is_empty() || value.len() > 256 || value.contains(['\r', '\n', '\0']) {
            bail!("policy audit event contains an invalid bounded field");
        }
    }
    if !event.event_id.is_empty() {
        validate_storage_id(&event.event_id)?;
    }
    validate_hash(&event.action_id_hash, "audit action ID digest")?;
    validate_hash(&event.action_hash, "audit action digest")?;
    validate_hash(&event.workspace_hash, "audit workspace digest")?;
    if let Some(value) = event.approval_hash.as_deref() {
        validate_hash(value, "audit approval digest")?;
    }
    if let Some(value) = event.transaction_hash.as_deref() {
        validate_hash(value, "audit transaction digest")?;
    }
    Ok(())
}

fn controlled_workspace_event_directories(directory: &Path) -> Result<Vec<PathBuf>> {
    let canonical = fs::canonicalize(directory).with_context(|| {
        format!(
            "failed to resolve audit workspace directory: {}",
            directory.display()
        )
    })?;
    let mut paths = Vec::new();
    for entry in fs::read_dir(&canonical)? {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if validate_hash(name, "audit workspace digest").is_err() {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_dir() || metadata_is_link_or_reparse(&metadata) {
            continue;
        }
        let resolved = fs::canonicalize(&path)?;
        if resolved.parent() != Some(canonical.as_path()) {
            continue;
        }
        let events = resolved.join("events");
        let Ok(events_metadata) = fs::symlink_metadata(&events) else {
            continue;
        };
        if !events_metadata.is_dir() || metadata_is_link_or_reparse(&events_metadata) {
            continue;
        }
        let events = fs::canonicalize(events)?;
        if events.parent() == Some(resolved.as_path()) {
            paths.push(events);
        }
    }
    paths.sort();
    Ok(paths)
}

fn controlled_json_files(directory: &Path) -> Result<Vec<PathBuf>> {
    let canonical = fs::canonicalize(directory).with_context(|| {
        format!(
            "failed to resolve policy state directory: {}",
            directory.display()
        )
    })?;
    let mut paths = Vec::new();
    for entry in fs::read_dir(&canonical).with_context(|| {
        format!(
            "failed to enumerate policy state directory: {}",
            canonical.display()
        )
    })? {
        let entry = entry?;
        let path = entry.path();
        if path.parent() != Some(canonical.as_path())
            || path.extension().and_then(|value| value.to_str()) != Some("json")
        {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        if validate_storage_id(stem).is_ok() {
            paths.push(path);
        }
    }
    Ok(paths)
}

fn ensure_controlled_directory(path: &Path) -> Result<()> {
    if path.exists() {
        let metadata = fs::symlink_metadata(path)
            .with_context(|| format!("failed to inspect state directory: {}", path.display()))?;
        if !metadata.is_dir() || metadata_is_link_or_reparse(&metadata) {
            bail!("policy state root must be a real directory");
        }
        return Ok(());
    }
    let parent = path
        .parent()
        .context("policy state root has no parent directory")?;
    if !parent.exists() {
        fs::create_dir_all(parent).with_context(|| {
            format!("failed to prepare state root parent: {}", parent.display())
        })?;
    }
    fs::create_dir(path)
        .with_context(|| format!("failed to create policy state root: {}", path.display()))?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata_is_link_or_reparse(&metadata) {
        bail!("created policy state root is unsafe");
    }
    Ok(())
}

fn ensure_controlled_child(parent: &Path, child: &Path) -> Result<()> {
    let parent = fs::canonicalize(parent)
        .with_context(|| format!("failed to resolve controlled parent: {}", parent.display()))?;
    match fs::create_dir(child) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(error).with_context(|| format!("failed to create {}", child.display()))
        }
    }
    let metadata = fs::symlink_metadata(child)
        .with_context(|| format!("failed to inspect controlled child: {}", child.display()))?;
    if !metadata.is_dir() || metadata_is_link_or_reparse(&metadata) {
        bail!("policy state directory is a symlink or reparse point");
    }
    let canonical = fs::canonicalize(child)?;
    if canonical.parent() != Some(parent.as_path()) {
        bail!("policy state directory escaped its controlled parent");
    }
    Ok(())
}

fn validate_directory_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("invalid policy state directory name");
    }
    Ok(())
}

fn validate_hash(value: &str, label: &str) -> Result<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("{label} must be a 64-character hexadecimal digest");
    }
    Ok(())
}

fn is_not_found_chain(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
    })
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

#[cfg(windows)]
fn metadata_is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(test)]
mod tests {
    use super::{rotate_to_limits, AuditEvent, AuditQuery, AuditStore, AUDIT_SCHEMA_VERSION};
    use crate::model::{ActionOrigin, RiskLevel};

    const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn rotation_keeps_the_newest_bounded_records() {
        let state = tempfile::tempdir().unwrap();
        let store = AuditStore::open(state.path()).unwrap();
        for index in 0..4 {
            store.record(event(index)).unwrap();
        }
        let query = AuditQuery {
            limit: 10,
            workspace_hash: Some(HASH.to_string()),
            action_kind: None,
        };
        let before = store.list_scoped(HASH, &query).unwrap();
        rotate_to_limits(&before.storage, 2, u64::MAX).unwrap();
        let after = store.list_scoped(HASH, &query).unwrap();
        let ids = after
            .events
            .iter()
            .map(|entry| entry.event_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["event-2", "event-3"]);
    }

    fn event(index: u64) -> AuditEvent {
        AuditEvent {
            schema_version: AUDIT_SCHEMA_VERSION,
            event_id: format!("event-{index}"),
            timestamp_unix_ms: index,
            request_id: "request-audit".to_string(),
            action_id_hash: HASH.to_string(),
            action_kind: "read_file".to_string(),
            action_hash: HASH.to_string(),
            rule_id: "read.safe_workspace_file".to_string(),
            decision: "allow".to_string(),
            risk: RiskLevel::Low,
            workspace_hash: HASH.to_string(),
            origin: ActionOrigin::Cli,
            approval_state: "none".to_string(),
            approval_hash: None,
            transaction_hash: None,
            result: "authorized".to_string(),
            duration_us: 1,
        }
    }
}
