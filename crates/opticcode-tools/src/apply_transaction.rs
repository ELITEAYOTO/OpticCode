#[cfg(windows)]
mod windows;

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::git_state::{capture_git_state, GitChange, GitStateSnapshot};

pub const APPLY_TRANSACTION_SCHEMA_VERSION: u32 = 1;

const MANIFEST_FILE: &str = "manifest.json";
const PATCH_FILE: &str = "patch.diff";
const EVENTS_DIR: &str = "events";
const BACKUPS_DIR: &str = "backups";
const WORKSPACE_LOCK_FILE: &str = "apply.lock";

static UNIQUE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyTransactionState {
    Prepared,
    Applying,
    Applied,
    Finalizing,
    Committed,
    RollbackStarted,
    RolledBack,
    RollbackFailed,
}

impl ApplyTransactionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Applying => "applying",
            Self::Applied => "applied",
            Self::Finalizing => "finalizing",
            Self::Committed => "committed",
            Self::RollbackStarted => "rollback_started",
            Self::RolledBack => "rolled_back",
            Self::RollbackFailed => "rollback_failed",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Committed | Self::RolledBack)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyGitPolicy {
    RequireClean,
    AllowDirty,
    Optional,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyTransactionErrorKind {
    Precondition,
    Collision,
    InvalidTransaction,
    Io,
}

#[derive(Debug)]
pub struct ApplyTransactionError {
    pub kind: ApplyTransactionErrorKind,
    message: String,
}

impl ApplyTransactionError {
    fn new(kind: ApplyTransactionErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for ApplyTransactionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ApplyTransactionError {}

pub fn apply_transaction_error_kind(error: &anyhow::Error) -> Option<ApplyTransactionErrorKind> {
    error
        .downcast_ref::<ApplyTransactionError>()
        .map(|error| error.kind)
}

#[derive(Debug, Clone)]
pub struct FileMutation {
    pub path: PathBuf,
    pub expected_before: Option<Vec<u8>>,
    pub desired_after: Option<Vec<u8>>,
}

impl FileMutation {
    pub fn replace(
        path: impl Into<PathBuf>,
        expected_before: impl Into<Vec<u8>>,
        desired_after: impl Into<Vec<u8>>,
    ) -> Self {
        Self {
            path: path.into(),
            expected_before: Some(expected_before.into()),
            desired_after: Some(desired_after.into()),
        }
    }

    pub fn create(path: impl Into<PathBuf>, desired_after: impl Into<Vec<u8>>) -> Self {
        Self {
            path: path.into(),
            expected_before: None,
            desired_after: Some(desired_after.into()),
        }
    }

    pub fn delete(path: impl Into<PathBuf>, expected_before: impl Into<Vec<u8>>) -> Self {
        Self {
            path: path.into(),
            expected_before: Some(expected_before.into()),
            desired_after: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ApplyTransactionRequest {
    pub workspace: PathBuf,
    pub patch: Vec<u8>,
    pub mutations: Vec<FileMutation>,
    pub git_policy: ApplyGitPolicy,
    pub copied_from: Option<PathBuf>,
    requested_id: Option<String>,
}

impl ApplyTransactionRequest {
    pub fn new(
        workspace: impl Into<PathBuf>,
        patch: impl Into<Vec<u8>>,
        mutations: Vec<FileMutation>,
    ) -> Self {
        Self {
            workspace: workspace.into(),
            patch: patch.into(),
            mutations,
            git_policy: ApplyGitPolicy::RequireClean,
            copied_from: None,
            requested_id: None,
        }
    }

    pub fn with_git_policy(mut self, git_policy: ApplyGitPolicy) -> Self {
        self.git_policy = git_policy;
        self
    }

    pub fn with_copied_from(mut self, copied_from: impl Into<PathBuf>) -> Self {
        self.copied_from = Some(copied_from.into());
        self
    }

    pub fn with_transaction_id(mut self, transaction_id: impl Into<String>) -> Self {
        self.requested_id = Some(transaction_id.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyTransactionValidation {
    pub git_policy: ApplyGitPolicy,
    pub git_captured: bool,
    pub git_clean: bool,
    pub pre_existing_changes: usize,
    pub expected_contents_verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyTransactionFile {
    pub path: String,
    pub existed_before: bool,
    pub before_hash: Option<String>,
    pub before_bytes: u64,
    pub after_hash: Option<String>,
    pub after_bytes: u64,
    pub backup_path: Option<String>,
    pub readonly: Option<bool>,
    pub unix_mode: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyTransactionManifest {
    pub schema_version: u32,
    pub transaction_id: String,
    pub workspace: String,
    pub copied_from: Option<String>,
    pub created_at_unix_ms: u64,
    pub patch_hash: String,
    pub patch_path: String,
    pub files: Vec<ApplyTransactionFile>,
    pub validation: ApplyTransactionValidation,
    pub git_before: Option<GitStateSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyTransactionEvent {
    pub schema_version: u32,
    pub transaction_id: String,
    pub sequence: u32,
    pub recorded_at_unix_ms: u64,
    pub state: ApplyTransactionState,
    pub message: String,
    pub files: Vec<String>,
    pub errors: Vec<String>,
    pub elapsed_ms: u64,
    pub git_snapshot: Option<GitStateSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyTransactionResult {
    pub schema_version: u32,
    pub transaction_id: String,
    pub workspace: String,
    pub operation_success: bool,
    pub final_state: ApplyTransactionState,
    pub rollback_attempted: bool,
    pub rollback_success: Option<bool>,
    pub planned_files: Vec<String>,
    pub modified_files: Vec<String>,
    pub restored_files: Vec<String>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub duration_ms: u64,
    pub git_restored: Option<bool>,
    pub transaction_dir: String,
}

impl ApplyTransactionResult {
    pub fn committed(&self) -> bool {
        self.operation_success && self.final_state == ApplyTransactionState::Committed
    }

    pub fn rolled_back(&self) -> bool {
        self.final_state == ApplyTransactionState::RolledBack && self.rollback_success == Some(true)
    }

    pub fn rollback_failed(&self) -> bool {
        self.final_state == ApplyTransactionState::RollbackFailed
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyTransactionInspection {
    pub schema_version: u32,
    pub transaction_id: String,
    pub valid: bool,
    pub legacy: bool,
    pub manifest: Option<ApplyTransactionManifest>,
    pub events: Vec<ApplyTransactionEvent>,
    pub final_state: Option<ApplyTransactionState>,
    pub recoverable: bool,
    pub recovery_reasons: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApplyFaultPoint {
    AfterFirstBackup,
    AfterPrepared,
    BeforeFirstTargetWrite,
    AfterTargetStaged,
    AfterFirstTargetWrite,
    AfterAllTargetWrites,
    BeforeFinalization,
    DuringFinalization,
    RollbackStarted,
    AfterFirstRestore,
}

trait ApplyFaultInjector {
    fn check(&mut self, point: ApplyFaultPoint) -> Result<()>;
}

struct NoFaultInjector;

impl ApplyFaultInjector for NoFaultInjector {
    fn check(&mut self, _point: ApplyFaultPoint) -> Result<()> {
        Ok(())
    }
}

pub fn execute_apply_transaction(
    request: ApplyTransactionRequest,
) -> Result<ApplyTransactionResult> {
    execute_apply_transaction_with_faults(request, &mut NoFaultInjector)
}

pub fn rollback_apply_transaction(
    workspace: &Path,
    transaction_id: &str,
) -> Result<ApplyTransactionResult> {
    rollback_apply_transaction_with_faults(
        workspace,
        transaction_id,
        "explicit rollback requested",
        &mut NoFaultInjector,
    )
}

pub fn recover_apply_transaction(
    workspace: &Path,
    transaction_id: &str,
) -> Result<ApplyTransactionResult> {
    rollback_apply_transaction_with_faults(
        workspace,
        transaction_id,
        "recovery rollback requested",
        &mut NoFaultInjector,
    )
}

pub fn append_apply_log_index(workspace: &Path, json: &[u8]) -> Result<()> {
    let workspace = fs::canonicalize(workspace).map_err(|error| {
        transaction_error(
            ApplyTransactionErrorKind::Io,
            format!("failed to resolve apply-log workspace: {error}"),
        )
    })?;
    let _workspace_lock = WorkspaceTransactionLock::acquire(&workspace)?;
    let log_path = workspace.join(".opticcode").join("apply-log.jsonl");
    match fs::symlink_metadata(&log_path) {
        Ok(metadata) if !metadata_is_link_or_reparse(&metadata) && metadata.is_file() => {}
        Ok(_) => {
            return Err(transaction_error(
                ApplyTransactionErrorKind::Precondition,
                format!(
                    "apply-log index is a symlink, reparse point, or non-file: {}",
                    log_path.display()
                ),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect apply-log: {}", log_path.display()));
        }
    }

    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("failed to open apply-log: {}", log_path.display()))?;
    log.write_all(json)?;
    log.write_all(b"\n")?;
    log.flush()?;
    log.sync_all()?;
    sync_parent_directory(&log_path)?;
    Ok(())
}

fn execute_apply_transaction_with_faults(
    request: ApplyTransactionRequest,
    faults: &mut dyn ApplyFaultInjector,
) -> Result<ApplyTransactionResult> {
    let mut context = prepare_transaction(request, faults)?;
    let mut modified_files = Vec::new();

    let apply_result = (|| -> Result<()> {
        faults.check(ApplyFaultPoint::AfterPrepared)?;
        context.journal.transition(
            ApplyTransactionState::Applying,
            "target file application started",
            Vec::new(),
            Vec::new(),
            None,
        )?;

        for (index, mutation) in context.mutations.iter().enumerate() {
            if index == 0 {
                faults.check(ApplyFaultPoint::BeforeFirstTargetWrite)?;
            }
            apply_mutation(
                &context.workspace,
                mutation,
                &context.manifest.files[index],
                faults,
            )?;
            modified_files.push(path_to_portable_string(&mutation.path)?);
            if index == 0 {
                faults.check(ApplyFaultPoint::AfterFirstTargetWrite)?;
            }
        }

        faults.check(ApplyFaultPoint::AfterAllTargetWrites)?;
        verify_mutations_applied(&context.workspace, &context.mutations)?;
        context.journal.transition(
            ApplyTransactionState::Applied,
            "all planned target files were applied and verified",
            modified_files.clone(),
            Vec::new(),
            None,
        )?;
        faults.check(ApplyFaultPoint::BeforeFinalization)?;
        context.journal.transition(
            ApplyTransactionState::Finalizing,
            "transaction finalization started",
            modified_files.clone(),
            Vec::new(),
            None,
        )?;
        faults.check(ApplyFaultPoint::DuringFinalization)?;

        let git_after = capture_git_for_manifest(&context.manifest)?;
        verify_git_state_for_commit(&context.manifest, git_after.as_ref())?;
        verify_mutations_applied(&context.workspace, &context.mutations)?;
        context.journal.transition(
            ApplyTransactionState::Committed,
            "transaction committed",
            modified_files.clone(),
            Vec::new(),
            git_after,
        )?;
        Ok(())
    })();

    match apply_result {
        Ok(()) => Ok(context.result(
            true,
            ApplyTransactionState::Committed,
            false,
            None,
            modified_files,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
        )),
        Err(error) => rollback_prepared_context(
            &mut context,
            faults,
            modified_files,
            format!("{error:#}"),
            false,
        ),
    }
}

struct PreparedTransaction {
    workspace: PathBuf,
    run_dir: PathBuf,
    manifest: ApplyTransactionManifest,
    mutations: Vec<FileMutation>,
    journal: TransactionJournal,
    started_at: Instant,
    _workspace_lock: WorkspaceTransactionLock,
}

struct WorkspaceTransactionLock {
    _file: File,
}

impl WorkspaceTransactionLock {
    fn acquire(workspace: &Path) -> Result<Self> {
        let opticcode_dir = workspace.join(".opticcode");
        match fs::symlink_metadata(&opticcode_dir) {
            Ok(metadata) => {
                if metadata_is_link_or_reparse(&metadata) || !metadata.is_dir() {
                    return Err(transaction_error(
                        ApplyTransactionErrorKind::Precondition,
                        format!(
                            "transaction metadata root is not a regular directory: {}",
                            opticcode_dir.display()
                        ),
                    ));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&opticcode_dir).with_context(|| {
                    format!(
                        "failed to create transaction metadata root: {}",
                        opticcode_dir.display()
                    )
                })?;
                sync_directory(workspace)?;
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to inspect transaction metadata root: {}",
                        opticcode_dir.display()
                    )
                });
            }
        }

        let canonical_metadata_root = fs::canonicalize(&opticcode_dir).with_context(|| {
            format!(
                "failed to resolve transaction metadata root: {}",
                opticcode_dir.display()
            )
        })?;
        if !canonical_metadata_root.starts_with(workspace) {
            return Err(transaction_error(
                ApplyTransactionErrorKind::Precondition,
                format!(
                    "transaction metadata root escapes workspace: {}",
                    opticcode_dir.display()
                ),
            ));
        }

        let lock_path = opticcode_dir.join(WORKSPACE_LOCK_FILE);
        if let Ok(metadata) = fs::symlink_metadata(&lock_path) {
            if metadata_is_link_or_reparse(&metadata) || !metadata.is_file() {
                return Err(transaction_error(
                    ApplyTransactionErrorKind::Precondition,
                    format!(
                        "transaction lock is not a regular file: {}",
                        lock_path.display()
                    ),
                ));
            }
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("failed to open transaction lock: {}", lock_path.display()))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(transaction_error(
                    ApplyTransactionErrorKind::Precondition,
                    format!(
                        "workspace already has an active apply or recovery: {}",
                        workspace.display()
                    ),
                ));
            }
            Err(TryLockError::Error(error)) => {
                return Err(transaction_error(
                    ApplyTransactionErrorKind::Io,
                    format!(
                        "failed to acquire transaction lock {}: {error}",
                        lock_path.display()
                    ),
                ));
            }
        }
        file.sync_all()
            .with_context(|| format!("failed to sync transaction lock: {}", lock_path.display()))?;
        sync_directory(&opticcode_dir)?;
        Ok(Self { _file: file })
    }
}

impl Drop for WorkspaceTransactionLock {
    fn drop(&mut self) {
        // Release explicitly before closing the handle so a following apply,
        // rollback, or recovery in the same process can acquire it immediately.
        let _ = self._file.unlock();
    }
}

impl PreparedTransaction {
    #[allow(clippy::too_many_arguments)]
    fn result(
        &self,
        operation_success: bool,
        final_state: ApplyTransactionState,
        rollback_attempted: bool,
        rollback_success: Option<bool>,
        modified_files: Vec<String>,
        restored_files: Vec<String>,
        errors: Vec<String>,
        warnings: Vec<String>,
        git_restored: Option<bool>,
    ) -> ApplyTransactionResult {
        ApplyTransactionResult {
            schema_version: APPLY_TRANSACTION_SCHEMA_VERSION,
            transaction_id: self.manifest.transaction_id.clone(),
            workspace: self.workspace.display().to_string(),
            operation_success,
            final_state,
            rollback_attempted,
            rollback_success,
            planned_files: self
                .manifest
                .files
                .iter()
                .map(|file| file.path.clone())
                .collect(),
            modified_files,
            restored_files,
            errors,
            warnings,
            duration_ms: duration_ms(self.started_at.elapsed()),
            git_restored,
            transaction_dir: relative_transaction_dir(&self.workspace, &self.run_dir),
        }
    }
}

fn prepare_transaction(
    request: ApplyTransactionRequest,
    faults: &mut dyn ApplyFaultInjector,
) -> Result<PreparedTransaction> {
    let started_at = Instant::now();
    let workspace = fs::canonicalize(&request.workspace).map_err(|error| {
        transaction_error(
            ApplyTransactionErrorKind::Precondition,
            format!(
                "failed to resolve transaction workspace {}: {error}",
                request.workspace.display()
            ),
        )
    })?;
    if !workspace.is_dir() {
        return Err(transaction_error(
            ApplyTransactionErrorKind::Precondition,
            format!(
                "transaction workspace is not a directory: {}",
                workspace.display()
            ),
        ));
    }
    if request.mutations.is_empty() {
        return Err(transaction_error(
            ApplyTransactionErrorKind::Precondition,
            "transaction must contain at least one file mutation",
        ));
    }

    let mutations = validate_and_sort_mutations(&workspace, request.mutations)?;
    if request.git_policy == ApplyGitPolicy::RequireClean {
        let preflight = capture_git_for_policy(&workspace, request.git_policy)?
            .expect("required Git policy should capture a snapshot");
        let pre_existing_changes = effective_git_changes(&preflight).len();
        if pre_existing_changes != 0 {
            return Err(transaction_error(
                ApplyTransactionErrorKind::Precondition,
                format!(
                    "transaction requires a clean Git worktree; found {pre_existing_changes} pre-existing change(s) outside .opticcode"
                ),
            ));
        }
    }

    let workspace_lock = WorkspaceTransactionLock::acquire(&workspace)?;
    let git_before = capture_git_for_policy(&workspace, request.git_policy)?;
    let pre_existing_changes = git_before
        .as_ref()
        .map_or(0, |snapshot| effective_git_changes(snapshot).len());
    let git_clean = pre_existing_changes == 0;
    if request.git_policy == ApplyGitPolicy::RequireClean && !git_clean {
        return Err(transaction_error(
            ApplyTransactionErrorKind::Precondition,
            format!(
                "transaction requires a clean Git worktree; found {pre_existing_changes} pre-existing change(s) outside .opticcode"
            ),
        ));
    }

    let (transaction_id, run_dir) =
        create_transaction_directory(&workspace, request.requested_id.as_deref())?;
    let backups_dir = run_dir.join(BACKUPS_DIR);
    let events_dir = run_dir.join(EVENTS_DIR);
    fs::create_dir(&backups_dir).with_context(|| {
        format!(
            "failed to create transaction backups: {}",
            backups_dir.display()
        )
    })?;
    fs::create_dir(&events_dir).with_context(|| {
        format!(
            "failed to create transaction events: {}",
            events_dir.display()
        )
    })?;
    sync_directory(&run_dir)?;

    let patch_path = run_dir.join(PATCH_FILE);
    write_new_atomic(&patch_path, &request.patch).with_context(|| {
        format!(
            "failed to persist transaction patch: {}",
            patch_path.display()
        )
    })?;

    let mut files = Vec::with_capacity(mutations.len());
    for (index, mutation) in mutations.iter().enumerate() {
        let absolute_path = workspace.join(&mutation.path);
        let before = read_optional_regular_file(&absolute_path)?;
        if before != mutation.expected_before {
            return Err(transaction_error(
                ApplyTransactionErrorKind::Precondition,
                format!(
                    "file changed after patch planning or did not match its expected state: {}",
                    mutation.path.display()
                ),
            ));
        }

        let metadata = fs::metadata(&absolute_path).ok();
        let backup_path = before.as_ref().map(|bytes| {
            let relative = PathBuf::from(BACKUPS_DIR).join(format!("{index:08}.bin"));
            (relative, bytes)
        });
        if let Some((relative, bytes)) = &backup_path {
            write_new_atomic(&run_dir.join(relative), bytes).with_context(|| {
                format!(
                    "failed to persist transaction backup for {}",
                    mutation.path.display()
                )
            })?;
        }

        files.push(ApplyTransactionFile {
            path: path_to_portable_string(&mutation.path)?,
            existed_before: before.is_some(),
            before_hash: before.as_ref().map(|bytes| fingerprint_bytes(bytes)),
            before_bytes: before
                .as_ref()
                .map_or(0, |bytes| u64::try_from(bytes.len()).unwrap_or(u64::MAX)),
            after_hash: mutation
                .desired_after
                .as_ref()
                .map(|bytes| fingerprint_bytes(bytes)),
            after_bytes: mutation
                .desired_after
                .as_ref()
                .map_or(0, |bytes| u64::try_from(bytes.len()).unwrap_or(u64::MAX)),
            backup_path: backup_path
                .as_ref()
                .map(|(path, _)| path_to_portable_string(path))
                .transpose()?,
            readonly: metadata
                .as_ref()
                .map(|metadata| metadata.permissions().readonly()),
            unix_mode: metadata.as_ref().and_then(unix_mode),
        });

        if index == 0 {
            faults.check(ApplyFaultPoint::AfterFirstBackup)?;
        }
    }

    let created_at_unix_ms = unix_time_ms()?;
    let manifest = ApplyTransactionManifest {
        schema_version: APPLY_TRANSACTION_SCHEMA_VERSION,
        transaction_id: transaction_id.clone(),
        workspace: workspace.display().to_string(),
        copied_from: request
            .copied_from
            .as_ref()
            .map(|path| path.display().to_string()),
        created_at_unix_ms,
        patch_hash: fingerprint_bytes(&request.patch),
        patch_path: PATCH_FILE.to_string(),
        files,
        validation: ApplyTransactionValidation {
            git_policy: request.git_policy,
            git_captured: git_before.is_some(),
            git_clean,
            pre_existing_changes,
            expected_contents_verified: true,
        },
        git_before,
    };
    write_json_new_atomic(&run_dir.join(MANIFEST_FILE), &manifest).with_context(|| {
        format!(
            "failed to persist transaction manifest: {}",
            run_dir.display()
        )
    })?;

    let mut journal = TransactionJournal::new(transaction_id, events_dir, started_at, None, 0);
    journal.transition(
        ApplyTransactionState::Prepared,
        "patch, manifest and rollback backups durably prepared",
        Vec::new(),
        Vec::new(),
        None,
    )?;

    Ok(PreparedTransaction {
        workspace,
        run_dir,
        manifest,
        mutations,
        journal,
        started_at,
        _workspace_lock: workspace_lock,
    })
}

struct TransactionJournal {
    transaction_id: String,
    events_dir: PathBuf,
    started_at: Instant,
    current_state: Option<ApplyTransactionState>,
    next_sequence: u32,
}

impl TransactionJournal {
    fn new(
        transaction_id: String,
        events_dir: PathBuf,
        started_at: Instant,
        current_state: Option<ApplyTransactionState>,
        next_sequence: u32,
    ) -> Self {
        Self {
            transaction_id,
            events_dir,
            started_at,
            current_state,
            next_sequence,
        }
    }

    fn transition(
        &mut self,
        next_state: ApplyTransactionState,
        message: impl Into<String>,
        files: Vec<String>,
        errors: Vec<String>,
        git_snapshot: Option<GitStateSnapshot>,
    ) -> Result<()> {
        if !valid_transition(self.current_state, next_state) {
            return Err(transaction_error(
                ApplyTransactionErrorKind::InvalidTransaction,
                format!(
                    "invalid transaction state transition: {} -> {}",
                    self.current_state
                        .map_or("none", ApplyTransactionState::as_str),
                    next_state.as_str()
                ),
            ));
        }

        let sequence = self.next_sequence;
        let event = ApplyTransactionEvent {
            schema_version: APPLY_TRANSACTION_SCHEMA_VERSION,
            transaction_id: self.transaction_id.clone(),
            sequence,
            recorded_at_unix_ms: unix_time_ms()?,
            state: next_state,
            message: message.into(),
            files,
            errors,
            elapsed_ms: duration_ms(self.started_at.elapsed()),
            git_snapshot,
        };
        let path = self
            .events_dir
            .join(format!("{sequence:08}-{}.json", next_state.as_str()));
        write_json_new_atomic(&path, &event)
            .with_context(|| format!("failed to append transaction event: {}", path.display()))?;
        self.current_state = Some(next_state);
        self.next_sequence = sequence.checked_add(1).ok_or_else(|| {
            transaction_error(
                ApplyTransactionErrorKind::InvalidTransaction,
                "transaction event sequence overflow",
            )
        })?;
        Ok(())
    }
}

fn valid_transition(current: Option<ApplyTransactionState>, next: ApplyTransactionState) -> bool {
    use ApplyTransactionState::{
        Applied, Applying, Committed, Finalizing, Prepared, RollbackFailed, RollbackStarted,
        RolledBack,
    };

    matches!(
        (current, next),
        (None, Prepared)
            | (Some(Prepared), Applying)
            | (Some(Prepared), RollbackStarted)
            | (Some(Applying), Applied)
            | (Some(Applying), RollbackStarted)
            | (Some(Applied), Finalizing)
            | (Some(Applied), RollbackStarted)
            | (Some(Finalizing), Committed)
            | (Some(Finalizing), RollbackStarted)
            | (Some(Committed), RollbackStarted)
            | (Some(RollbackStarted), RolledBack)
            | (Some(RollbackStarted), RollbackFailed)
            | (Some(RollbackFailed), RollbackStarted)
    )
}

pub fn inspect_apply_transaction(
    workspace: &Path,
    transaction_id: &str,
) -> Result<ApplyTransactionInspection> {
    validate_transaction_id(transaction_id)?;
    let workspace = fs::canonicalize(workspace).map_err(|error| {
        transaction_error(
            ApplyTransactionErrorKind::InvalidTransaction,
            format!("failed to resolve transaction workspace: {error}"),
        )
    })?;
    let run_dir = transaction_run_dir(&workspace, transaction_id);
    match fs::symlink_metadata(&run_dir) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(transaction_error(
                ApplyTransactionErrorKind::InvalidTransaction,
                format!("transaction not found: {transaction_id}"),
            ));
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect transaction: {}", run_dir.display()));
        }
    }
    Ok(inspect_run_directory(&workspace, transaction_id, &run_dir))
}

pub fn list_apply_transactions(workspace: &Path) -> Result<Vec<ApplyTransactionInspection>> {
    let workspace = fs::canonicalize(workspace).map_err(|error| {
        transaction_error(
            ApplyTransactionErrorKind::InvalidTransaction,
            format!("failed to resolve transaction workspace: {error}"),
        )
    })?;
    let runs_dir = workspace.join(".opticcode").join("runs");
    if !runs_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut transactions = Vec::new();
    for entry in fs::read_dir(&runs_dir)
        .with_context(|| format!("failed to list transactions: {}", runs_dir.display()))?
    {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if !metadata.is_dir() && !metadata_is_link_or_reparse(&metadata) {
            continue;
        }
        let transaction_id = entry.file_name().to_string_lossy().into_owned();
        if validate_transaction_id(&transaction_id).is_err() {
            transactions.push(ApplyTransactionInspection {
                schema_version: APPLY_TRANSACTION_SCHEMA_VERSION,
                transaction_id,
                valid: false,
                legacy: false,
                manifest: None,
                events: Vec::new(),
                final_state: None,
                recoverable: false,
                recovery_reasons: Vec::new(),
                errors: vec!["transaction directory has an invalid identifier".to_string()],
            });
            continue;
        }
        transactions.push(inspect_run_directory(
            &workspace,
            &transaction_id,
            &entry.path(),
        ));
    }
    transactions.sort_by(|left, right| left.transaction_id.cmp(&right.transaction_id));
    Ok(transactions)
}

fn inspect_run_directory(
    workspace: &Path,
    transaction_id: &str,
    run_dir: &Path,
) -> ApplyTransactionInspection {
    match fs::symlink_metadata(run_dir) {
        Ok(metadata) if !metadata_is_link_or_reparse(&metadata) && metadata.is_dir() => {}
        Ok(_) => {
            return invalid_inspection(
                transaction_id,
                "transaction directory is a symlink, reparse point, or non-directory",
            );
        }
        Err(error) => {
            return invalid_inspection(
                transaction_id,
                format!("failed to inspect transaction directory: {error}"),
            );
        }
    }

    let manifest_path = run_dir.join(MANIFEST_FILE);
    let manifest_metadata = fs::symlink_metadata(&manifest_path);
    if matches!(&manifest_metadata, Err(error) if error.kind() == io::ErrorKind::NotFound) {
        let legacy = is_regular_artifact(&run_dir.join(PATCH_FILE))
            && !run_dir.join(BACKUPS_DIR).exists()
            && !run_dir.join(EVENTS_DIR).exists();
        return ApplyTransactionInspection {
            schema_version: APPLY_TRANSACTION_SCHEMA_VERSION,
            transaction_id: transaction_id.to_string(),
            valid: legacy,
            legacy,
            manifest: None,
            events: Vec::new(),
            final_state: None,
            recoverable: false,
            recovery_reasons: if legacy {
                vec!["legacy apply run can only use reverse-patch undo".to_string()]
            } else {
                Vec::new()
            },
            errors: if legacy {
                Vec::new()
            } else {
                vec!["transaction manifest is missing".to_string()]
            },
        };
    }
    match manifest_metadata {
        Ok(metadata) if !metadata_is_link_or_reparse(&metadata) && metadata.is_file() => {}
        Ok(_) => {
            return invalid_inspection(
                transaction_id,
                "transaction manifest is a symlink, reparse point, or non-file",
            );
        }
        Err(error) => {
            return invalid_inspection(
                transaction_id,
                format!("failed to inspect transaction manifest: {error}"),
            );
        }
    }

    let mut errors = Vec::new();
    let manifest = match read_json::<ApplyTransactionManifest>(&manifest_path) {
        Ok(manifest) => Some(manifest),
        Err(error) => {
            errors.push(format!("invalid transaction manifest: {error:#}"));
            None
        }
    };
    if let Some(manifest) = &manifest {
        if manifest.schema_version != APPLY_TRANSACTION_SCHEMA_VERSION {
            errors.push(format!(
                "unsupported transaction schema version: {}",
                manifest.schema_version
            ));
        }
        if manifest.transaction_id != transaction_id {
            errors.push("manifest transaction id does not match its directory".to_string());
        }
        if Path::new(&manifest.workspace) != workspace {
            errors.push(format!(
                "manifest workspace does not match inspected workspace: {}",
                manifest.workspace
            ));
        }
        if manifest.patch_path != PATCH_FILE {
            errors.push(format!(
                "transaction patch path must be `{PATCH_FILE}`, found `{}`",
                manifest.patch_path
            ));
        } else if !is_regular_artifact(&run_dir.join(PATCH_FILE))
            || fingerprint_file(&run_dir.join(PATCH_FILE)).ok().as_deref()
                != Some(manifest.patch_hash.as_str())
        {
            errors.push("transaction patch is missing or its BLAKE3 hash differs".to_string());
        }
        validate_manifest_artifacts(run_dir, manifest, &mut errors);
    }

    let mut events = Vec::new();
    let events_dir = run_dir.join(EVENTS_DIR);
    let events_metadata = fs::symlink_metadata(&events_dir);
    if matches!(
        events_metadata,
        Ok(ref metadata) if !metadata_is_link_or_reparse(metadata) && metadata.is_dir()
    ) {
        let mut paths = match fs::read_dir(&events_dir) {
            Ok(entries) => entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension()
                        .is_some_and(|extension| extension == "json")
                })
                .collect::<Vec<_>>(),
            Err(error) => {
                errors.push(format!("failed to read transaction events: {error}"));
                Vec::new()
            }
        };
        paths.sort();
        let mut expected_sequence = 0_u32;
        let mut current_state = None;
        for path in paths {
            if !is_regular_artifact(&path) {
                errors.push(format!(
                    "transaction event is a symlink, reparse point, or non-file: {}",
                    path.display()
                ));
                continue;
            }
            match read_json::<ApplyTransactionEvent>(&path) {
                Ok(event) => {
                    if event.schema_version != APPLY_TRANSACTION_SCHEMA_VERSION {
                        errors.push(format!(
                            "event {} has unsupported schema version {}",
                            path.display(),
                            event.schema_version
                        ));
                    }
                    if event.transaction_id != transaction_id {
                        errors.push(format!(
                            "event {} has a mismatched transaction id",
                            path.display()
                        ));
                    }
                    if event.sequence != expected_sequence {
                        errors.push(format!(
                            "event sequence mismatch: expected {expected_sequence}, found {}",
                            event.sequence
                        ));
                    }
                    if !valid_transition(current_state, event.state) {
                        errors.push(format!(
                            "invalid persisted transition: {} -> {}",
                            current_state.map_or("none", ApplyTransactionState::as_str),
                            event.state.as_str()
                        ));
                    }
                    let expected_name =
                        format!("{:08}-{}.json", event.sequence, event.state.as_str());
                    if path.file_name().and_then(|name| name.to_str())
                        != Some(expected_name.as_str())
                    {
                        errors.push(format!(
                            "event filename does not match its sequence/state: {}",
                            path.display()
                        ));
                    }
                    if let Some(manifest) = manifest.as_ref() {
                        validate_event_semantics(&event, manifest, &path, &mut errors);
                    }
                    current_state = Some(event.state);
                    expected_sequence = event.sequence.saturating_add(1);
                    events.push(event);
                }
                Err(error) => errors.push(format!(
                    "invalid or truncated transaction event {}: {error:#}",
                    path.display()
                )),
            }
        }
        if events.is_empty() {
            errors.push("transaction has no readable state event".to_string());
        }
    } else {
        errors.push("transaction events directory is missing".to_string());
    }

    let final_state = events.last().map(|event| event.state);
    let recoverable = errors.is_empty()
        && matches!(
            final_state,
            Some(
                ApplyTransactionState::Prepared
                    | ApplyTransactionState::Applying
                    | ApplyTransactionState::Applied
                    | ApplyTransactionState::Finalizing
                    | ApplyTransactionState::RollbackStarted
                    | ApplyTransactionState::RollbackFailed
            )
        );
    let recovery_reasons = if recoverable {
        vec![format!(
            "transaction ended in non-terminal state `{}` and has validated backups",
            final_state
                .expect("recoverable state should exist")
                .as_str()
        )]
    } else {
        Vec::new()
    };

    ApplyTransactionInspection {
        schema_version: APPLY_TRANSACTION_SCHEMA_VERSION,
        transaction_id: transaction_id.to_string(),
        valid: errors.is_empty(),
        legacy: false,
        manifest,
        events,
        final_state,
        recoverable,
        recovery_reasons,
        errors,
    }
}

fn invalid_inspection(
    transaction_id: &str,
    error: impl Into<String>,
) -> ApplyTransactionInspection {
    ApplyTransactionInspection {
        schema_version: APPLY_TRANSACTION_SCHEMA_VERSION,
        transaction_id: transaction_id.to_string(),
        valid: false,
        legacy: false,
        manifest: None,
        events: Vec::new(),
        final_state: None,
        recoverable: false,
        recovery_reasons: Vec::new(),
        errors: vec![error.into()],
    }
}

fn is_regular_artifact(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| !metadata_is_link_or_reparse(&metadata) && metadata.is_file())
}

fn validate_event_semantics(
    event: &ApplyTransactionEvent,
    manifest: &ApplyTransactionManifest,
    path: &Path,
    errors: &mut Vec<String>,
) {
    let planned = manifest
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<BTreeSet<_>>();
    let event_files = event
        .files
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if event_files.len() != event.files.len() {
        errors.push(format!(
            "event contains duplicate file paths: {}",
            path.display()
        ));
    }
    if !event_files.is_subset(&planned) {
        errors.push(format!(
            "event contains a file outside its manifest: {}",
            path.display()
        ));
    }

    match event.state {
        ApplyTransactionState::Prepared | ApplyTransactionState::Applying => {
            if !event.files.is_empty() {
                errors.push(format!(
                    "event state {} must not claim modified files: {}",
                    event.state.as_str(),
                    path.display()
                ));
            }
        }
        ApplyTransactionState::Applied
        | ApplyTransactionState::Finalizing
        | ApplyTransactionState::Committed
        | ApplyTransactionState::RolledBack => {
            if event_files != planned {
                errors.push(format!(
                    "event state {} must contain every planned file: {}",
                    event.state.as_str(),
                    path.display()
                ));
            }
        }
        ApplyTransactionState::RollbackStarted | ApplyTransactionState::RollbackFailed => {}
    }

    if matches!(
        event.state,
        ApplyTransactionState::Committed | ApplyTransactionState::RolledBack
    ) && !event.errors.is_empty()
    {
        errors.push(format!(
            "successful terminal event contains errors: {}",
            path.display()
        ));
    }
    if event.state == ApplyTransactionState::RollbackFailed && event.errors.is_empty() {
        errors.push(format!(
            "rollback_failed event has no recorded error: {}",
            path.display()
        ));
    }
    if event.state == ApplyTransactionState::Committed
        && event.git_snapshot.is_some() != manifest.git_before.is_some()
    {
        errors.push(format!(
            "committed event Git snapshot does not match manifest policy: {}",
            path.display()
        ));
    }
    if event.state == ApplyTransactionState::RolledBack
        && event.git_snapshot.is_some() != manifest.git_before.is_some()
    {
        errors.push(format!(
            "rolled_back event Git snapshot does not match manifest policy: {}",
            path.display()
        ));
    }
}

fn validate_manifest_artifacts(
    run_dir: &Path,
    manifest: &ApplyTransactionManifest,
    errors: &mut Vec<String>,
) {
    if manifest.files.is_empty() {
        errors.push("transaction manifest has no planned file".to_string());
    }
    if manifest.validation.git_captured != manifest.git_before.is_some() {
        errors.push("manifest Git capture flag does not match its snapshot".to_string());
    }
    if !manifest.validation.expected_contents_verified {
        errors.push("manifest did not verify expected file contents".to_string());
    }
    if manifest.validation.git_clean != (manifest.validation.pre_existing_changes == 0) {
        errors.push("manifest Git cleanliness metadata is inconsistent".to_string());
    }
    if manifest.validation.git_policy == ApplyGitPolicy::RequireClean
        && (!manifest.validation.git_captured || !manifest.validation.git_clean)
    {
        errors.push("clean-Git transaction manifest contains an invalid Git state".to_string());
    }

    let backups_dir = run_dir.join(BACKUPS_DIR);
    match fs::symlink_metadata(&backups_dir) {
        Ok(metadata) if !metadata_is_link_or_reparse(&metadata) && metadata.is_dir() => {}
        Ok(_) => {
            errors.push(
                "transaction backups directory is a symlink, reparse point, or non-directory"
                    .to_string(),
            );
            return;
        }
        Err(error) => {
            errors.push(format!(
                "transaction backups directory cannot be inspected: {error}"
            ));
            return;
        }
    }

    let mut seen = BTreeSet::new();
    for (index, file) in manifest.files.iter().enumerate() {
        match validate_relative_path(Path::new(&file.path)) {
            Ok(path) => {
                let portable = path.to_string_lossy().replace('\\', "/");
                if !seen.insert(portable) {
                    errors.push(format!("duplicate manifest file path: {}", file.path));
                }
            }
            Err(error) => errors.push(format!(
                "invalid manifest file path `{}`: {error:#}",
                file.path
            )),
        }

        if file.after_hash.is_none() && file.after_bytes != 0 {
            errors.push(format!(
                "deleted file has inconsistent post-apply size: {}",
                file.path
            ));
        }

        if file.existed_before {
            let expected_backup = format!("{BACKUPS_DIR}/{index:08}.bin");
            if file.backup_path.as_deref() != Some(expected_backup.as_str()) {
                errors.push(format!(
                    "manifest backup path is missing or unexpected for {}",
                    file.path
                ));
                continue;
            }
            let Some(expected_hash) = file.before_hash.as_deref() else {
                errors.push(format!("manifest backup hash is missing for {}", file.path));
                continue;
            };
            let backup_path = run_dir.join(&expected_backup);
            if !is_regular_artifact(&backup_path) {
                errors.push(format!(
                    "manifest backup is a symlink, reparse point, or non-file for {}",
                    file.path
                ));
                continue;
            }
            match fs::read(&backup_path) {
                Ok(bytes) => {
                    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != file.before_bytes {
                        errors.push(format!("manifest backup size differs for {}", file.path));
                    }
                    if fingerprint_bytes(&bytes) != expected_hash {
                        errors.push(format!("manifest backup BLAKE3 differs for {}", file.path));
                    }
                }
                Err(error) => errors.push(format!(
                    "manifest backup cannot be read for {}: {error}",
                    file.path
                )),
            }
        } else if file.before_hash.is_some()
            || file.before_bytes != 0
            || file.backup_path.is_some()
            || file.readonly.is_some()
            || file.unix_mode.is_some()
        {
            errors.push(format!(
                "new file contains inconsistent pre-transaction metadata: {}",
                file.path
            ));
        }
    }
}

fn rollback_apply_transaction_with_faults(
    workspace: &Path,
    transaction_id: &str,
    reason: &str,
    faults: &mut dyn ApplyFaultInjector,
) -> Result<ApplyTransactionResult> {
    validate_transaction_id(transaction_id)?;
    let workspace = fs::canonicalize(workspace).map_err(|error| {
        transaction_error(
            ApplyTransactionErrorKind::InvalidTransaction,
            format!("failed to resolve transaction workspace: {error}"),
        )
    })?;
    let _workspace_lock = WorkspaceTransactionLock::acquire(&workspace)?;
    let inspection = inspect_apply_transaction(&workspace, transaction_id)?;
    if inspection.legacy {
        return Err(transaction_error(
            ApplyTransactionErrorKind::InvalidTransaction,
            "legacy apply run requires reverse-patch undo",
        ));
    }
    if !inspection.valid {
        return Err(transaction_error(
            ApplyTransactionErrorKind::InvalidTransaction,
            format!("transaction is invalid: {}", inspection.errors.join("; ")),
        ));
    }
    let manifest = inspection.manifest.ok_or_else(|| {
        transaction_error(
            ApplyTransactionErrorKind::InvalidTransaction,
            "transaction manifest is unavailable",
        )
    })?;
    let run_dir = transaction_run_dir(&workspace, transaction_id);
    let final_state = inspection.final_state.ok_or_else(|| {
        transaction_error(
            ApplyTransactionErrorKind::InvalidTransaction,
            "transaction has no state event",
        )
    })?;

    if final_state == ApplyTransactionState::RolledBack {
        if !verify_manifest_files_restored(&workspace, &manifest)? {
            return Err(transaction_error(
                ApplyTransactionErrorKind::Precondition,
                "transaction was rolled back, but a managed file changed afterwards",
            ));
        }
        let (git_restored, _) = verify_git_restored(&manifest)?;
        if git_restored == Some(false) {
            return Err(transaction_error(
                ApplyTransactionErrorKind::Precondition,
                "transaction was rolled back, but Git state changed afterwards",
            ));
        }
        return Ok(result_from_manifest(
            &manifest,
            &run_dir,
            true,
            ApplyTransactionState::RolledBack,
            false,
            Some(true),
            Vec::new(),
            manifest
                .files
                .iter()
                .map(|file| file.path.clone())
                .collect(),
            Vec::new(),
            vec!["transaction was already rolled back".to_string()],
            git_restored,
            Duration::ZERO,
        ));
    }
    if final_state == ApplyTransactionState::Committed && reason.contains("recovery") {
        return Err(transaction_error(
            ApplyTransactionErrorKind::Precondition,
            "committed transaction is not incomplete; use explicit apply --undo",
        ));
    }

    let started_at = Instant::now();
    let mut journal = TransactionJournal::new(
        transaction_id.to_string(),
        run_dir.join(EVENTS_DIR),
        started_at,
        Some(final_state),
        u32::try_from(inspection.events.len()).map_err(|_| {
            transaction_error(
                ApplyTransactionErrorKind::InvalidTransaction,
                "too many transaction events",
            )
        })?,
    );

    rollback_manifest(
        &workspace,
        &run_dir,
        &manifest,
        &mut journal,
        faults,
        Vec::new(),
        reason.to_string(),
        true,
        started_at,
    )
}

fn rollback_prepared_context(
    context: &mut PreparedTransaction,
    faults: &mut dyn ApplyFaultInjector,
    modified_files: Vec<String>,
    reason: String,
    rollback_operation: bool,
) -> Result<ApplyTransactionResult> {
    rollback_manifest(
        &context.workspace,
        &context.run_dir,
        &context.manifest,
        &mut context.journal,
        faults,
        modified_files,
        reason,
        rollback_operation,
        context.started_at,
    )
}

#[allow(clippy::too_many_arguments)]
fn rollback_manifest(
    workspace: &Path,
    run_dir: &Path,
    manifest: &ApplyTransactionManifest,
    journal: &mut TransactionJournal,
    faults: &mut dyn ApplyFaultInjector,
    modified_files: Vec<String>,
    reason: String,
    rollback_operation: bool,
    started_at: Instant,
) -> Result<ApplyTransactionResult> {
    let mut rollback_errors = Vec::new();
    let mut restored_files = Vec::new();

    if journal.current_state != Some(ApplyTransactionState::RollbackStarted) {
        if let Err(error) = journal.transition(
            ApplyTransactionState::RollbackStarted,
            "automatic or explicit rollback started",
            modified_files.clone(),
            vec![reason.clone()],
            None,
        ) {
            rollback_errors.push(format!("failed to journal rollback start: {error:#}"));
        }
    }

    if let Err(error) = faults.check(ApplyFaultPoint::RollbackStarted) {
        rollback_errors.push(format!("rollback start failed: {error:#}"));
    } else {
        for (index, file) in manifest.files.iter().enumerate().rev() {
            match restore_transaction_file(workspace, run_dir, file) {
                Ok(()) => restored_files.push(file.path.clone()),
                Err(error) => {
                    rollback_errors.push(format!("failed to restore {}: {error:#}", file.path));
                    break;
                }
            }
            if index == manifest.files.len().saturating_sub(1) {
                if let Err(error) = faults.check(ApplyFaultPoint::AfterFirstRestore) {
                    rollback_errors.push(format!("rollback restoration interrupted: {error:#}"));
                    break;
                }
            }
        }
    }

    let (git_restored, rollback_git_snapshot) = match verify_git_restored(manifest) {
        Ok(value) => value,
        Err(error) => {
            rollback_errors.push(format!(
                "failed to verify Git state after rollback: {error:#}"
            ));
            (Some(false), None)
        }
    };
    if git_restored == Some(false)
        && !rollback_errors
            .iter()
            .any(|error| error.contains("Git state"))
    {
        rollback_errors
            .push("Git state after rollback differs from the pre-transaction state".to_string());
    }

    let content_rollback_ok = rollback_errors.is_empty() && git_restored != Some(false);
    let journal_state = if content_rollback_ok {
        ApplyTransactionState::RolledBack
    } else {
        ApplyTransactionState::RollbackFailed
    };
    let event_errors = if content_rollback_ok {
        Vec::new()
    } else {
        rollback_errors.clone()
    };
    if let Err(error) = journal.transition(
        journal_state,
        if content_rollback_ok {
            "rollback completed and verified"
        } else {
            "rollback did not complete cleanly"
        },
        restored_files.clone(),
        event_errors,
        rollback_git_snapshot,
    ) {
        rollback_errors.push(format!("failed to journal rollback result: {error:#}"));
    }

    let rollback_ok = content_rollback_ok && rollback_errors.is_empty();
    let final_state = if rollback_ok {
        ApplyTransactionState::RolledBack
    } else {
        ApplyTransactionState::RollbackFailed
    };

    let mut result_errors = if rollback_operation {
        Vec::new()
    } else {
        vec![reason]
    };
    result_errors.extend(rollback_errors.iter().cloned());

    Ok(result_from_manifest(
        manifest,
        run_dir,
        rollback_operation && rollback_ok,
        final_state,
        true,
        Some(rollback_ok),
        modified_files,
        restored_files,
        result_errors,
        Vec::new(),
        git_restored,
        started_at.elapsed(),
    ))
}

fn apply_mutation(
    workspace: &Path,
    mutation: &FileMutation,
    file_record: &ApplyTransactionFile,
    faults: &mut dyn ApplyFaultInjector,
) -> Result<()> {
    let target = workspace.join(&mutation.path);
    let current = read_validated_target(workspace, &mutation.path)?;
    if current != mutation.expected_before {
        return Err(transaction_error(
            ApplyTransactionErrorKind::Precondition,
            format!(
                "concurrent file change detected immediately before apply: {}",
                mutation.path.display()
            ),
        ));
    }

    match &mutation.desired_after {
        Some(bytes) => {
            atomic_replace_bytes_checked(
                workspace,
                &mutation.path,
                bytes,
                mutation.expected_before.as_deref(),
                Some(faults),
            )
            .with_context(|| {
                format!(
                    "failed to atomically write target file: {}",
                    target.display()
                )
            })?;
            restore_permissions(workspace, &mutation.path, file_record)?;
            let actual = read_validated_target(workspace, &mutation.path)?;
            if actual.as_deref() != Some(bytes.as_slice()) {
                return Err(transaction_error(
                    ApplyTransactionErrorKind::Io,
                    format!(
                        "target verification failed after write: {}",
                        target.display()
                    ),
                ));
            }
        }
        None => {
            let latest = read_validated_target(workspace, &mutation.path)?;
            if latest != mutation.expected_before {
                return Err(transaction_error(
                    ApplyTransactionErrorKind::Precondition,
                    format!(
                        "concurrent file change detected immediately before delete: {}",
                        mutation.path.display()
                    ),
                ));
            }
            fs::remove_file(&target)
                .with_context(|| format!("failed to delete target file: {}", target.display()))?;
            sync_parent_directory(&target)?;
            if read_validated_target(workspace, &mutation.path)?.is_some() {
                return Err(transaction_error(
                    ApplyTransactionErrorKind::Io,
                    format!("delete target still exists: {}", target.display()),
                ));
            }
        }
    }
    Ok(())
}

fn restore_transaction_file(
    workspace: &Path,
    run_dir: &Path,
    file: &ApplyTransactionFile,
) -> Result<()> {
    let relative = validate_relative_path(Path::new(&file.path))?;
    let target = workspace.join(&relative);
    let current = read_validated_target(workspace, &relative)?;
    let current_hash = current.as_ref().map(|bytes| fingerprint_bytes(bytes));

    if current_hash == file.before_hash {
        if file.existed_before {
            restore_permissions(workspace, &relative, file)?;
        }
        return Ok(());
    }
    if current_hash != file.after_hash {
        return Err(transaction_error(
            ApplyTransactionErrorKind::Precondition,
            format!(
                "refusing to overwrite a file changed outside the transaction: {}",
                file.path
            ),
        ));
    }

    if file.existed_before {
        let backup_relative = file.backup_path.as_ref().ok_or_else(|| {
            transaction_error(
                ApplyTransactionErrorKind::InvalidTransaction,
                format!("backup path is missing for {}", file.path),
            )
        })?;
        let backup_relative = validate_relative_path(Path::new(backup_relative))?;
        let backup_path = run_dir.join(&backup_relative);
        if !backup_path.starts_with(run_dir) {
            return Err(transaction_error(
                ApplyTransactionErrorKind::InvalidTransaction,
                format!("backup escapes transaction directory: {}", file.path),
            ));
        }
        let bytes = fs::read(&backup_path)
            .with_context(|| format!("failed to read backup: {}", backup_path.display()))?;
        let expected_hash = file.before_hash.as_deref().ok_or_else(|| {
            transaction_error(
                ApplyTransactionErrorKind::InvalidTransaction,
                format!("backup hash is missing for {}", file.path),
            )
        })?;
        if fingerprint_bytes(&bytes) != expected_hash {
            return Err(transaction_error(
                ApplyTransactionErrorKind::InvalidTransaction,
                format!("backup hash mismatch for {}", file.path),
            ));
        }
        atomic_replace_bytes_checked(workspace, &relative, &bytes, current.as_deref(), None)
            .with_context(|| format!("failed to restore target: {}", target.display()))?;
        restore_permissions(workspace, &relative, file)?;
        let restored = read_validated_target(workspace, &relative)?;
        if restored
            .as_ref()
            .map(|bytes| fingerprint_bytes(bytes))
            .as_deref()
            != Some(expected_hash)
        {
            return Err(transaction_error(
                ApplyTransactionErrorKind::Io,
                format!("restored file hash mismatch for {}", file.path),
            ));
        }
    } else if current.is_some() {
        let latest = read_validated_target(workspace, &relative)?;
        if latest != current {
            return Err(transaction_error(
                ApplyTransactionErrorKind::Precondition,
                format!(
                    "refusing to remove a transaction-created file changed concurrently: {}",
                    file.path
                ),
            ));
        }
        fs::remove_file(&target).with_context(|| {
            format!(
                "failed to remove transaction-created file: {}",
                target.display()
            )
        })?;
        sync_parent_directory(&target)?;
    }

    Ok(())
}

fn validate_and_sort_mutations(
    workspace: &Path,
    mutations: Vec<FileMutation>,
) -> Result<Vec<FileMutation>> {
    let mut normalized = Vec::with_capacity(mutations.len());
    let mut seen = BTreeSet::new();
    for mut mutation in mutations {
        mutation.path = validate_relative_path(&mutation.path)?;
        validate_target_location(workspace, &mutation.path)?;
        let portable = path_to_portable_string(&mutation.path)?;
        if !seen.insert(portable.clone()) {
            return Err(transaction_error(
                ApplyTransactionErrorKind::Precondition,
                format!("duplicate transaction file: {portable}"),
            ));
        }
        if mutation.expected_before == mutation.desired_after {
            return Err(transaction_error(
                ApplyTransactionErrorKind::Precondition,
                format!("transaction mutation has no content change: {portable}"),
            ));
        }
        normalized.push(mutation);
    }
    normalized.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(normalized)
}

fn validate_relative_path(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(transaction_error(
            ApplyTransactionErrorKind::Precondition,
            format!(
                "transaction path must be a non-empty relative path: {}",
                path.display()
            ),
        ));
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => normalized.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(transaction_error(
                    ApplyTransactionErrorKind::Precondition,
                    format!("transaction path escapes its workspace: {}", path.display()),
                ));
            }
        }
    }
    if normalized.as_os_str().is_empty()
        || normalized
            .components()
            .any(|component| component.as_os_str() == ".opticcode")
    {
        return Err(transaction_error(
            ApplyTransactionErrorKind::Precondition,
            format!("transaction path is reserved or empty: {}", path.display()),
        ));
    }
    if normalized.to_str().is_none() {
        return Err(transaction_error(
            ApplyTransactionErrorKind::Precondition,
            format!("transaction path is not valid Unicode: {}", path.display()),
        ));
    }
    Ok(normalized)
}

fn validate_target_location(workspace: &Path, relative: &Path) -> Result<()> {
    let relative = validate_relative_path(relative)?;
    let target = workspace.join(&relative);
    let component_count = relative.components().count();
    let mut current = workspace.to_path_buf();
    for (index, component) in relative.components().enumerate() {
        current.push(component.as_os_str());
        let is_target = index + 1 == component_count;
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata_is_link_or_reparse(&metadata) {
                    return Err(transaction_error(
                        ApplyTransactionErrorKind::Precondition,
                        format!(
                            "transaction path contains a symlink or reparse point: {}",
                            current.display()
                        ),
                    ));
                }
                if (!is_target && !metadata.is_dir()) || (is_target && !metadata.is_file()) {
                    return Err(transaction_error(
                        ApplyTransactionErrorKind::Precondition,
                        format!(
                            "transaction path component has an invalid type: {}",
                            current.display()
                        ),
                    ));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound && is_target => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(transaction_error(
                    ApplyTransactionErrorKind::Precondition,
                    format!(
                        "transaction target parent must already exist: {}",
                        current.display()
                    ),
                ));
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to inspect transaction path: {}", current.display())
                });
            }
        }
    }

    let parent = target.parent().ok_or_else(|| {
        transaction_error(
            ApplyTransactionErrorKind::Precondition,
            format!("transaction target has no parent: {}", target.display()),
        )
    })?;
    if !parent.is_dir() {
        return Err(transaction_error(
            ApplyTransactionErrorKind::Precondition,
            format!(
                "transaction target parent must already exist: {}",
                parent.display()
            ),
        ));
    }
    let canonical_parent = fs::canonicalize(parent).map_err(|error| {
        transaction_error(
            ApplyTransactionErrorKind::Precondition,
            format!(
                "failed to resolve target parent {}: {error}",
                parent.display()
            ),
        )
    })?;
    if !canonical_parent.starts_with(workspace) {
        return Err(transaction_error(
            ApplyTransactionErrorKind::Precondition,
            format!("transaction target escapes workspace: {}", target.display()),
        ));
    }
    Ok(())
}

fn read_optional_regular_file(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata_is_link_or_reparse(&metadata) || !metadata.file_type().is_file() {
                return Err(transaction_error(
                    ApplyTransactionErrorKind::Precondition,
                    format!("transaction path is not a regular file: {}", path.display()),
                ));
            }
            Ok(Some(fs::read(path).with_context(|| {
                format!("failed to read transaction file: {}", path.display())
            })?))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error)
            .with_context(|| format!("failed to inspect transaction file: {}", path.display())),
    }
}

fn read_validated_target(workspace: &Path, relative: &Path) -> Result<Option<Vec<u8>>> {
    validate_target_location(workspace, relative)?;
    read_optional_regular_file(&workspace.join(relative))
}

#[cfg(windows)]
fn metadata_is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn capture_git_for_policy(
    workspace: &Path,
    policy: ApplyGitPolicy,
) -> Result<Option<GitStateSnapshot>> {
    match capture_git_state(workspace) {
        Ok(snapshot) => Ok(Some(snapshot)),
        Err(_error) if policy == ApplyGitPolicy::Optional => Ok(None),
        Err(error) => Err(transaction_error(
            ApplyTransactionErrorKind::Precondition,
            format!("transaction requires a Git worktree: {error:#}"),
        )),
    }
}

fn capture_git_for_manifest(
    manifest: &ApplyTransactionManifest,
) -> Result<Option<GitStateSnapshot>> {
    if manifest.git_before.is_none() {
        return Ok(None);
    }
    capture_git_state(Path::new(&manifest.workspace)).map(Some)
}

fn verify_git_restored(
    manifest: &ApplyTransactionManifest,
) -> Result<(Option<bool>, Option<GitStateSnapshot>)> {
    let Some(before) = manifest.git_before.as_ref() else {
        return Ok((None, None));
    };
    let after = capture_git_state(Path::new(&manifest.workspace))?;
    let restored = effective_git_changes(before) == effective_git_changes(&after);
    Ok((Some(restored), Some(after)))
}

fn verify_mutations_applied(workspace: &Path, mutations: &[FileMutation]) -> Result<()> {
    for mutation in mutations {
        let current = read_validated_target(workspace, &mutation.path)?;
        if current != mutation.desired_after {
            return Err(transaction_error(
                ApplyTransactionErrorKind::Precondition,
                format!(
                    "transaction target changed before finalization: {}",
                    mutation.path.display()
                ),
            ));
        }
    }
    Ok(())
}

fn verify_manifest_files_restored(
    workspace: &Path,
    manifest: &ApplyTransactionManifest,
) -> Result<bool> {
    for file in &manifest.files {
        let relative = validate_relative_path(Path::new(&file.path))?;
        let current = read_validated_target(workspace, &relative)?;
        let current_hash = current.as_ref().map(|bytes| fingerprint_bytes(bytes));
        if current_hash != file.before_hash {
            return Ok(false);
        }
        if file.existed_before {
            let metadata = fs::symlink_metadata(workspace.join(&relative))?;
            if file.readonly != Some(metadata.permissions().readonly())
                || file.unix_mode != unix_mode(&metadata)
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn verify_git_state_for_commit(
    manifest: &ApplyTransactionManifest,
    after: Option<&GitStateSnapshot>,
) -> Result<()> {
    let Some(before) = manifest.git_before.as_ref() else {
        return Ok(());
    };
    let after = after.ok_or_else(|| {
        transaction_error(
            ApplyTransactionErrorKind::Precondition,
            "Git state disappeared before transaction commit",
        )
    })?;
    if before.root != after.root {
        return Err(transaction_error(
            ApplyTransactionErrorKind::Precondition,
            format!(
                "Git root changed before transaction commit: {} -> {}",
                before.root.display(),
                after.root.display()
            ),
        ));
    }

    let workspace = Path::new(&manifest.workspace);
    let mut managed_paths = BTreeSet::new();
    for file in &manifest.files {
        let absolute = workspace.join(validate_relative_path(Path::new(&file.path))?);
        let relative = absolute.strip_prefix(&before.root).map_err(|_| {
            transaction_error(
                ApplyTransactionErrorKind::Precondition,
                format!(
                    "managed transaction file is outside the Git root: {}",
                    absolute.display()
                ),
            )
        })?;
        managed_paths.insert(path_to_portable_string(relative)?);
    }

    let unrelated_before = unrelated_git_changes(before, &managed_paths);
    let unrelated_after = unrelated_git_changes(after, &managed_paths);
    if unrelated_before != unrelated_after {
        return Err(transaction_error(
            ApplyTransactionErrorKind::Precondition,
            "unrelated Git worktree changes appeared or changed during transaction",
        ));
    }
    Ok(())
}

fn unrelated_git_changes(
    snapshot: &GitStateSnapshot,
    managed_paths: &BTreeSet<String>,
) -> Vec<GitChange> {
    effective_git_changes(snapshot)
        .into_iter()
        .filter(|change| {
            !managed_paths.contains(&change.path.replace('\\', "/"))
                && !change
                    .original_path
                    .as_ref()
                    .is_some_and(|path| managed_paths.contains(&path.replace('\\', "/")))
        })
        .collect()
}

fn effective_git_changes(snapshot: &GitStateSnapshot) -> Vec<GitChange> {
    snapshot
        .changes
        .iter()
        .filter(|change| {
            !change
                .path
                .replace('\\', "/")
                .split('/')
                .any(|component| component == ".opticcode")
        })
        .cloned()
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn result_from_manifest(
    manifest: &ApplyTransactionManifest,
    run_dir: &Path,
    operation_success: bool,
    final_state: ApplyTransactionState,
    rollback_attempted: bool,
    rollback_success: Option<bool>,
    modified_files: Vec<String>,
    restored_files: Vec<String>,
    errors: Vec<String>,
    warnings: Vec<String>,
    git_restored: Option<bool>,
    duration: Duration,
) -> ApplyTransactionResult {
    ApplyTransactionResult {
        schema_version: APPLY_TRANSACTION_SCHEMA_VERSION,
        transaction_id: manifest.transaction_id.clone(),
        workspace: manifest.workspace.clone(),
        operation_success,
        final_state,
        rollback_attempted,
        rollback_success,
        planned_files: manifest
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect(),
        modified_files,
        restored_files,
        errors,
        warnings,
        duration_ms: duration_ms(duration),
        git_restored,
        transaction_dir: relative_transaction_dir(Path::new(&manifest.workspace), run_dir),
    }
}

fn create_transaction_directory(
    workspace: &Path,
    requested_id: Option<&str>,
) -> Result<(String, PathBuf)> {
    let opticcode_dir = workspace.join(".opticcode");
    let runs_dir = opticcode_dir.join("runs");
    ensure_workspace_directory(workspace, &runs_dir, "transaction runs directory")?;

    if let Some(transaction_id) = requested_id {
        validate_transaction_id(transaction_id)?;
        let run_dir = runs_dir.join(transaction_id);
        match fs::create_dir(&run_dir) {
            Ok(()) => {
                sync_directory(&runs_dir)?;
                verify_workspace_directory(workspace, &run_dir, "transaction directory")?;
                return Ok((transaction_id.to_string(), run_dir));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return Err(transaction_error(
                    ApplyTransactionErrorKind::Collision,
                    format!("transaction already exists: {transaction_id}"),
                ));
            }
            Err(error) => return Err(error).context("failed to create transaction directory"),
        }
    }

    for _ in 0..100 {
        let counter = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let transaction_id = format!(
            "apply-{}-{}-{counter}",
            unix_time_nanos()?,
            std::process::id()
        );
        let run_dir = runs_dir.join(&transaction_id);
        match fs::create_dir(&run_dir) {
            Ok(()) => {
                sync_directory(&runs_dir)?;
                verify_workspace_directory(workspace, &run_dir, "transaction directory")?;
                return Ok((transaction_id, run_dir));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("failed to create transaction directory"),
        }
    }

    Err(transaction_error(
        ApplyTransactionErrorKind::Collision,
        "could not allocate a unique transaction identifier",
    ))
}

fn ensure_workspace_directory(workspace: &Path, path: &Path, label: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(path)
                .with_context(|| format!("failed to create {label}: {}", path.display()))?;
            if let Some(parent) = path.parent() {
                sync_directory(parent)?;
            }
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect {label}: {}", path.display()));
        }
    }
    verify_workspace_directory(workspace, path, label)
}

fn verify_workspace_directory(workspace: &Path, path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect {label}: {}", path.display()))?;
    if metadata_is_link_or_reparse(&metadata) || !metadata.is_dir() {
        return Err(transaction_error(
            ApplyTransactionErrorKind::Precondition,
            format!("{label} is not a regular directory: {}", path.display()),
        ));
    }
    let canonical = fs::canonicalize(path)
        .with_context(|| format!("failed to resolve {label}: {}", path.display()))?;
    if !canonical.starts_with(workspace) {
        return Err(transaction_error(
            ApplyTransactionErrorKind::Precondition,
            format!("{label} escapes workspace: {}", path.display()),
        ));
    }
    sync_directory(path)?;
    Ok(())
}

pub fn validate_transaction_id(transaction_id: &str) -> Result<()> {
    if transaction_id.is_empty() {
        return Err(transaction_error(
            ApplyTransactionErrorKind::InvalidTransaction,
            "transaction id is empty",
        ));
    }
    if transaction_id.len() > 160 {
        return Err(transaction_error(
            ApplyTransactionErrorKind::InvalidTransaction,
            "transaction id is too long",
        ));
    }
    if !transaction_id
        .chars()
        .all(|value| value.is_ascii_alphanumeric() || value == '-' || value == '_')
    {
        return Err(transaction_error(
            ApplyTransactionErrorKind::InvalidTransaction,
            format!("transaction id contains invalid characters: {transaction_id}"),
        ));
    }
    Ok(())
}

fn transaction_run_dir(workspace: &Path, transaction_id: &str) -> PathBuf {
    workspace
        .join(".opticcode")
        .join("runs")
        .join(transaction_id)
}

fn relative_transaction_dir(workspace: &Path, run_dir: &Path) -> String {
    run_dir
        .strip_prefix(workspace)
        .unwrap_or(run_dir)
        .display()
        .to_string()
}

fn write_json_new_atomic<T>(path: &Path, value: &T) -> Result<()>
where
    T: Serialize,
{
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    write_new_atomic(path, &bytes)
}

fn read_json<T>(path: &Path) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("failed to parse {}", path.display()))
}

fn write_new_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.exists() {
        return Err(transaction_error(
            ApplyTransactionErrorKind::Collision,
            format!(
                "refusing to overwrite existing transaction artifact: {}",
                path.display()
            ),
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        transaction_error(
            ApplyTransactionErrorKind::Io,
            format!("transaction artifact has no parent: {}", path.display()),
        )
    })?;
    let temp = unique_temp_path(
        parent,
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("artifact"),
    );
    write_synced_file(&temp, bytes)?;
    if let Err(error) = move_new_file(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(error)
            .with_context(|| format!("failed to publish transaction artifact {}", path.display()));
    }
    sync_directory(parent)?;
    Ok(())
}

fn atomic_replace_bytes_checked(
    workspace: &Path,
    relative: &Path,
    bytes: &[u8],
    expected_current: Option<&[u8]>,
    staged_faults: Option<&mut dyn ApplyFaultInjector>,
) -> Result<()> {
    validate_target_location(workspace, relative)?;
    let path = workspace.join(relative);
    let parent = path.parent().ok_or_else(|| {
        transaction_error(
            ApplyTransactionErrorKind::Io,
            format!("target file has no parent: {}", path.display()),
        )
    })?;
    let temp = unique_temp_path(parent, "target");
    write_synced_file(&temp, bytes)?;
    if let Some(faults) = staged_faults {
        if let Err(error) = faults.check(ApplyFaultPoint::AfterTargetStaged) {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
    }
    let current = match read_validated_target(workspace, relative) {
        Ok(current) => current,
        Err(error) => {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
    };
    if current.as_deref() != expected_current {
        let _ = fs::remove_file(&temp);
        return Err(transaction_error(
            ApplyTransactionErrorKind::Precondition,
            format!(
                "concurrent file change detected after staging temporary content: {}",
                relative.display()
            ),
        ));
    }
    let existed = current.is_some();
    let result = if existed {
        replace_existing_file(&temp, &path)
    } else {
        move_new_file(&temp, &path)
    };
    if let Err(error) = result {
        let _ = fs::remove_file(&temp);
        return Err(error).with_context(|| format!("failed to replace {}", path.display()));
    }
    sync_directory(parent)?;
    Ok(())
}

fn write_synced_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("failed to create temporary file: {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("failed to write temporary file: {}", path.display()))?;
    file.flush()?;
    file.sync_all()
        .with_context(|| format!("failed to sync temporary file: {}", path.display()))?;
    Ok(())
}

fn unique_temp_path(parent: &Path, label: &str) -> PathBuf {
    let counter = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(
        ".opticcode-{label}-{}-{counter}.tmp",
        std::process::id()
    ))
}

#[cfg(windows)]
fn move_new_file(source: &Path, destination: &Path) -> io::Result<()> {
    windows::move_new(source, destination)
}

#[cfg(not(windows))]
fn move_new_file(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_existing_file(source: &Path, destination: &Path) -> io::Result<()> {
    windows::replace_existing(source, destination)
}

#[cfg(not(windows))]
fn replace_existing_file(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

fn sync_parent_directory(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("failed to sync directory: {}", path.display()))
}

#[cfg(windows)]
fn sync_directory(_path: &Path) -> Result<()> {
    // MOVEFILE_WRITE_THROUGH covers new-file publication. ReplaceFileW has no
    // supported write-through flag, and std cannot portably sync a Windows
    // directory handle, so replacement metadata has a documented durability gap.
    Ok(())
}

fn restore_permissions(
    workspace: &Path,
    relative: &Path,
    record: &ApplyTransactionFile,
) -> Result<()> {
    let Some(readonly) = record.readonly else {
        return Ok(());
    };
    validate_target_location(workspace, relative)?;
    let path = workspace.join(relative);
    let mut permissions = fs::metadata(&path)?.permissions();
    permissions.set_readonly(readonly);
    set_unix_mode(&mut permissions, record.unix_mode);
    fs::set_permissions(&path, permissions)
        .with_context(|| format!("failed to restore permissions: {}", path.display()))
}

#[cfg(unix)]
fn unix_mode(metadata: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(metadata.permissions().mode())
}

#[cfg(not(unix))]
fn unix_mode(_metadata: &fs::Metadata) -> Option<u32> {
    None
}

#[cfg(unix)]
fn set_unix_mode(permissions: &mut fs::Permissions, unix_mode: Option<u32>) {
    use std::os::unix::fs::PermissionsExt;
    if let Some(mode) = unix_mode {
        permissions.set_mode(mode);
    }
}

#[cfg(not(unix))]
fn set_unix_mode(_permissions: &mut fs::Permissions, _unix_mode: Option<u32>) {}

fn fingerprint_bytes(bytes: &[u8]) -> String {
    format!("blake3:{}:{}", bytes.len(), blake3::hash(bytes).to_hex())
}

fn fingerprint_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("failed to hash {}", path.display()))?;
    Ok(fingerprint_bytes(&bytes))
}

fn path_to_portable_string(path: &Path) -> Result<String> {
    let value = path.to_str().ok_or_else(|| {
        transaction_error(
            ApplyTransactionErrorKind::Precondition,
            format!("path is not valid Unicode: {}", path.display()),
        )
    })?;
    Ok(value.replace('\\', "/"))
}

fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

fn unix_time_ms() -> Result<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time should be after Unix epoch")?
        .as_millis();
    u64::try_from(millis).context("system timestamp does not fit in u64")
}

fn unix_time_nanos() -> Result<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time should be after Unix epoch")?
        .as_nanos())
}

fn transaction_error(kind: ApplyTransactionErrorKind, message: impl Into<String>) -> anyhow::Error {
    ApplyTransactionError::new(kind, message).into()
}

#[cfg(test)]
mod tests {
    use super::{
        append_apply_log_index, apply_transaction_error_kind, execute_apply_transaction,
        execute_apply_transaction_with_faults, inspect_apply_transaction, list_apply_transactions,
        recover_apply_transaction, rollback_apply_transaction, valid_transition,
        ApplyFaultInjector, ApplyFaultPoint, ApplyGitPolicy, ApplyTransactionErrorKind,
        ApplyTransactionEvent, ApplyTransactionRequest, ApplyTransactionState, FileMutation,
        EVENTS_DIR,
    };
    use anyhow::{bail, Result};
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    static FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct GitFixture {
        root: PathBuf,
    }

    impl GitFixture {
        fn new(files: &[(&str, &[u8])]) -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should work")
                .as_nanos();
            let counter = FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "opticcode-apply-transaction-{}-{stamp}-{counter}",
                std::process::id()
            ));
            fs::create_dir_all(&root).expect("fixture root should be created");
            for (relative, bytes) in files {
                let path = root.join(relative);
                fs::create_dir_all(path.parent().unwrap()).expect("fixture parent should exist");
                fs::write(path, bytes).expect("fixture file should be written");
            }
            run_git(&root, &["init", "--quiet"]);
            run_git(&root, &["add", "--all"]);
            run_git(
                &root,
                &[
                    "-c",
                    "user.name=OpticCode Test",
                    "-c",
                    "user.email=opticcode-test@example.invalid",
                    "commit",
                    "--quiet",
                    "--no-verify",
                    "-m",
                    "fixture",
                ],
            );
            Self { root }
        }

        fn path(&self, relative: &str) -> PathBuf {
            self.root.join(relative)
        }
    }

    impl Drop for GitFixture {
        fn drop(&mut self) {
            let temp_root = std::env::temp_dir();
            if self.root.starts_with(&temp_root) {
                let _ = fs::remove_dir_all(&self.root);
            }
        }
    }

    #[derive(Default)]
    struct ScriptedFaults {
        points: Vec<ApplyFaultPoint>,
    }

    impl ScriptedFaults {
        fn at(points: impl IntoIterator<Item = ApplyFaultPoint>) -> Self {
            Self {
                points: points.into_iter().collect(),
            }
        }
    }

    impl ApplyFaultInjector for ScriptedFaults {
        fn check(&mut self, point: ApplyFaultPoint) -> Result<()> {
            if self.points.first() == Some(&point) {
                self.points.remove(0);
                bail!("injected fault at {point:?}");
            }
            Ok(())
        }
    }

    struct PermissionDeniedFault;

    impl ApplyFaultInjector for PermissionDeniedFault {
        fn check(&mut self, point: ApplyFaultPoint) -> Result<()> {
            if point == ApplyFaultPoint::BeforeFirstTargetWrite {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "simulated permission denied before target write",
                )
                .into());
            }
            Ok(())
        }
    }

    struct BlockingFault {
        reached: Sender<()>,
        release: Receiver<()>,
    }

    impl ApplyFaultInjector for BlockingFault {
        fn check(&mut self, point: ApplyFaultPoint) -> Result<()> {
            if point == ApplyFaultPoint::AfterPrepared {
                self.reached.send(()).expect("test receiver should exist");
                self.release.recv().expect("test release should arrive");
            }
            Ok(())
        }
    }

    struct MutatingFault {
        point: ApplyFaultPoint,
        action: Option<Box<dyn FnOnce() -> Result<()>>>,
    }

    impl MutatingFault {
        fn at(point: ApplyFaultPoint, action: impl FnOnce() -> Result<()> + 'static) -> Self {
            Self {
                point,
                action: Some(Box::new(action)),
            }
        }
    }

    impl ApplyFaultInjector for MutatingFault {
        fn check(&mut self, point: ApplyFaultPoint) -> Result<()> {
            if point == self.point {
                if let Some(action) = self.action.take() {
                    action()?;
                }
            }
            Ok(())
        }
    }

    #[cfg(windows)]
    fn create_directory_link(target: &Path, link: &Path) -> Result<()> {
        let output = Command::new("cmd")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()?;
        if !output.status.success() {
            bail!(
                "failed to create test junction: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }

    #[cfg(unix)]
    fn create_directory_link(target: &Path, link: &Path) -> Result<()> {
        std::os::unix::fs::symlink(target, link)?;
        Ok(())
    }

    #[cfg(windows)]
    fn remove_directory_link(link: &Path) -> Result<()> {
        fs::remove_dir(link)?;
        Ok(())
    }

    #[cfg(unix)]
    fn remove_directory_link(link: &Path) -> Result<()> {
        fs::remove_file(link)?;
        Ok(())
    }

    #[test]
    fn commits_single_file_with_durable_manifest_and_events() {
        let fixture = GitFixture::new(&[("src/Main.java", b"class Main {}\n")]);
        let request = request(
            &fixture,
            vec![FileMutation::replace(
                "src/Main.java",
                b"class Main {}\n".to_vec(),
                b"class Main { int value; }\n".to_vec(),
            )],
        );

        let result = execute_apply_transaction(request).expect("transaction should commit");

        assert!(result.committed());
        assert_eq!(result.final_state, ApplyTransactionState::Committed);
        assert_eq!(result.modified_files, vec!["src/Main.java"]);
        assert_eq!(
            fs::read(fixture.path("src/Main.java")).unwrap(),
            b"class Main { int value; }\n"
        );
        let inspection = inspect_apply_transaction(&fixture.root, &result.transaction_id).unwrap();
        assert!(inspection.valid);
        assert_eq!(
            inspection.final_state,
            Some(ApplyTransactionState::Committed)
        );
        let manifest = inspection.manifest.unwrap();
        assert_eq!(manifest.schema_version, 1);
        assert!(manifest.patch_hash.starts_with("blake3:"));
        assert_eq!(manifest.files.len(), 1);
        assert!(manifest.files[0]
            .before_hash
            .as_deref()
            .is_some_and(|hash| hash.starts_with("blake3:")));
        assert!(manifest.files[0]
            .after_hash
            .as_deref()
            .is_some_and(|hash| hash.starts_with("blake3:")));
        assert_eq!(inspection.events.len(), 5);
    }

    #[test]
    fn commits_and_rolls_back_create_modify_delete_transaction() {
        let fixture = GitFixture::new(&[
            ("modify.txt", b"before\n"),
            ("delete.txt", b"delete me\n"),
            ("nested/.keep", b"keep\n"),
        ]);
        let request = request(
            &fixture,
            vec![
                FileMutation::replace("modify.txt", b"before\n".to_vec(), b"after\n".to_vec()),
                FileMutation::delete("delete.txt", b"delete me\n".to_vec()),
                FileMutation::create("nested/new file.txt", b"created\n".to_vec()),
            ],
        );

        let committed = execute_apply_transaction(request).unwrap();
        assert!(committed.committed());
        assert_eq!(fs::read(fixture.path("modify.txt")).unwrap(), b"after\n");
        assert!(!fixture.path("delete.txt").exists());
        assert_eq!(
            fs::read(fixture.path("nested/new file.txt")).unwrap(),
            b"created\n"
        );

        let rollback =
            rollback_apply_transaction(&fixture.root, &committed.transaction_id).unwrap();
        assert!(rollback.operation_success);
        assert!(rollback.rolled_back());
        assert_eq!(fs::read(fixture.path("modify.txt")).unwrap(), b"before\n");
        assert_eq!(
            fs::read(fixture.path("delete.txt")).unwrap(),
            b"delete me\n"
        );
        assert!(!fixture.path("nested/new file.txt").exists());
        assert_eq!(rollback.git_restored, Some(true));

        let second = rollback_apply_transaction(&fixture.root, &committed.transaction_id).unwrap();
        assert!(second.rolled_back());
        assert!(!second.rollback_attempted);
        assert!(second
            .warnings
            .iter()
            .any(|warning| warning.contains("already rolled back")));
    }

    #[test]
    fn rejects_dirty_repo_by_default_and_preserves_it_when_explicitly_allowed() {
        let fixture = GitFixture::new(&[("target.txt", b"old\n"), ("user.txt", b"clean\n")]);
        fs::write(fixture.path("user.txt"), b"user change\n").unwrap();
        let mutation = FileMutation::replace("target.txt", b"old\n".to_vec(), b"new\n".to_vec());

        let error = execute_apply_transaction(request(&fixture, vec![mutation.clone()]))
            .expect_err("dirty repository should be rejected");
        assert_eq!(
            apply_transaction_error_kind(&error),
            Some(ApplyTransactionErrorKind::Precondition)
        );
        assert_eq!(fs::read(fixture.path("target.txt")).unwrap(), b"old\n");

        let mut faults = ScriptedFaults::at([ApplyFaultPoint::AfterAllTargetWrites]);
        let allowed = request(&fixture, vec![mutation]).with_git_policy(ApplyGitPolicy::AllowDirty);
        let result = execute_apply_transaction_with_faults(allowed, &mut faults).unwrap();

        assert!(result.rolled_back());
        assert_eq!(result.git_restored, Some(true));
        assert_eq!(fs::read(fixture.path("target.txt")).unwrap(), b"old\n");
        assert_eq!(
            fs::read(fixture.path("user.txt")).unwrap(),
            b"user change\n"
        );
    }

    #[test]
    fn dirty_target_content_is_the_exact_rollback_baseline() {
        let fixture = GitFixture::new(&[("target.txt", b"committed\n")]);
        fs::write(fixture.path("target.txt"), b"user draft\r\n").unwrap();
        let request = request(
            &fixture,
            vec![FileMutation::replace(
                "target.txt",
                b"user draft\r\n".to_vec(),
                b"opticcode result\r\n".to_vec(),
            )],
        )
        .with_git_policy(ApplyGitPolicy::AllowDirty);

        let committed = execute_apply_transaction(request).unwrap();
        assert!(committed.committed());
        let rollback =
            rollback_apply_transaction(&fixture.root, &committed.transaction_id).unwrap();

        assert!(rollback.rolled_back());
        assert_eq!(rollback.git_restored, Some(true));
        assert_eq!(
            fs::read(fixture.path("target.txt")).unwrap(),
            b"user draft\r\n"
        );
    }

    #[test]
    fn binary_contents_are_restored_byte_for_byte() {
        let before = b"\0\xFFbefore\r\n\0".to_vec();
        let after = b"\0\xFEafter\n\0".to_vec();
        let fixture = GitFixture::new(&[("data.bin", before.as_slice())]);
        let committed = execute_apply_transaction(request(
            &fixture,
            vec![FileMutation::replace(
                "data.bin",
                before.clone(),
                after.clone(),
            )],
        ))
        .unwrap();

        assert_eq!(fs::read(fixture.path("data.bin")).unwrap(), after);
        let rollback =
            rollback_apply_transaction(&fixture.root, &committed.transaction_id).unwrap();
        assert!(rollback.rolled_back());
        assert_eq!(fs::read(fixture.path("data.bin")).unwrap(), before);
    }

    #[test]
    fn collision_and_expected_content_mismatch_fail_before_target_write() {
        let fixture = GitFixture::new(&[("file.txt", b"actual\n")]);
        let collision_id = "apply-fixed-collision";
        fs::create_dir_all(fixture.root.join(".opticcode/runs").join(collision_id)).unwrap();
        let collision = request(
            &fixture,
            vec![FileMutation::replace(
                "file.txt",
                b"actual\n".to_vec(),
                b"new\n".to_vec(),
            )],
        )
        .with_transaction_id(collision_id);
        let error = execute_apply_transaction(collision).unwrap_err();
        assert_eq!(
            apply_transaction_error_kind(&error),
            Some(ApplyTransactionErrorKind::Collision)
        );

        let mismatch = request(
            &fixture,
            vec![FileMutation::replace(
                "file.txt",
                b"stale\n".to_vec(),
                b"new\n".to_vec(),
            )],
        );
        let error = execute_apply_transaction(mismatch).unwrap_err();
        assert_eq!(
            apply_transaction_error_kind(&error),
            Some(ApplyTransactionErrorKind::Precondition)
        );
        assert_eq!(fs::read(fixture.path("file.txt")).unwrap(), b"actual\n");
    }

    #[test]
    fn rejects_absolute_parent_reserved_and_windows_prefixed_paths() {
        let fixture = GitFixture::new(&[("file.txt", b"before\n")]);
        let mutations = vec![
            FileMutation::create("../escape.txt", b"escape\n".to_vec()),
            FileMutation::create(".opticcode/evil.txt", b"reserved\n".to_vec()),
            FileMutation::replace(
                fixture.path("file.txt"),
                b"before\n".to_vec(),
                b"after\n".to_vec(),
            ),
        ];
        #[cfg(windows)]
        let mutations = {
            let mut mutations = mutations;
            mutations.push(FileMutation::create(
                r"C:drive-relative.txt",
                b"prefixed\n".to_vec(),
            ));
            mutations.push(FileMutation::create(
                r"\\server\share\outside.txt",
                b"unc\n".to_vec(),
            ));
            mutations
        };

        for mutation in mutations {
            let error = execute_apply_transaction(request(&fixture, vec![mutation]))
                .expect_err("unsafe transaction path should be rejected");
            assert_eq!(
                apply_transaction_error_kind(&error),
                Some(ApplyTransactionErrorKind::Precondition)
            );
        }
        assert_eq!(fs::read(fixture.path("file.txt")).unwrap(), b"before\n");
    }

    #[test]
    fn rejects_linked_parent_components() {
        let fixture = GitFixture::new(&[("real/file.txt", b"before\n")]);
        let link = fixture.path("linked");
        create_directory_link(&fixture.path("real"), &link).expect("test link should be created");

        let error = execute_apply_transaction(request(
            &fixture,
            vec![FileMutation::replace(
                "linked/file.txt",
                b"before\n".to_vec(),
                b"after\n".to_vec(),
            )],
        ))
        .expect_err("linked parent must be rejected");

        assert_eq!(
            apply_transaction_error_kind(&error),
            Some(ApplyTransactionErrorKind::Precondition)
        );
        assert!(format!("{error:#}").contains("symlink or reparse point"));
        assert_eq!(
            fs::read(fixture.path("real/file.txt")).unwrap(),
            b"before\n"
        );
        remove_directory_link(&link).expect("test link should be removed");
    }

    #[test]
    fn workspace_lock_rejects_a_concurrent_transaction_and_releases_cleanly() {
        let fixture = GitFixture::new(&[("file.txt", b"before\n")]);
        let first_request = request(
            &fixture,
            vec![FileMutation::replace(
                "file.txt",
                b"before\n".to_vec(),
                b"first\n".to_vec(),
            )],
        );
        let second_request = request(
            &fixture,
            vec![FileMutation::replace(
                "file.txt",
                b"before\n".to_vec(),
                b"second\n".to_vec(),
            )],
        );
        let (reached_tx, reached_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut faults = BlockingFault {
                reached: reached_tx,
                release: release_rx,
            };
            execute_apply_transaction_with_faults(first_request, &mut faults)
        });

        reached_rx
            .recv()
            .expect("first transaction should reach prepared state");
        let concurrent = execute_apply_transaction(second_request)
            .expect_err("second transaction should not acquire workspace lock");
        assert_eq!(
            apply_transaction_error_kind(&concurrent),
            Some(ApplyTransactionErrorKind::Precondition)
        );
        assert!(format!("{concurrent:#}").contains("active apply or recovery"));

        release_tx
            .send(())
            .expect("first transaction should be released");
        let committed = worker
            .join()
            .expect("transaction worker should not panic")
            .expect("first transaction should complete");
        assert!(committed.committed());
        assert_eq!(fs::read(fixture.path("file.txt")).unwrap(), b"first\n");

        let rollback = rollback_apply_transaction(&fixture.root, &committed.transaction_id)
            .expect("released persistent lock file should be reusable");
        assert!(rollback.rolled_back());
    }

    #[test]
    fn parent_swapped_to_link_is_never_written_and_can_be_recovered() {
        let fixture = GitFixture::new(&[("safe/file.txt", b"before\n")]);
        let outside = fixture.root.parent().unwrap().join(format!(
            "{}-outside",
            fixture.root.file_name().unwrap().to_string_lossy()
        ));
        fs::create_dir(&outside).expect("outside test directory should be created");
        fs::write(outside.join("file.txt"), b"before\n")
            .expect("outside test file should be written");
        let safe = fixture.path("safe");
        let parked = fixture.path("safe-original");
        let action_safe = safe.clone();
        let action_parked = parked.clone();
        let action_outside = outside.clone();
        let mut faults = MutatingFault::at(ApplyFaultPoint::AfterPrepared, move || {
            fs::rename(&action_safe, &action_parked)?;
            create_directory_link(&action_outside, &action_safe)?;
            Ok(())
        });

        let result = execute_apply_transaction_with_faults(
            request(
                &fixture,
                vec![FileMutation::replace(
                    "safe/file.txt",
                    b"before\n".to_vec(),
                    b"after\n".to_vec(),
                )],
            ),
            &mut faults,
        )
        .expect("path swap should produce an explicit rollback result");

        assert!(result.rollback_failed());
        assert_eq!(fs::read(outside.join("file.txt")).unwrap(), b"before\n");
        assert_eq!(fs::read(parked.join("file.txt")).unwrap(), b"before\n");
        let inspection = inspect_apply_transaction(&fixture.root, &result.transaction_id).unwrap();
        assert!(inspection.valid);
        assert!(inspection.recoverable);

        remove_directory_link(&safe).expect("swapped link should be removed");
        fs::rename(&parked, &safe).expect("original directory should be restored");
        let recovered = recover_apply_transaction(&fixture.root, &result.transaction_id).unwrap();
        assert!(recovered.rolled_back());
        assert_eq!(fs::read(safe.join("file.txt")).unwrap(), b"before\n");
        fs::remove_dir_all(&outside).expect("outside test directory should be removed");
    }

    #[test]
    fn preparation_fault_leaves_target_unchanged_and_detectable_invalid_run() {
        let fixture = GitFixture::new(&[("file.txt", b"before\n")]);
        let transaction_id = "apply-preparation-fault";
        let request = request(
            &fixture,
            vec![FileMutation::replace(
                "file.txt",
                b"before\n".to_vec(),
                b"after\n".to_vec(),
            )],
        )
        .with_transaction_id(transaction_id);
        let mut faults = ScriptedFaults::at([ApplyFaultPoint::AfterFirstBackup]);

        let error = execute_apply_transaction_with_faults(request, &mut faults).unwrap_err();

        assert!(error.to_string().contains("AfterFirstBackup"));
        assert_eq!(fs::read(fixture.path("file.txt")).unwrap(), b"before\n");
        let inspection = inspect_apply_transaction(&fixture.root, transaction_id).unwrap();
        assert!(!inspection.valid);
        assert!(!inspection.legacy);
        assert!(inspection
            .errors
            .iter()
            .any(|error| error.contains("manifest")));
    }

    #[test]
    fn faults_before_or_after_target_writes_roll_back_exactly() {
        let points = [
            ApplyFaultPoint::AfterPrepared,
            ApplyFaultPoint::BeforeFirstTargetWrite,
            ApplyFaultPoint::AfterFirstTargetWrite,
            ApplyFaultPoint::AfterAllTargetWrites,
            ApplyFaultPoint::BeforeFinalization,
            ApplyFaultPoint::DuringFinalization,
        ];

        for point in points {
            let fixture = GitFixture::new(&[("file.txt", b"before\r\n")]);
            let mut faults = ScriptedFaults::at([point]);
            let result = execute_apply_transaction_with_faults(
                request(
                    &fixture,
                    vec![FileMutation::replace(
                        "file.txt",
                        b"before\r\n".to_vec(),
                        b"after\r\n".to_vec(),
                    )],
                ),
                &mut faults,
            )
            .unwrap();

            assert!(result.rolled_back(), "fault {point:?} should roll back");
            assert!(!result.operation_success);
            assert_eq!(fs::read(fixture.path("file.txt")).unwrap(), b"before\r\n");
            assert_eq!(result.git_restored, Some(true));
            assert!(result
                .errors
                .iter()
                .any(|error| error.contains(&format!("{point:?}"))));
            let inspection =
                inspect_apply_transaction(&fixture.root, &result.transaction_id).unwrap();
            assert!(inspection.valid, "fault {point:?} journal should be valid");
            assert_eq!(
                inspection.final_state,
                Some(ApplyTransactionState::RolledBack),
                "fault {point:?} should persist rolled_back"
            );
            let manifest = inspection
                .manifest
                .expect("manifest should remain readable");
            assert!(manifest.files[0]
                .before_hash
                .as_deref()
                .is_some_and(|hash| hash.starts_with("blake3:")));
        }
    }

    #[test]
    fn simulated_permission_error_leaves_the_target_unchanged() {
        let fixture = GitFixture::new(&[("file.txt", b"before\n")]);
        let result = execute_apply_transaction_with_faults(
            request(
                &fixture,
                vec![FileMutation::replace(
                    "file.txt",
                    b"before\n".to_vec(),
                    b"after\n".to_vec(),
                )],
            ),
            &mut PermissionDeniedFault,
        )
        .expect("permission failure should produce a rolled-back result");

        assert!(result.rolled_back());
        assert!(!result.operation_success);
        assert_eq!(fs::read(fixture.path("file.txt")).unwrap(), b"before\n");
        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("simulated permission denied")));
    }

    #[cfg(windows)]
    #[test]
    fn locked_windows_target_fails_without_losing_content() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

        let fixture = GitFixture::new(&[("file.txt", b"before\n")]);
        let locked = fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(fixture.path("file.txt"))
            .expect("target should be opened without delete sharing");

        let result = execute_apply_transaction(request(
            &fixture,
            vec![FileMutation::replace(
                "file.txt",
                b"before\n".to_vec(),
                b"after\n".to_vec(),
            )],
        ))
        .expect("locked replacement should return a rollback result");

        assert!(result.rolled_back());
        assert!(!result.operation_success);
        assert_eq!(fs::read(fixture.path("file.txt")).unwrap(), b"before\n");
        let inspection = inspect_apply_transaction(&fixture.root, &result.transaction_id).unwrap();
        assert!(inspection.valid);
        assert_eq!(
            inspection.final_state,
            Some(ApplyTransactionState::RolledBack)
        );
        drop(locked);
    }

    #[test]
    fn rollback_start_failure_is_explicit_then_recoverable() {
        let fixture = GitFixture::new(&[("file.txt", b"before\n")]);
        let mut faults = ScriptedFaults::at([
            ApplyFaultPoint::AfterAllTargetWrites,
            ApplyFaultPoint::RollbackStarted,
        ]);
        let result = execute_apply_transaction_with_faults(
            request(
                &fixture,
                vec![FileMutation::replace(
                    "file.txt",
                    b"before\n".to_vec(),
                    b"after\n".to_vec(),
                )],
            ),
            &mut faults,
        )
        .unwrap();

        assert!(result.rollback_failed());
        assert_eq!(result.rollback_success, Some(false));
        assert_eq!(fs::read(fixture.path("file.txt")).unwrap(), b"after\n");
        let inspection = inspect_apply_transaction(&fixture.root, &result.transaction_id).unwrap();
        assert!(inspection.valid);
        assert!(inspection.recoverable);

        let recovered = recover_apply_transaction(&fixture.root, &result.transaction_id).unwrap();
        assert!(recovered.rolled_back());
        assert_eq!(fs::read(fixture.path("file.txt")).unwrap(), b"before\n");
        assert_eq!(recovered.git_restored, Some(true));
    }

    #[test]
    fn rollback_preserves_external_content_drift_then_recovers() {
        let fixture = GitFixture::new(&[("file.txt", b"before\n")]);
        let committed = execute_apply_transaction(request(
            &fixture,
            vec![FileMutation::replace(
                "file.txt",
                b"before\n".to_vec(),
                b"after\n".to_vec(),
            )],
        ))
        .unwrap();
        fs::write(fixture.path("file.txt"), b"external user edit\n").unwrap();

        let failed = rollback_apply_transaction(&fixture.root, &committed.transaction_id).unwrap();

        assert!(failed.rollback_failed());
        assert_eq!(
            fs::read(fixture.path("file.txt")).unwrap(),
            b"external user edit\n"
        );
        assert!(failed
            .errors
            .iter()
            .any(|error| error.contains("changed outside the transaction")));

        fs::write(fixture.path("file.txt"), b"after\n").unwrap();
        let recovered =
            recover_apply_transaction(&fixture.root, &committed.transaction_id).unwrap();

        assert!(recovered.rolled_back());
        assert_eq!(fs::read(fixture.path("file.txt")).unwrap(), b"before\n");
    }

    #[test]
    fn second_undo_refuses_post_rollback_drift_without_overwriting_it() {
        let fixture = GitFixture::new(&[("file.txt", b"before\n")]);
        let committed = execute_apply_transaction(request(
            &fixture,
            vec![FileMutation::replace(
                "file.txt",
                b"before\n".to_vec(),
                b"after\n".to_vec(),
            )],
        ))
        .unwrap();
        let rollback =
            rollback_apply_transaction(&fixture.root, &committed.transaction_id).unwrap();
        assert!(rollback.rolled_back());

        fs::write(fixture.path("file.txt"), b"new external work\n").unwrap();
        let second = rollback_apply_transaction(&fixture.root, &committed.transaction_id)
            .expect_err("second undo must not claim success after external drift");

        assert_eq!(
            apply_transaction_error_kind(&second),
            Some(ApplyTransactionErrorKind::Precondition)
        );
        assert_eq!(
            fs::read(fixture.path("file.txt")).unwrap(),
            b"new external work\n"
        );
    }

    #[test]
    fn external_change_during_finalization_is_not_committed_or_overwritten() {
        let fixture = GitFixture::new(&[("file.txt", b"before\n")]);
        let target = fixture.path("file.txt");
        let action_target = target.clone();
        let mut faults = MutatingFault::at(ApplyFaultPoint::DuringFinalization, move || {
            fs::write(action_target, b"external edit\n")?;
            Ok(())
        });

        let result = execute_apply_transaction_with_faults(
            request(
                &fixture,
                vec![FileMutation::replace(
                    "file.txt",
                    b"before\n".to_vec(),
                    b"after\n".to_vec(),
                )],
            ),
            &mut faults,
        )
        .unwrap();

        assert!(result.rollback_failed());
        assert!(!result.operation_success);
        assert_eq!(fs::read(&target).unwrap(), b"external edit\n");
        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("changed before finalization")));
        let inspection = inspect_apply_transaction(&fixture.root, &result.transaction_id).unwrap();
        assert!(inspection.valid);
        assert!(inspection.recoverable);

        fs::write(&target, b"after\n").unwrap();
        let recovered = recover_apply_transaction(&fixture.root, &result.transaction_id).unwrap();
        assert!(recovered.rolled_back());
        assert_eq!(fs::read(target).unwrap(), b"before\n");
    }

    #[test]
    fn external_change_after_temp_staging_blocks_publication() {
        let fixture = GitFixture::new(&[("file.txt", b"before\n")]);
        let target = fixture.path("file.txt");
        let action_target = target.clone();
        let mut faults = MutatingFault::at(ApplyFaultPoint::AfterTargetStaged, move || {
            fs::write(action_target, b"external during staging\n")?;
            Ok(())
        });

        let result = execute_apply_transaction_with_faults(
            request(
                &fixture,
                vec![FileMutation::replace(
                    "file.txt",
                    b"before\n".to_vec(),
                    b"after\n".to_vec(),
                )],
            ),
            &mut faults,
        )
        .unwrap();

        assert!(result.rollback_failed());
        assert_eq!(fs::read(&target).unwrap(), b"external during staging\n");
        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("after staging temporary content")));
        assert!(!fs::read_dir(&fixture.root).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".opticcode-target-")
        }));

        fs::write(&target, b"before\n").unwrap();
        let recovered = recover_apply_transaction(&fixture.root, &result.transaction_id).unwrap();
        assert!(recovered.rolled_back());
        assert_eq!(fs::read(target).unwrap(), b"before\n");
    }

    #[test]
    fn partial_rollback_resumes_idempotently() {
        let fixture = GitFixture::new(&[("one.txt", b"one\n"), ("two.txt", b"two\n")]);
        let mut faults = ScriptedFaults::at([
            ApplyFaultPoint::AfterAllTargetWrites,
            ApplyFaultPoint::AfterFirstRestore,
        ]);
        let result = execute_apply_transaction_with_faults(
            request(
                &fixture,
                vec![
                    FileMutation::replace("one.txt", b"one\n".to_vec(), b"ONE\n".to_vec()),
                    FileMutation::replace("two.txt", b"two\n".to_vec(), b"TWO\n".to_vec()),
                ],
            ),
            &mut faults,
        )
        .unwrap();

        assert!(result.rollback_failed());
        let current = [
            fs::read(fixture.path("one.txt")).unwrap(),
            fs::read(fixture.path("two.txt")).unwrap(),
        ];
        assert!(current.contains(&b"ONE\n".to_vec()));
        assert!(current.contains(&b"two\n".to_vec()));

        let recovered = recover_apply_transaction(&fixture.root, &result.transaction_id).unwrap();
        assert!(recovered.rolled_back());
        assert_eq!(fs::read(fixture.path("one.txt")).unwrap(), b"one\n");
        assert_eq!(fs::read(fixture.path("two.txt")).unwrap(), b"two\n");
    }

    #[test]
    fn unicode_spaces_and_line_endings_are_restored_byte_for_byte() {
        let fixture = GitFixture::new(&[
            ("données avec espace.txt", b"ligne un\nligne deux\n"),
            ("windows.txt", b"line one\r\nline two\r\n"),
        ]);
        let request = request(
            &fixture,
            vec![
                FileMutation::replace(
                    "données avec espace.txt",
                    b"ligne un\nligne deux\n".to_vec(),
                    b"ligne modifiee\nligne deux\n".to_vec(),
                ),
                FileMutation::replace(
                    "windows.txt",
                    b"line one\r\nline two\r\n".to_vec(),
                    b"changed\r\nline two\r\n".to_vec(),
                ),
            ],
        );

        let committed = execute_apply_transaction(request).unwrap();
        let rollback =
            rollback_apply_transaction(&fixture.root, &committed.transaction_id).unwrap();

        assert!(rollback.rolled_back());
        assert_eq!(
            fs::read(fixture.path("données avec espace.txt")).unwrap(),
            b"ligne un\nligne deux\n"
        );
        assert_eq!(
            fs::read(fixture.path("windows.txt")).unwrap(),
            b"line one\r\nline two\r\n"
        );
    }

    #[test]
    fn truncated_event_is_reported_and_not_recoverable() {
        let fixture = GitFixture::new(&[("file.txt", b"before\n")]);
        let committed = execute_apply_transaction(request(
            &fixture,
            vec![FileMutation::replace(
                "file.txt",
                b"before\n".to_vec(),
                b"after\n".to_vec(),
            )],
        ))
        .unwrap();
        let events = fixture
            .root
            .join(".opticcode/runs")
            .join(&committed.transaction_id)
            .join("events");
        fs::write(events.join("99999999-truncated.json"), b"{").unwrap();

        let inspection =
            inspect_apply_transaction(&fixture.root, &committed.transaction_id).unwrap();

        assert!(!inspection.valid);
        assert!(!inspection.recoverable);
        assert!(inspection
            .errors
            .iter()
            .any(|error| error.contains("truncated")));
        let recovery = recover_apply_transaction(&fixture.root, &committed.transaction_id)
            .expect_err("invalid journal must refuse recovery");
        assert_eq!(
            apply_transaction_error_kind(&recovery),
            Some(ApplyTransactionErrorKind::InvalidTransaction)
        );
    }

    #[test]
    fn duplicated_reordered_and_contradictory_events_are_refused() {
        {
            let fixture = GitFixture::new(&[("file.txt", b"before\n")]);
            let transaction_id = commit_single_file(&fixture);
            let events = events_dir(&fixture, &transaction_id);
            fs::copy(
                events.join("00000000-prepared.json"),
                events.join("00000000-prepared-duplicate.json"),
            )
            .unwrap();
            assert_invalid_journal(&fixture, &transaction_id, "sequence");
        }

        {
            let fixture = GitFixture::new(&[("file.txt", b"before\n")]);
            let transaction_id = commit_single_file(&fixture);
            let events = events_dir(&fixture, &transaction_id);
            fs::rename(
                events.join("00000000-prepared.json"),
                events.join("99999999-prepared.json"),
            )
            .unwrap();
            assert_invalid_journal(&fixture, &transaction_id, "filename");
        }

        {
            let fixture = GitFixture::new(&[("file.txt", b"before\n")]);
            let transaction_id = commit_single_file(&fixture);
            let event_path = events_dir(&fixture, &transaction_id).join("00000001-applying.json");
            let mut event: ApplyTransactionEvent =
                serde_json::from_slice(&fs::read(&event_path).unwrap()).unwrap();
            event.state = ApplyTransactionState::Committed;
            let mut bytes = serde_json::to_vec_pretty(&event).unwrap();
            bytes.push(b'\n');
            fs::write(&event_path, bytes).unwrap();
            assert_invalid_journal(&fixture, &transaction_id, "transition");
        }
    }

    #[test]
    fn journal_reparse_directories_are_refused_before_recovery_writes() {
        let fixture = GitFixture::new(&[("file.txt", b"before\n")]);
        let transaction_id = commit_single_file(&fixture);
        let runs_dir = fixture.root.join(".opticcode").join("runs");
        let run_dir = runs_dir.join(&transaction_id);
        let parked_run = runs_dir.join(format!("{transaction_id}-parked"));

        fs::rename(&run_dir, &parked_run).unwrap();
        create_directory_link(&parked_run, &run_dir).unwrap();
        let run_inspection = inspect_apply_transaction(&fixture.root, &transaction_id).unwrap();
        assert!(!run_inspection.valid);
        assert!(run_inspection
            .errors
            .iter()
            .any(|error| error.contains("transaction directory")));
        let run_recovery = recover_apply_transaction(&fixture.root, &transaction_id)
            .expect_err("reparse transaction directory must refuse recovery");
        // Windows junctions reach persisted-journal validation, while Unix
        // symlinks may be rejected one layer earlier as a precondition.
        // Both classifications must fail closed before any recovery write.
        assert!(
            matches!(
                apply_transaction_error_kind(&run_recovery),
                Some(
                    ApplyTransactionErrorKind::InvalidTransaction
                        | ApplyTransactionErrorKind::Precondition
                )
            ),
            "unexpected linked transaction-directory recovery error: {run_recovery:#}"
        );
        remove_directory_link(&run_dir).unwrap();
        fs::rename(&parked_run, &run_dir).unwrap();

        let events = run_dir.join(EVENTS_DIR);
        let parked_events = run_dir.join("events-parked");
        fs::rename(&events, &parked_events).unwrap();
        create_directory_link(&parked_events, &events).unwrap();
        let event_inspection = inspect_apply_transaction(&fixture.root, &transaction_id).unwrap();
        assert!(!event_inspection.valid);
        let event_recovery = recover_apply_transaction(&fixture.root, &transaction_id)
            .expect_err("reparse events directory must refuse recovery");
        assert_eq!(
            apply_transaction_error_kind(&event_recovery),
            Some(ApplyTransactionErrorKind::InvalidTransaction)
        );
        remove_directory_link(&events).unwrap();
        fs::rename(&parked_events, &events).unwrap();

        let rollback = rollback_apply_transaction(&fixture.root, &transaction_id).unwrap();
        assert!(rollback.rolled_back());
    }

    #[test]
    fn compatibility_log_refuses_reparse_destination() {
        let fixture = GitFixture::new(&[("file.txt", b"before\n")]);
        let opticcode = fixture.root.join(".opticcode");
        fs::create_dir(&opticcode).unwrap();
        let outside = fixture.root.parent().unwrap().join(format!(
            "{}-log-outside",
            fixture.root.file_name().unwrap().to_string_lossy()
        ));
        fs::create_dir(&outside).unwrap();
        let log_path = opticcode.join("apply-log.jsonl");
        create_directory_link(&outside, &log_path).unwrap();

        let error = append_apply_log_index(&fixture.root, br#"{"run":"test"}"#)
            .expect_err("reparse apply-log destination must be rejected");

        assert_eq!(
            apply_transaction_error_kind(&error),
            Some(ApplyTransactionErrorKind::Precondition)
        );
        assert!(fs::read_dir(&outside).unwrap().next().is_none());
        remove_directory_link(&log_path).unwrap();
        fs::remove_dir(&outside).unwrap();
    }

    #[test]
    fn corrupted_backup_is_invalid_and_recovery_is_refused() {
        let fixture = GitFixture::new(&[("file.txt", b"before\n")]);
        let committed = execute_apply_transaction(request(
            &fixture,
            vec![FileMutation::replace(
                "file.txt",
                b"before\n".to_vec(),
                b"after\n".to_vec(),
            )],
        ))
        .unwrap();
        let backup = fixture
            .root
            .join(".opticcode/runs")
            .join(&committed.transaction_id)
            .join("backups/00000000.bin");
        fs::write(backup, b"corrupted\n").unwrap();

        let inspection =
            inspect_apply_transaction(&fixture.root, &committed.transaction_id).unwrap();

        assert!(!inspection.valid);
        assert!(!inspection.recoverable);
        assert!(inspection
            .errors
            .iter()
            .any(|error| error.contains("backup BLAKE3")));
        let recovery = recover_apply_transaction(&fixture.root, &committed.transaction_id)
            .expect_err("corrupted backup must refuse recovery");
        assert_eq!(
            apply_transaction_error_kind(&recovery),
            Some(ApplyTransactionErrorKind::InvalidTransaction)
        );
    }

    #[test]
    fn lists_transactions_and_rejects_impossible_transitions() {
        let fixture = GitFixture::new(&[("file.txt", b"before\n")]);
        let committed = execute_apply_transaction(request(
            &fixture,
            vec![FileMutation::replace(
                "file.txt",
                b"before\n".to_vec(),
                b"after\n".to_vec(),
            )],
        ))
        .unwrap();

        let transactions = list_apply_transactions(&fixture.root).unwrap();
        assert_eq!(transactions.len(), 1);
        assert_eq!(transactions[0].transaction_id, committed.transaction_id);
        assert!(!valid_transition(
            Some(ApplyTransactionState::Committed),
            ApplyTransactionState::Applying
        ));
        assert!(!valid_transition(
            Some(ApplyTransactionState::RolledBack),
            ApplyTransactionState::RollbackStarted
        ));
    }

    fn request(fixture: &GitFixture, mutations: Vec<FileMutation>) -> ApplyTransactionRequest {
        ApplyTransactionRequest::new(&fixture.root, b"test patch\n".to_vec(), mutations)
    }

    fn commit_single_file(fixture: &GitFixture) -> String {
        execute_apply_transaction(request(
            fixture,
            vec![FileMutation::replace(
                "file.txt",
                b"before\n".to_vec(),
                b"after\n".to_vec(),
            )],
        ))
        .expect("fixture transaction should commit")
        .transaction_id
    }

    fn events_dir(fixture: &GitFixture, transaction_id: &str) -> PathBuf {
        fixture
            .root
            .join(".opticcode/runs")
            .join(transaction_id)
            .join("events")
    }

    fn assert_invalid_journal(fixture: &GitFixture, transaction_id: &str, expected: &str) {
        let inspection = inspect_apply_transaction(&fixture.root, transaction_id).unwrap();
        assert!(!inspection.valid);
        assert!(!inspection.recoverable);
        assert!(
            inspection
                .errors
                .iter()
                .any(|error| error.contains(expected)),
            "missing `{expected}` in {:?}",
            inspection.errors
        );
        let recovery = recover_apply_transaction(&fixture.root, transaction_id)
            .expect_err("invalid journal must refuse recovery");
        assert_eq!(
            apply_transaction_error_kind(&recovery),
            Some(ApplyTransactionErrorKind::InvalidTransaction)
        );
    }

    fn run_git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(["-c", "core.autocrlf=false", "-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(root)
            .output()
            .expect("Git command should start");
        assert!(
            output.status.success(),
            "Git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
