use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::audit::{
    atomic_create, default_policy_state_dir, new_storage_id, policy_subdirectory, unix_millis,
    validate_storage_id,
};
use crate::model::PolicyMode;

pub const APPROVAL_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_APPROVAL_TTL_SECONDS: u64 = 10 * 60;
pub const MAX_APPROVAL_TTL_SECONDS: u64 = 30 * 60;
const MAX_APPROVAL_RECORD_BYTES: u64 = 256 * 1024;
const MAX_APPROVAL_FILES: usize = 512;
const MAX_APPROVAL_ACTIONS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct ApprovalFileBinding {
    pub path_hash: String,
    pub expected_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalBinding {
    pub request_id: String,
    pub workspace_id: String,
    pub workspace_root_hash: String,
    pub mode: PolicyMode,
    pub base_head: String,
    pub working_tree_digest: String,
    pub diff_hash: String,
    pub files_hash: String,
    pub files: Vec<ApprovalFileBinding>,
    pub action_hashes: Vec<String>,
    pub transaction_id: String,
}

impl ApprovalBinding {
    pub fn normalized(self) -> Result<Self> {
        validate_binding(&self)?;
        if self.files.iter().collect::<BTreeSet<_>>().len() != self.files.len() {
            bail!("approval file bindings must be unique");
        }
        if self.action_hashes.iter().collect::<BTreeSet<_>>().len() != self.action_hashes.len() {
            bail!("approval action hashes must be unique");
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeConfirmation {
    client: String,
    confirmation_id: String,
}

impl NativeConfirmation {
    /// Construct only after a native, explicit user confirmation surface returned consent.
    pub fn explicit(client: impl Into<String>, confirmation_id: impl Into<String>) -> Result<Self> {
        let value = Self {
            client: client.into(),
            confirmation_id: confirmation_id.into(),
        };
        validate_bounded_identifier(&value.client, 96)?;
        validate_bounded_identifier(&value.confirmation_id, 160)?;
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalGrant {
    pub schema_version: u32,
    pub approval_id: String,
    pub issued_unix_ms: u64,
    pub expires_unix_ms: u64,
    pub binding_hash: String,
    pub state: ApprovalState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    Available,
    Consumed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalError {
    Missing,
    Expired,
    Reused,
    WrongRequest,
    WrongWorkspace,
    WrongMode,
    HeadChanged,
    WorkingTreeChanged,
    DiffChanged,
    FilesChanged,
    ActionsChanged,
    TransactionChanged,
    InvalidRecord,
}

impl std::fmt::Display for ApprovalError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Missing => "approval does not exist",
            Self::Expired => "approval expired",
            Self::Reused => "approval was already consumed",
            Self::WrongRequest => "approval belongs to another request",
            Self::WrongWorkspace => "approval belongs to another workspace",
            Self::WrongMode => "approval belongs to another policy mode",
            Self::HeadChanged => "repository HEAD changed after approval",
            Self::WorkingTreeChanged => "working tree changed after approval",
            Self::DiffChanged => "verified diff changed after approval",
            Self::FilesChanged => "approved file set or hashes changed",
            Self::ActionsChanged => "approved action list changed",
            Self::TransactionChanged => "transaction binding changed",
            Self::InvalidRecord => "approval record is invalid",
        })
    }
}

impl std::error::Error for ApprovalError {}

#[derive(Debug, Clone)]
pub struct ApprovalStore {
    active_dir: PathBuf,
    used_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApprovalRecord {
    schema_version: u32,
    issued_unix_ms: u64,
    expires_unix_ms: u64,
    confirmation_hash: String,
    binding_hash: String,
    binding: ApprovalBinding,
}

impl ApprovalStore {
    pub fn open(state_root: impl Into<PathBuf>) -> Result<Self> {
        let state_root = state_root.into();
        let approval_root = policy_subdirectory(&state_root, "approvals")?;
        let active_dir = approval_root.join("active");
        ensure_child(&approval_root, &active_dir)?;
        let used_dir = approval_root.join("used");
        ensure_child(&approval_root, &used_dir)?;
        Ok(Self {
            active_dir,
            used_dir,
        })
    }

    pub fn default_store() -> Result<Self> {
        Self::open(default_policy_state_dir()?)
    }

    pub fn issue(
        &self,
        binding: ApprovalBinding,
        confirmation: &NativeConfirmation,
        ttl_seconds: u64,
    ) -> Result<ApprovalGrant> {
        self.issue_at(binding, confirmation, ttl_seconds, unix_millis())
    }

    pub fn consume(
        &self,
        approval_id: &str,
        observed: &ApprovalBinding,
    ) -> std::result::Result<ApprovalGrant, ApprovalError> {
        self.consume_at(approval_id, observed, unix_millis())
    }

    fn issue_at(
        &self,
        binding: ApprovalBinding,
        confirmation: &NativeConfirmation,
        ttl_seconds: u64,
        now_ms: u64,
    ) -> Result<ApprovalGrant> {
        if ttl_seconds == 0 || ttl_seconds > MAX_APPROVAL_TTL_SECONDS {
            bail!("approval TTL must be between 1 and {MAX_APPROVAL_TTL_SECONDS} seconds");
        }
        let binding = binding.normalized()?;
        let binding_hash = hash_json(&binding)?;
        let approval_id = format!("approval-{}", new_storage_id("grant")?);
        validate_storage_id(&approval_id)?;
        let expires_unix_ms = now_ms.saturating_add(ttl_seconds.saturating_mul(1_000));
        let confirmation_hash = blake3::hash(
            format!("{}:{}", confirmation.client, confirmation.confirmation_id).as_bytes(),
        )
        .to_hex()
        .to_string();
        let record = ApprovalRecord {
            schema_version: APPROVAL_SCHEMA_VERSION,
            issued_unix_ms: now_ms,
            expires_unix_ms,
            confirmation_hash,
            binding_hash: binding_hash.clone(),
            binding,
        };
        let mut bytes =
            serde_json::to_vec(&record).context("failed to serialize approval record")?;
        bytes.push(b'\n');
        if bytes.len() as u64 > MAX_APPROVAL_RECORD_BYTES {
            bail!("approval record exceeds its bounded size");
        }
        atomic_create(
            &self.active_dir,
            &self.active_dir.join(format!("{approval_id}.json")),
            &bytes,
        )?;
        Ok(ApprovalGrant {
            schema_version: APPROVAL_SCHEMA_VERSION,
            approval_id,
            issued_unix_ms: now_ms,
            expires_unix_ms,
            binding_hash,
            state: ApprovalState::Available,
        })
    }

    fn consume_at(
        &self,
        approval_id: &str,
        observed: &ApprovalBinding,
        now_ms: u64,
    ) -> std::result::Result<ApprovalGrant, ApprovalError> {
        validate_storage_id(approval_id).map_err(|_| ApprovalError::InvalidRecord)?;
        let active = self.active_dir.join(format!("{approval_id}.json"));
        let used = self.used_dir.join(format!("{approval_id}.json"));
        let consuming = self.used_dir.join(format!("{approval_id}-consuming.json"));
        let claim = self.used_dir.join(format!("{approval_id}.claim"));
        if claim.exists() || used.exists() || consuming.exists() {
            return Err(ApprovalError::Reused);
        }
        if !active.exists() {
            return Err(ApprovalError::Missing);
        }
        match OpenOptions::new().write(true).create_new(true).open(&claim) {
            Ok(file) => {
                if file.sync_all().is_err() {
                    return Err(ApprovalError::InvalidRecord);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(ApprovalError::Reused);
            }
            Err(_) => return Err(ApprovalError::InvalidRecord),
        }
        match fs::rename(&active, &consuming) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(ApprovalError::Reused);
            }
            Err(_) => return Err(ApprovalError::InvalidRecord),
        }

        let outcome = read_record(&consuming).and_then(|record| {
            let normalized = observed
                .clone()
                .normalized()
                .map_err(|_| ApprovalError::InvalidRecord)?;
            let binding_hash =
                hash_json(&record.binding).map_err(|_| ApprovalError::InvalidRecord)?;
            if record.schema_version != APPROVAL_SCHEMA_VERSION
                || record.binding_hash != binding_hash
                || record.expires_unix_ms < record.issued_unix_ms
                || record.confirmation_hash.len() != 64
            {
                return Err(ApprovalError::InvalidRecord);
            }
            if now_ms > record.expires_unix_ms {
                return Err(ApprovalError::Expired);
            }
            compare_binding(&record.binding, &normalized)?;
            Ok(ApprovalGrant {
                schema_version: APPROVAL_SCHEMA_VERSION,
                approval_id: approval_id.to_string(),
                issued_unix_ms: record.issued_unix_ms,
                expires_unix_ms: record.expires_unix_ms,
                binding_hash: record.binding_hash,
                state: ApprovalState::Consumed,
            })
        });

        // A failed comparison also consumes the grant: drift must never turn into a retry oracle.
        if fs::rename(&consuming, &used).is_err() {
            return Err(ApprovalError::InvalidRecord);
        }
        outcome
    }
}

fn compare_binding(
    expected: &ApprovalBinding,
    observed: &ApprovalBinding,
) -> std::result::Result<(), ApprovalError> {
    if expected.request_id != observed.request_id {
        return Err(ApprovalError::WrongRequest);
    }
    if expected.workspace_id != observed.workspace_id
        || expected.workspace_root_hash != observed.workspace_root_hash
    {
        return Err(ApprovalError::WrongWorkspace);
    }
    if expected.mode != observed.mode {
        return Err(ApprovalError::WrongMode);
    }
    if expected.base_head != observed.base_head {
        return Err(ApprovalError::HeadChanged);
    }
    if expected.working_tree_digest != observed.working_tree_digest {
        return Err(ApprovalError::WorkingTreeChanged);
    }
    if expected.diff_hash != observed.diff_hash {
        return Err(ApprovalError::DiffChanged);
    }
    if expected.files_hash != observed.files_hash || expected.files != observed.files {
        return Err(ApprovalError::FilesChanged);
    }
    if expected.transaction_id != observed.transaction_id {
        return Err(ApprovalError::TransactionChanged);
    }
    if expected.action_hashes != observed.action_hashes {
        return Err(ApprovalError::ActionsChanged);
    }
    Ok(())
}

fn validate_binding(binding: &ApprovalBinding) -> Result<()> {
    validate_bounded_identifier(&binding.request_id, 160)?;
    validate_bounded_identifier(&binding.workspace_id, 160)?;
    validate_hash(&binding.workspace_root_hash)?;
    validate_hash(&binding.working_tree_digest)?;
    validate_hash(&binding.diff_hash)?;
    validate_hash(&binding.files_hash)?;
    validate_bounded_identifier(&binding.base_head, 160)?;
    validate_bounded_identifier(&binding.transaction_id, 160)?;
    if binding.files.is_empty() || binding.files.len() > MAX_APPROVAL_FILES {
        bail!("approval must bind a bounded non-empty file list");
    }
    if binding.action_hashes.is_empty() || binding.action_hashes.len() > MAX_APPROVAL_ACTIONS {
        bail!("approval must bind a bounded non-empty action list");
    }
    for file in &binding.files {
        validate_hash(&file.path_hash)?;
        validate_hash(&file.expected_hash)?;
    }
    for action_hash in &binding.action_hashes {
        validate_hash(action_hash)?;
    }
    Ok(())
}

fn validate_hash(value: &str) -> Result<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("approval binding contains an invalid hash");
    }
    Ok(())
}

fn validate_bounded_identifier(value: &str, max: usize) -> Result<()> {
    if value.is_empty() || value.len() > max || value.contains(['\r', '\n', '\0']) {
        bail!("approval binding contains an invalid identifier");
    }
    Ok(())
}

fn read_record(path: &Path) -> std::result::Result<ApprovalRecord, ApprovalError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ApprovalError::InvalidRecord)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_APPROVAL_RECORD_BYTES
    {
        return Err(ApprovalError::InvalidRecord);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(ApprovalError::InvalidRecord);
        }
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(|_| ApprovalError::InvalidRecord)?;
    serde_json::from_slice(&bytes).map_err(|_| ApprovalError::InvalidRecord)
}

fn hash_json<T: Serialize>(value: &T) -> Result<String> {
    Ok(blake3::hash(&serde_json::to_vec(value)?)
        .to_hex()
        .to_string())
}

fn ensure_child(parent: &Path, child: &Path) -> Result<()> {
    let parent = fs::canonicalize(parent)?;
    match fs::create_dir(child) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let metadata = fs::symlink_metadata(child)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        bail!("approval state directory is unsafe");
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            bail!("approval state directory is a reparse point");
        }
    }
    if fs::canonicalize(child)?.parent() != Some(parent.as_path()) {
        bail!("approval state directory escaped its parent");
    }
    Ok(())
}
