use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    unix_millis, validate_edit_plan_against_binding, validate_edit_plan_against_intent,
    ChatEditApplyReport, ChatEditRollbackReport, ChatEditVerificationReport, EditPlan,
    PolicyDecisionRecord, ProposalFileSnapshot, ProposalIntentBinding, ProposalState,
    ProposalTransition, ValidatedEditIntent, ValidatedEditPlan, VerifiedDiff,
    DEFAULT_TRANSACTION_REPORT_TTL_SECONDS, PROPOSAL_STORE_SCHEMA_VERSION,
};

const STORE_DIRECTORY: &str = "proposals-v1";
const RECORDS_DIRECTORY: &str = "records";
const WORKSPACE_LOCK_FILE: &str = ".store.lock";
const MAX_RECORD_BYTES: u64 = 12 * 1024 * 1024;
const MAX_RECORD_GENERATIONS: usize = 4_096;
const STORE_LOCK_WAIT: Duration = Duration::from_secs(5);
const STORE_LOCK_RETRY: Duration = Duration::from_millis(10);
static STORE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalRecord {
    pub schema_version: u32,
    pub proposal_id: String,
    pub workspace_id: String,
    pub workspace_root_hash: String,
    pub sequence: u32,
    pub state: ProposalState,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    pub transaction_report_expires_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<ProposalIntentBinding>,
    pub plan: EditPlan,
    pub files: Vec<ProposalFileSnapshot>,
    pub transitions: Vec<ProposalTransition>,
    pub policy: Vec<PolicyDecisionRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<ChatEditVerificationReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_diff: Option<VerifiedDiff>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apply: Option<ChatEditApplyReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback: Option<ChatEditRollbackReport>,
}

impl ProposalRecord {
    pub fn expired(&self, now_unix_ms: u64) -> bool {
        now_unix_ms > self.expires_at_unix_ms
            && !matches!(
                self.state,
                ProposalState::Applied
                    | ProposalState::RollbackAvailable
                    | ProposalState::RollingBack
                    | ProposalState::RolledBack
            )
    }
}

#[derive(Debug, Clone)]
pub struct ProposalStore {
    root: PathBuf,
    workspace_hash: String,
    workspace_dir: PathBuf,
}

impl ProposalStore {
    pub fn open(state_root: impl Into<PathBuf>, workspace_hash: &str) -> Result<Self> {
        validate_hash(workspace_hash, "workspace hash")?;
        let state_root = state_root.into();
        ensure_real_directory(&state_root)?;
        let root = ensure_child_directory(&state_root, STORE_DIRECTORY)?;
        let workspace_dir = ensure_child_directory(&root, workspace_hash)?;
        Ok(Self {
            root,
            workspace_hash: workspace_hash.to_string(),
            workspace_dir,
        })
    }

    pub fn default_store(workspace_hash: &str) -> Result<Self> {
        Self::open(default_state_root()?, workspace_hash)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn workspace_dir(&self) -> &Path {
        &self.workspace_dir
    }

    pub fn create(&self, validated: ValidatedEditPlan) -> Result<ProposalRecord> {
        self.create_record(validated, None)
    }

    pub fn create_with_intent(
        &self,
        validated: ValidatedEditPlan,
        intent: &ValidatedEditIntent,
    ) -> Result<ProposalRecord> {
        validate_edit_plan_against_intent(&validated, intent).map_err(anyhow::Error::new)?;
        let binding = ProposalIntentBinding::from_validated(intent).map_err(anyhow::Error::new)?;
        binding.validate().map_err(anyhow::Error::new)?;
        self.create_record(validated, Some(binding))
    }

    fn create_record(
        &self,
        validated: ValidatedEditPlan,
        intent: Option<ProposalIntentBinding>,
    ) -> Result<ProposalRecord> {
        let _lock = StoreLock::acquire(&self.workspace_dir)?;
        let now = unix_millis();
        let proposal_id = validated.plan.plan_id.clone();
        validate_identifier(&proposal_id, "proposal ID")?;
        let proposal_dir = self.workspace_dir.join(&proposal_id);
        match fs::create_dir(&proposal_dir) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                bail!("proposal already exists: {proposal_id}")
            }
            Err(error) => return Err(error).context("failed to create proposal directory"),
        }
        let proposal_dir = fs::canonicalize(&proposal_dir)?;
        if proposal_dir.parent() != Some(self.workspace_dir.as_path()) {
            bail!("proposal directory escaped its workspace namespace");
        }
        let records = ensure_child_directory(&proposal_dir, RECORDS_DIRECTORY)?;
        let transitions = vec![
            ProposalTransition {
                sequence: 0,
                state: ProposalState::Generated,
                recorded_at_unix_ms: now,
                reason: "structured edit plan generated".to_string(),
            },
            ProposalTransition {
                sequence: 1,
                state: ProposalState::Validated,
                recorded_at_unix_ms: now,
                reason: "untrusted plan passed strict runtime validation".to_string(),
            },
        ];
        let record = ProposalRecord {
            schema_version: PROPOSAL_STORE_SCHEMA_VERSION,
            proposal_id,
            workspace_id: validated.plan.workspace_id.clone(),
            workspace_root_hash: self.workspace_hash.clone(),
            sequence: 0,
            state: ProposalState::Validated,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
            expires_at_unix_ms: validated.plan.expires_at_unix_ms,
            transaction_report_expires_at_unix_ms: now
                .saturating_add(DEFAULT_TRANSACTION_REPORT_TTL_SECONDS.saturating_mul(1_000)),
            intent,
            plan: validated.plan,
            files: validated.files,
            transitions,
            policy: Vec::new(),
            verification: None,
            verified_diff: None,
            apply: None,
            rollback: None,
        };
        publish_record(&records, &record)?;
        Ok(record)
    }

    pub fn load(&self, proposal_id: &str) -> Result<ProposalRecord> {
        validate_identifier(proposal_id, "proposal ID")?;
        let proposal_dir = recognized_proposal_dir(&self.workspace_dir, proposal_id)?;
        let records = recognized_records_dir(&proposal_dir)?;
        let record = load_latest_valid_record(&records, proposal_id, &self.workspace_hash)?;
        validate_record(&record, proposal_id, &self.workspace_hash)?;
        Ok(record)
    }

    pub fn latest(&self) -> Result<Option<ProposalRecord>> {
        let mut candidates = Vec::new();
        for entry in fs::read_dir(&self.workspace_dir)? {
            let entry = entry?;
            if entry.file_name() == WORKSPACE_LOCK_FILE {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if validate_identifier(&name, "proposal ID").is_err() {
                continue;
            }
            if let Ok(record) = self.load(&name) {
                candidates.push(record);
            }
        }
        candidates.sort_by(|left, right| {
            right
                .updated_at_unix_ms
                .cmp(&left.updated_at_unix_ms)
                .then_with(|| right.proposal_id.cmp(&left.proposal_id))
        });
        Ok(candidates.into_iter().next())
    }

    pub fn find_by_transaction_id(&self, transaction_id: &str) -> Result<Option<ProposalRecord>> {
        validate_identifier(transaction_id, "transaction ID")?;
        let mut matches = Vec::new();
        for entry in fs::read_dir(&self.workspace_dir)? {
            let entry = entry?;
            let Some(proposal_id) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if validate_identifier(&proposal_id, "proposal ID").is_err() {
                continue;
            }
            let Ok(record) = self.load(&proposal_id) else {
                continue;
            };
            if record
                .apply
                .as_ref()
                .is_some_and(|report| report.transaction_id == transaction_id)
            {
                matches.push(record);
            }
        }
        matches.sort_by_key(|record| std::cmp::Reverse(record.updated_at_unix_ms));
        if matches.len() > 1 {
            bail!("transaction ID is associated with multiple proposal records");
        }
        Ok(matches.into_iter().next())
    }

    pub fn transition(
        &self,
        proposal_id: &str,
        next: ProposalState,
        reason: impl Into<String>,
    ) -> Result<ProposalRecord> {
        let reason = reason.into();
        if reason.trim().is_empty() || reason.len() > 4 * 1024 {
            bail!("proposal transition reason is empty or too large");
        }
        self.update(proposal_id, move |record| {
            if !record.state.can_transition_to(next) {
                bail!(
                    "invalid proposal transition: {:?} -> {:?}",
                    record.state,
                    next
                );
            }
            record.state = next;
            record.transitions.push(ProposalTransition {
                sequence: record.transitions.len().min(u32::MAX as usize) as u32,
                state: next,
                recorded_at_unix_ms: unix_millis(),
                reason,
            });
            Ok(())
        })
    }

    pub fn record_policy(
        &self,
        proposal_id: &str,
        decisions: Vec<PolicyDecisionRecord>,
    ) -> Result<ProposalRecord> {
        self.update(proposal_id, move |record| {
            record.policy.extend(decisions);
            if record.policy.len() > 512 {
                bail!("proposal policy audit references exceed their bound");
            }
            Ok(())
        })
    }

    pub fn record_verification(
        &self,
        proposal_id: &str,
        report: ChatEditVerificationReport,
        diff: Option<VerifiedDiff>,
    ) -> Result<ProposalRecord> {
        self.update(proposal_id, move |record| {
            if report.proposal_id != record.proposal_id {
                bail!("verification report belongs to another proposal");
            }
            record.verification = Some(report);
            record.verified_diff = diff;
            Ok(())
        })
    }

    pub fn record_apply(
        &self,
        proposal_id: &str,
        report: ChatEditApplyReport,
    ) -> Result<ProposalRecord> {
        self.update(proposal_id, move |record| {
            if report.proposal_id != record.proposal_id {
                bail!("apply report belongs to another proposal");
            }
            record.apply = Some(report);
            Ok(())
        })
    }

    pub fn record_rollback(
        &self,
        proposal_id: &str,
        report: ChatEditRollbackReport,
    ) -> Result<ProposalRecord> {
        self.update(proposal_id, move |record| {
            if report.proposal_id != record.proposal_id {
                bail!("rollback report belongs to another proposal");
            }
            record.rollback = Some(report);
            Ok(())
        })
    }

    pub fn discard(&self, proposal_id: &str) -> Result<ProposalRecord> {
        self.transition(
            proposal_id,
            ProposalState::Discarded,
            "proposal discarded by explicit client command",
        )
    }

    pub fn expire_if_needed(&self, proposal_id: &str, now: u64) -> Result<ProposalRecord> {
        let record = self.load(proposal_id)?;
        if record.expired(now) && record.state != ProposalState::Expired {
            self.transition(proposal_id, ProposalState::Expired, "proposal TTL elapsed")
        } else {
            Ok(record)
        }
    }

    fn update<F>(&self, proposal_id: &str, update: F) -> Result<ProposalRecord>
    where
        F: FnOnce(&mut ProposalRecord) -> Result<()>,
    {
        validate_identifier(proposal_id, "proposal ID")?;
        let _lock = StoreLock::acquire(&self.workspace_dir)?;
        let proposal_dir = recognized_proposal_dir(&self.workspace_dir, proposal_id)?;
        let records = recognized_records_dir(&proposal_dir)?;
        let mut record = load_latest_valid_record(&records, proposal_id, &self.workspace_hash)?;
        update(&mut record)?;
        record.sequence = record
            .sequence
            .checked_add(1)
            .context("proposal sequence exhausted")?;
        record.updated_at_unix_ms = unix_millis();
        validate_record(&record, proposal_id, &self.workspace_hash)?;
        publish_record(&records, &record)?;
        Ok(record)
    }
}

struct StoreLock {
    _file: File,
}

impl StoreLock {
    fn acquire(workspace_dir: &Path) -> Result<Self> {
        let path = workspace_dir.join(WORKSPACE_LOCK_FILE);
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            if !metadata.is_file() || metadata_is_link_or_reparse(&metadata) {
                bail!("proposal store lock is not a regular file");
            }
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        let deadline = Instant::now() + STORE_LOCK_WAIT;
        loop {
            match file.try_lock() {
                Ok(()) => break,
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    thread::sleep(STORE_LOCK_RETRY);
                }
                Err(TryLockError::WouldBlock) => {
                    bail!("proposal store remained busy for this workspace")
                }
                Err(TryLockError::Error(error)) => {
                    return Err(error).context("failed to acquire proposal store lock")
                }
            }
        }
        Ok(Self { _file: file })
    }
}

fn publish_record(records: &Path, record: &ProposalRecord) -> Result<()> {
    let mut bytes = serde_json::to_vec(record)?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        bail!("proposal record exceeds its bounded storage size");
    }
    let final_path = records.join(format!("record-{:010}.json", record.sequence));
    if final_path.exists() {
        bail!("proposal record generation already exists");
    }
    let temp = records.join(format!(
        ".record-{:010}-{}-{}.tmp",
        record.sequence,
        std::process::id(),
        STORE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    file.write_all(&bytes)?;
    file.flush()?;
    file.sync_all()?;
    drop(file);
    if let Err(error) = fs::rename(&temp, &final_path) {
        let _ = fs::remove_file(&temp);
        return Err(error).context("failed to atomically publish proposal record");
    }
    Ok(())
}

fn load_latest_valid_record(
    records: &Path,
    proposal_id: &str,
    workspace_hash: &str,
) -> Result<ProposalRecord> {
    let mut candidates = fs::read_dir(records)?
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            let sequence = parse_record_sequence(&name)?;
            Some((sequence, entry.path()))
        })
        .collect::<Vec<_>>();
    if candidates.len() > MAX_RECORD_GENERATIONS {
        bail!("proposal record generation limit exceeded");
    }
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.0));
    for (sequence, path) in candidates {
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.is_file()
            || metadata_is_link_or_reparse(&metadata)
            || metadata.len() > MAX_RECORD_BYTES
        {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let Ok(record) = serde_json::from_slice::<ProposalRecord>(&bytes) else {
            continue;
        };
        if record.sequence == sequence
            && validate_record(&record, proposal_id, workspace_hash).is_ok()
        {
            return Ok(record);
        }
    }
    bail!("proposal has no valid published record generation")
}

fn validate_record(record: &ProposalRecord, proposal_id: &str, workspace_hash: &str) -> Result<()> {
    if record.schema_version != PROPOSAL_STORE_SCHEMA_VERSION
        || record.proposal_id != proposal_id
        || record.plan.plan_id != proposal_id
        || record.workspace_root_hash != workspace_hash
        || record.plan.workspace_root_hash != workspace_hash
        || record.workspace_id != record.plan.workspace_id
        || record.created_at_unix_ms > record.updated_at_unix_ms
        || record.expires_at_unix_ms < record.created_at_unix_ms
        || record.transitions.is_empty()
        || record.transitions.last().map(|item| item.state) != Some(record.state)
    {
        bail!("proposal record identity, state, or timestamps are inconsistent");
    }
    if let Some(binding) = &record.intent {
        binding.validate().map_err(anyhow::Error::new)?;
        if binding.request_id != record.plan.request_id
            || binding.workspace_id != record.workspace_id
            || binding.workspace_root_hash != record.workspace_root_hash
            || binding.base_head != record.plan.base_head
            || binding.working_tree_digest != record.plan.working_tree_digest
        {
            bail!("proposal record intent binding does not match its persisted plan");
        }
        let total_snapshot_bytes = record.files.iter().fold(0usize, |total, file| {
            total
                .saturating_add(file.base_content.as_ref().map_or(0, String::len))
                .saturating_add(file.proposed_content.len())
        });
        let persisted_plan = ValidatedEditPlan {
            plan: record.plan.clone(),
            files: record.files.clone(),
            estimated_added_lines: 0,
            estimated_deleted_lines: 0,
            total_snapshot_bytes,
        };
        validate_edit_plan_against_binding(&persisted_plan, binding).map_err(anyhow::Error::new)?;
    }
    for (index, transition) in record.transitions.iter().enumerate() {
        if transition.sequence as usize != index {
            bail!("proposal transition sequence is inconsistent");
        }
        if index > 0
            && !record.transitions[index - 1]
                .state
                .can_transition_to(transition.state)
        {
            bail!("proposal transition history contains an invalid transition");
        }
    }
    Ok(())
}

fn recognized_proposal_dir(workspace_dir: &Path, proposal_id: &str) -> Result<PathBuf> {
    let path = workspace_dir.join(proposal_id);
    let metadata = fs::symlink_metadata(&path).context("proposal does not exist")?;
    if !metadata.is_dir() || metadata_is_link_or_reparse(&metadata) {
        bail!("proposal path is not a regular directory");
    }
    let canonical = fs::canonicalize(&path)?;
    if canonical.parent() != Some(workspace_dir) {
        bail!("proposal path escapes its workspace namespace");
    }
    Ok(canonical)
}

fn recognized_records_dir(proposal_dir: &Path) -> Result<PathBuf> {
    let path = proposal_dir.join(RECORDS_DIRECTORY);
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_dir() || metadata_is_link_or_reparse(&metadata) {
        bail!("proposal records path is not a regular directory");
    }
    let canonical = fs::canonicalize(path)?;
    if canonical.parent() != Some(proposal_dir) {
        bail!("proposal records path escapes its proposal directory");
    }
    Ok(canonical)
}

fn parse_record_sequence(name: &str) -> Option<u32> {
    name.strip_prefix("record-")?
        .strip_suffix(".json")?
        .parse()
        .ok()
}

fn ensure_real_directory(path: &Path) -> Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata_is_link_or_reparse(&metadata) => {}
        Ok(_) => bail!("proposal state root is not a regular directory"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path)?;
        }
        Err(error) => return Err(error.into()),
    }
    let canonical = fs::canonicalize(path)?;
    let metadata = fs::symlink_metadata(&canonical)?;
    if !metadata.is_dir() || metadata_is_link_or_reparse(&metadata) {
        bail!("proposal state root resolves through an unsafe entry");
    }
    Ok(canonical)
}

fn ensure_child_directory(parent: &Path, name: &str) -> Result<PathBuf> {
    validate_identifier(name, "storage directory")?;
    let parent = ensure_real_directory(parent)?;
    let child = parent.join(name);
    match fs::create_dir(&child) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let metadata = fs::symlink_metadata(&child)?;
    if !metadata.is_dir() || metadata_is_link_or_reparse(&metadata) {
        bail!("proposal storage child is not a regular directory");
    }
    let canonical = fs::canonicalize(&child)?;
    if canonical.parent() != Some(parent.as_path()) {
        bail!("proposal storage child escaped its parent");
    }
    Ok(canonical)
}

fn default_state_root() -> Result<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")));
    let base = base.context("local application state directory is unavailable")?;
    Ok(base.join("OpticCode"))
}

fn validate_identifier(value: &str, name: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 160
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
    {
        bail!("{name} is invalid");
    }
    Ok(())
}

fn validate_hash(value: &str, name: &str) -> Result<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("{name} is not a 64-character hexadecimal hash");
    }
    Ok(())
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
    use std::sync::Arc;

    use crate::{
        ByteRange, EditOperation, EditPlanLimits, EditValidationKind, LineEnding, TextEncoding,
    };

    use super::*;

    fn validated(root_hash: &str, proposal_id: &str) -> ValidatedEditPlan {
        let now = unix_millis();
        let base = "class Example { int value = 1; }\n".to_string();
        let proposed = "class Example { int value = 2; }\n".to_string();
        let start = base.find('1').unwrap();
        ValidatedEditPlan {
            plan: EditPlan {
                schema_version: 1,
                plan_id: proposal_id.to_string(),
                request_id: "request-1".to_string(),
                workspace_id: "workspace-1".to_string(),
                workspace_root_hash: root_hash.to_string(),
                profile: "minecraft-java-1.8".to_string(),
                provider: opticcode_llm::ProviderId::Ollama,
                model: "fixture".to_string(),
                base_head: "a".repeat(40),
                working_tree_digest: "b".repeat(64),
                context_used: Vec::new(),
                user_references: Vec::new(),
                summary: "Update a fixture.".to_string(),
                rationale_summary: "Requested behavior.".to_string(),
                operations: vec![EditOperation::Modify {
                    path: "src/Example.java".to_string(),
                    expected_file_hash: crate::content_hash(base.as_bytes()),
                    encoding: TextEncoding::Utf8,
                    line_ending: LineEnding::Lf,
                    range: ByteRange {
                        start,
                        end: start + 1,
                    },
                    expected_old: "1".to_string(),
                    replacement: "2".to_string(),
                    reason: "Requested value.".to_string(),
                    symbol: Some("Example.value".to_string()),
                    provenance: vec!["fixture".to_string()],
                }],
                validations: vec![EditValidationKind::ReparseJava],
                risks: vec!["Fixture behavior changes.".to_string()],
                limitations: vec!["Fixture only.".to_string()],
                limits: EditPlanLimits::default(),
                expires_at_unix_ms: now + 60_000,
            },
            files: vec![ProposalFileSnapshot {
                path: "src/Example.java".to_string(),
                status: crate::ProposalFileStatus::Modified,
                encoding: TextEncoding::Utf8,
                line_ending: LineEnding::Lf,
                base_hash: Some(crate::content_hash(base.as_bytes())),
                base_content: Some(base),
                proposed_hash: crate::content_hash(proposed.as_bytes()),
                proposed_bytes: proposed.len(),
                proposed_content: proposed,
            }],
            estimated_added_lines: 1,
            estimated_deleted_lines: 1,
            total_snapshot_bytes: 70,
        }
    }

    #[test]
    fn store_publishes_and_recovers_last_valid_generation() {
        let temp = tempfile::tempdir().unwrap();
        let root_hash = "a".repeat(64);
        let store = ProposalStore::open(temp.path(), &root_hash).unwrap();
        let created = store.create(validated(&root_hash, "proposal-1")).unwrap();
        assert_eq!(created.state, ProposalState::Validated);
        let verified = store
            .transition(
                "proposal-1",
                ProposalState::WorktreePrepared,
                "fixture worktree",
            )
            .unwrap();
        assert_eq!(verified.sequence, 1);

        let records = store
            .workspace_dir()
            .join("proposal-1")
            .join(RECORDS_DIRECTORY);
        fs::write(records.join("record-0000000002.json"), b"{truncated").unwrap();
        let recovered = store.load("proposal-1").unwrap();
        assert_eq!(recovered.sequence, 1);
        assert_eq!(recovered.state, ProposalState::WorktreePrepared);
    }

    #[test]
    fn store_refuses_workspace_reuse_and_invalid_transitions() {
        let temp = tempfile::tempdir().unwrap();
        let root_hash = "b".repeat(64);
        let store = ProposalStore::open(temp.path(), &root_hash).unwrap();
        store.create(validated(&root_hash, "proposal-2")).unwrap();
        assert!(store
            .transition("proposal-2", ProposalState::Applied, "skip verification")
            .is_err());

        let other = ProposalStore::open(temp.path(), &"c".repeat(64)).unwrap();
        assert!(other.load("proposal-2").is_err());
    }

    #[test]
    fn concurrent_updates_are_serialized_and_unknown_directories_are_preserved() {
        let temp = tempfile::tempdir().unwrap();
        let root_hash = "d".repeat(64);
        let store = Arc::new(ProposalStore::open(temp.path(), &root_hash).unwrap());
        store.create(validated(&root_hash, "proposal-3")).unwrap();
        let unknown = store.workspace_dir().join("do-not-delete-me");
        fs::create_dir(&unknown).unwrap();
        fs::write(unknown.join("owner.txt"), "user data").unwrap();

        let handles = (0..8)
            .map(|index| {
                let store = Arc::clone(&store);
                thread::spawn(move || {
                    store
                        .transition(
                            "proposal-3",
                            ProposalState::Validated,
                            format!("concurrent update {index}"),
                        )
                        .unwrap();
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            handle.join().unwrap();
        }
        let record = store.load("proposal-3").unwrap();
        assert_eq!(record.sequence, 8);
        assert!(unknown.join("owner.txt").exists());
    }

    #[test]
    fn expiry_and_discard_are_state_transitions_not_recursive_deletes() {
        let temp = tempfile::tempdir().unwrap();
        let root_hash = "e".repeat(64);
        let store = ProposalStore::open(temp.path(), &root_hash).unwrap();
        let created = store.create(validated(&root_hash, "proposal-4")).unwrap();
        let expired = store
            .expire_if_needed("proposal-4", created.expires_at_unix_ms + 1)
            .unwrap();
        assert_eq!(expired.state, ProposalState::Expired);
        assert!(store.workspace_dir().join("proposal-4").exists());

        store.create(validated(&root_hash, "proposal-5")).unwrap();
        assert_eq!(
            store.discard("proposal-5").unwrap().state,
            ProposalState::Discarded
        );
    }
}
