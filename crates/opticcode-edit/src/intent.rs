use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

use crate::{EditPlanLimits, ALLOWED_EDIT_EXTENSIONS, MAX_EDIT_IDENTIFIER_BYTES};

pub const EDIT_INTENT_SCHEMA_VERSION: u32 = 1;
pub const MAX_EDIT_INTENT_TARGETS: usize = 16;
pub const MAX_EDIT_INTENT_TASK_BYTES: usize = 64 * 1024;
pub const MAX_EDIT_INTENT_JSON_BYTES: usize = 512 * 1024;
pub const MAX_EDIT_INTENT_REFERENCES_PER_TARGET: usize = 32;
pub const MAX_EDIT_INTENT_PATH_BYTES: usize = 4 * 1024;
pub const DEFAULT_EDIT_INTENT_TTL_SECONDS: u64 = 15 * 60;
pub const MAX_EDIT_INTENT_AGE_SECONDS: u64 = 5 * 60;
pub const MAX_EDIT_INTENT_CLOCK_SKEW_SECONDS: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditIntentSelectionMode {
    ExplicitReferences,
    ResolvedContext,
    Hybrid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditIntentOperationKind {
    ModifyExisting,
    CreateTextFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditIntentTargetProvenance {
    UserReference,
    ResolvedReference,
    ContextManifest,
    UserRequestedCreation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditOffsetEncoding {
    Utf8Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EditIntentTarget {
    ExistingFile {
        path: String,
        content_hash: String,
        reference_ids: Vec<String>,
        provenance: EditIntentTargetProvenance,
    },
    ProspectiveFile {
        path: String,
        extension: String,
        reference_ids: Vec<String>,
        provenance: EditIntentTargetProvenance,
    },
}

impl EditIntentTarget {
    pub fn path(&self) -> &str {
        match self {
            Self::ExistingFile { path, .. } | Self::ProspectiveFile { path, .. } => path,
        }
    }

    pub const fn operation_kind(&self) -> EditIntentOperationKind {
        match self {
            Self::ExistingFile { .. } => EditIntentOperationKind::ModifyExisting,
            Self::ProspectiveFile { .. } => EditIntentOperationKind::CreateTextFile,
        }
    }

    pub fn reference_ids(&self) -> &[String] {
        match self {
            Self::ExistingFile { reference_ids, .. }
            | Self::ProspectiveFile { reference_ids, .. } => reference_ids,
        }
    }

    fn reference_ids_mut(&mut self) -> &mut Vec<String> {
        match self {
            Self::ExistingFile { reference_ids, .. }
            | Self::ProspectiveFile { reference_ids, .. } => reference_ids,
        }
    }

    pub const fn provenance(&self) -> EditIntentTargetProvenance {
        match self {
            Self::ExistingFile { provenance, .. } | Self::ProspectiveFile { provenance, .. } => {
                *provenance
            }
        }
    }

    const fn sort_rank(&self) -> u8 {
        match self {
            Self::ExistingFile { .. } => 0,
            Self::ProspectiveFile { .. } => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditIntentConstraints {
    pub allowed_operations: Vec<EditIntentOperationKind>,
    pub allowed_extensions: Vec<String>,
    pub limits: EditPlanLimits,
    pub offset_encoding: EditOffsetEncoding,
    pub require_clean_worktree: bool,
    pub require_offline_verification: bool,
    pub require_native_confirmation: bool,
}

impl EditIntentConstraints {
    pub fn modify_only(mut limits: EditPlanLimits) -> Self {
        limits.max_created_files = 0;
        Self {
            allowed_operations: vec![EditIntentOperationKind::ModifyExisting],
            allowed_extensions: ALLOWED_EDIT_EXTENSIONS
                .iter()
                .map(|extension| (*extension).to_string())
                .collect(),
            limits,
            offset_encoding: EditOffsetEncoding::Utf8Bytes,
            require_clean_worktree: true,
            require_offline_verification: true,
            require_native_confirmation: true,
        }
    }

    pub fn modify_and_create(limits: EditPlanLimits) -> Self {
        Self {
            allowed_operations: vec![
                EditIntentOperationKind::ModifyExisting,
                EditIntentOperationKind::CreateTextFile,
            ],
            allowed_extensions: ALLOWED_EDIT_EXTENSIONS
                .iter()
                .map(|extension| (*extension).to_string())
                .collect(),
            limits,
            offset_encoding: EditOffsetEncoding::Utf8Bytes,
            require_clean_worktree: true,
            require_offline_verification: true,
            require_native_confirmation: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditIntent {
    pub schema_version: u32,
    pub intent_id: String,
    pub request_id: String,
    pub workspace_id: String,
    pub workspace_root_hash: String,
    pub base_head: String,
    pub working_tree_digest: String,
    pub task: String,
    pub selection_mode: EditIntentSelectionMode,
    pub targets: Vec<EditIntentTarget>,
    pub constraints: EditIntentConstraints,
    pub created_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditIntentAllowedExistingTarget {
    pub path: String,
    pub content_hash: String,
    pub reference_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditIntentAllowedCreateTarget {
    pub path: String,
    pub reference_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditIntentExpectations {
    pub request_id: String,
    pub workspace_id: String,
    pub workspace_root_hash: String,
    pub base_head: String,
    pub working_tree_digest: String,
    pub now_unix_ms: u64,
    pub limits: EditPlanLimits,
    pub allowed_existing_targets: Vec<EditIntentAllowedExistingTarget>,
    pub allowed_create_targets: Vec<EditIntentAllowedCreateTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedEditIntent {
    pub intent: EditIntent,
    pub intent_hash: String,
    pub canonical_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditIntentError {
    pub code: String,
    pub message: String,
}

impl EditIntentError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for EditIntentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for EditIntentError {}

pub fn parse_edit_intent_json(raw: &str) -> Result<EditIntent, EditIntentError> {
    if raw.is_empty() || raw.len() > MAX_EDIT_INTENT_JSON_BYTES {
        return Err(EditIntentError::new(
            "intent.size",
            "edit intent JSON is empty or exceeds the bounded payload limit",
        ));
    }
    let trimmed = raw.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return Err(EditIntentError::new(
            "intent.trailing_text",
            "edit intent input must contain exactly one JSON object",
        ));
    }
    serde_json::from_str::<EditIntent>(trimmed).map_err(|error| {
        EditIntentError::new(
            "intent.invalid_json",
            format!("edit intent does not match schema v1: {error}"),
        )
    })
}

pub fn validate_edit_intent(
    intent: EditIntent,
    expected: &EditIntentExpectations,
) -> Result<ValidatedEditIntent, EditIntentError> {
    validate_envelope(&intent, expected)?;
    validate_task(&intent.task)?;
    validate_timestamps(&intent, expected.now_unix_ms)?;

    let operations = validate_constraints(&intent.constraints, expected.limits)?;
    let trusted = validate_trusted_targets(expected)?;
    validate_targets(&intent, &operations, &trusted)?;

    let canonical_intent = canonicalize_intent(intent);
    let canonical_json = serde_json::to_string(&canonical_intent).map_err(|error| {
        EditIntentError::new(
            "intent.canonicalization",
            format!("edit intent canonical serialization failed: {error}"),
        )
    })?;
    let intent_hash = blake3::hash(canonical_json.as_bytes()).to_hex().to_string();

    Ok(ValidatedEditIntent {
        intent: canonical_intent,
        intent_hash,
        canonical_json,
    })
}

fn validate_envelope(
    intent: &EditIntent,
    expected: &EditIntentExpectations,
) -> Result<(), EditIntentError> {
    if intent.schema_version != EDIT_INTENT_SCHEMA_VERSION {
        return Err(EditIntentError::new(
            "intent.schema_version",
            format!(
                "unsupported edit intent schema version {}",
                intent.schema_version
            ),
        ));
    }

    validate_identifier("intent_id", &intent.intent_id)?;
    for (name, actual, trusted) in [
        (
            "request_id",
            intent.request_id.as_str(),
            expected.request_id.as_str(),
        ),
        (
            "workspace_id",
            intent.workspace_id.as_str(),
            expected.workspace_id.as_str(),
        ),
        (
            "workspace_root_hash",
            intent.workspace_root_hash.as_str(),
            expected.workspace_root_hash.as_str(),
        ),
        (
            "base_head",
            intent.base_head.as_str(),
            expected.base_head.as_str(),
        ),
        (
            "working_tree_digest",
            intent.working_tree_digest.as_str(),
            expected.working_tree_digest.as_str(),
        ),
    ] {
        validate_identifier(name, actual)?;
        if actual != trusted {
            return Err(EditIntentError::new(
                format!("intent.{name}_mismatch"),
                format!("{name} does not match the trusted request state"),
            ));
        }
    }

    if !is_hash(&intent.workspace_root_hash) || !is_hash(&intent.working_tree_digest) {
        return Err(EditIntentError::new(
            "intent.hash_invalid",
            "workspace_root_hash and working_tree_digest must be 64-character hexadecimal BLAKE3 values",
        ));
    }
    if !is_git_oid(&intent.base_head) {
        return Err(EditIntentError::new(
            "intent.head_invalid",
            "base_head must be a 40- or 64-character hexadecimal Git object ID",
        ));
    }
    Ok(())
}

fn validate_task(task: &str) -> Result<(), EditIntentError> {
    if task.trim().is_empty() || task.len() > MAX_EDIT_INTENT_TASK_BYTES {
        return Err(EditIntentError::new(
            "intent.task",
            "edit intent task is empty or exceeds the 64 KiB byte limit",
        ));
    }
    if task.contains('\0') {
        return Err(EditIntentError::new(
            "intent.task",
            "edit intent task contains a NUL byte",
        ));
    }
    Ok(())
}

fn validate_timestamps(intent: &EditIntent, now_unix_ms: u64) -> Result<(), EditIntentError> {
    let skew_ms = MAX_EDIT_INTENT_CLOCK_SKEW_SECONDS.saturating_mul(1_000);
    let max_age_ms = MAX_EDIT_INTENT_AGE_SECONDS.saturating_mul(1_000);
    let max_ttl_ms = DEFAULT_EDIT_INTENT_TTL_SECONDS.saturating_mul(1_000);

    if intent.created_at_unix_ms > now_unix_ms.saturating_add(skew_ms) {
        return Err(EditIntentError::new(
            "intent.created_at",
            "edit intent creation time is too far in the future",
        ));
    }
    if now_unix_ms > intent.created_at_unix_ms.saturating_add(max_age_ms) {
        return Err(EditIntentError::new(
            "intent.stale",
            "edit intent is older than the allowed validation window",
        ));
    }
    if intent.expires_at_unix_ms <= now_unix_ms
        || intent.expires_at_unix_ms <= intent.created_at_unix_ms
        || intent.expires_at_unix_ms > intent.created_at_unix_ms.saturating_add(max_ttl_ms)
    {
        return Err(EditIntentError::new(
            "intent.expiry",
            "edit intent expiry is invalid or exceeds the 15-minute TTL",
        ));
    }
    Ok(())
}

fn validate_constraints(
    constraints: &EditIntentConstraints,
    expected_limits: EditPlanLimits,
) -> Result<BTreeSet<EditIntentOperationKind>, EditIntentError> {
    if constraints.offset_encoding != EditOffsetEncoding::Utf8Bytes {
        return Err(EditIntentError::new(
            "intent.offset_encoding",
            "edit intent v1 requires utf8_bytes offsets",
        ));
    }
    if !constraints.require_clean_worktree
        || !constraints.require_offline_verification
        || !constraints.require_native_confirmation
    {
        return Err(EditIntentError::new(
            "intent.security_constraints",
            "edit intent v1 requires a clean worktree, offline verification, and native confirmation",
        ));
    }
    if !constraints.limits.bounded_by_hard_maxima()
        || limits_exceed(constraints.limits, expected_limits)
    {
        return Err(EditIntentError::new(
            "intent.limit_escalation",
            "edit intent attempted to raise or invalidate trusted runtime limits",
        ));
    }

    let operations = constraints
        .allowed_operations
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if operations.is_empty() || operations.len() != constraints.allowed_operations.len() {
        return Err(EditIntentError::new(
            "intent.operation_contract",
            "allowed_operations must be non-empty and contain no duplicates",
        ));
    }
    if operations.contains(&EditIntentOperationKind::CreateTextFile) {
        if constraints.limits.max_created_files == 0 {
            return Err(EditIntentError::new(
                "intent.creation_contract",
                "create_text_file is enabled while max_created_files is zero",
            ));
        }
    } else if constraints.limits.max_created_files != 0 {
        return Err(EditIntentError::new(
            "intent.creation_contract",
            "max_created_files must be zero when create_text_file is disabled",
        ));
    }

    if constraints.allowed_extensions.is_empty()
        || constraints.allowed_extensions.len() > ALLOWED_EDIT_EXTENSIONS.len()
    {
        return Err(EditIntentError::new(
            "intent.extension_contract",
            "allowed_extensions is empty or exceeds the runtime extension inventory",
        ));
    }
    let mut extensions = BTreeSet::new();
    for extension in &constraints.allowed_extensions {
        if extension.is_empty()
            || extension != &extension.to_ascii_lowercase()
            || extension.starts_with('.')
            || !ALLOWED_EDIT_EXTENSIONS.contains(&extension.as_str())
            || !extensions.insert(extension.as_str())
        {
            return Err(EditIntentError::new(
                "intent.extension_contract",
                format!("extension `{extension}` is invalid, duplicated, or not allowlisted"),
            ));
        }
    }

    Ok(operations)
}

#[derive(Debug)]
struct TrustedTargets {
    existing: BTreeMap<String, TrustedExistingTarget>,
    prospective: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Debug)]
struct TrustedExistingTarget {
    content_hash: String,
    reference_ids: BTreeSet<String>,
}

fn validate_trusted_targets(
    expected: &EditIntentExpectations,
) -> Result<TrustedTargets, EditIntentError> {
    let mut existing = BTreeMap::new();
    for target in &expected.allowed_existing_targets {
        let path = validate_relative_path(&target.path).map_err(|error| {
            EditIntentError::new(
                "intent.trusted_state_invalid",
                format!("trusted existing target is invalid: {error}"),
            )
        })?;
        if !is_hash(&target.content_hash) {
            return Err(EditIntentError::new(
                "intent.trusted_state_invalid",
                format!("trusted existing target `{path}` has an invalid content hash"),
            ));
        }
        let reference_ids =
            validate_reference_inventory(&target.reference_ids).map_err(|error| {
                EditIntentError::new(
                    "intent.trusted_state_invalid",
                    format!("trusted existing target `{path}` is invalid: {error}"),
                )
            })?;
        if existing
            .insert(
                path.clone(),
                TrustedExistingTarget {
                    content_hash: target.content_hash.clone(),
                    reference_ids,
                },
            )
            .is_some()
        {
            return Err(EditIntentError::new(
                "intent.trusted_state_invalid",
                format!("trusted existing target `{path}` is duplicated"),
            ));
        }
    }

    let mut prospective = BTreeMap::new();
    for target in &expected.allowed_create_targets {
        let path = validate_relative_path(&target.path).map_err(|error| {
            EditIntentError::new(
                "intent.trusted_state_invalid",
                format!("trusted create target is invalid: {error}"),
            )
        })?;
        let reference_ids =
            validate_reference_inventory(&target.reference_ids).map_err(|error| {
                EditIntentError::new(
                    "intent.trusted_state_invalid",
                    format!("trusted create target `{path}` is invalid: {error}"),
                )
            })?;
        if prospective.insert(path.clone(), reference_ids).is_some() {
            return Err(EditIntentError::new(
                "intent.trusted_state_invalid",
                format!("trusted create target `{path}` is duplicated"),
            ));
        }
    }

    Ok(TrustedTargets {
        existing,
        prospective,
    })
}

fn validate_targets(
    intent: &EditIntent,
    operations: &BTreeSet<EditIntentOperationKind>,
    trusted: &TrustedTargets,
) -> Result<(), EditIntentError> {
    if intent.targets.is_empty() || intent.targets.len() > MAX_EDIT_INTENT_TARGETS {
        return Err(EditIntentError::new(
            "intent.target_limit",
            format!("edit intent must contain between 1 and {MAX_EDIT_INTENT_TARGETS} targets"),
        ));
    }

    let allowed_extensions = intent
        .constraints
        .allowed_extensions
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut seen_paths = BTreeSet::new();
    let mut has_existing = false;
    let mut has_prospective = false;
    let mut has_explicit_reference = false;

    for target in &intent.targets {
        let path = validate_relative_path(target.path())?;
        if !seen_paths.insert(path.clone()) {
            return Err(EditIntentError::new(
                "intent.target_duplicate",
                format!("target `{path}` appears more than once"),
            ));
        }
        if !operations.contains(&target.operation_kind()) {
            return Err(EditIntentError::new(
                "intent.target_operation",
                format!(
                    "target `{path}` requires operation `{}` which is not allowed",
                    operation_name(target.operation_kind())
                ),
            ));
        }

        let references = validate_reference_inventory(target.reference_ids())?;
        has_explicit_reference |= !references.is_empty();

        match target {
            EditIntentTarget::ExistingFile {
                content_hash,
                provenance,
                ..
            } => {
                has_existing = true;
                if *provenance == EditIntentTargetProvenance::UserRequestedCreation {
                    return Err(EditIntentError::new(
                        "intent.target_provenance",
                        format!("existing target `{path}` has creation-only provenance"),
                    ));
                }
                if *provenance == EditIntentTargetProvenance::UserReference && references.is_empty()
                {
                    return Err(EditIntentError::new(
                        "intent.target_provenance",
                        format!("user-reference target `{path}` has no reference IDs"),
                    ));
                }
                if !is_hash(content_hash) {
                    return Err(EditIntentError::new(
                        "intent.target_hash",
                        format!("existing target `{path}` has an invalid content hash"),
                    ));
                }
                let Some(allowed) = trusted.existing.get(&path) else {
                    return Err(EditIntentError::new(
                        "intent.target_not_trusted",
                        format!("existing target `{path}` was not resolved by the trusted runtime"),
                    ));
                };
                if content_hash != &allowed.content_hash {
                    return Err(EditIntentError::new(
                        "intent.target_hash_mismatch",
                        format!("existing target `{path}` no longer matches trusted content"),
                    ));
                }
                ensure_reference_subset(&path, &references, &allowed.reference_ids)?;
            }
            EditIntentTarget::ProspectiveFile {
                extension,
                provenance,
                ..
            } => {
                has_prospective = true;
                if *provenance != EditIntentTargetProvenance::UserRequestedCreation {
                    return Err(EditIntentError::new(
                        "intent.target_provenance",
                        format!("prospective target `{path}` requires user_requested_creation"),
                    ));
                }
                let actual_extension = extension_for_path(&path)?;
                if extension != &actual_extension
                    || !allowed_extensions.contains(extension.as_str())
                {
                    return Err(EditIntentError::new(
                        "intent.target_extension",
                        format!(
                            "prospective target `{path}` has an invalid or disallowed extension"
                        ),
                    ));
                }
                let Some(allowed_references) = trusted.prospective.get(&path) else {
                    return Err(EditIntentError::new(
                        "intent.target_not_trusted",
                        format!(
                            "prospective target `{path}` was not explicitly allowed by the trusted runtime"
                        ),
                    ));
                };
                ensure_reference_subset(&path, &references, allowed_references)?;
            }
        }

        let target_extension = extension_for_path(&path)?;
        if !allowed_extensions.contains(target_extension.as_str()) {
            return Err(EditIntentError::new(
                "intent.target_extension",
                format!("target `{path}` uses a disallowed extension"),
            ));
        }
    }

    if operations.contains(&EditIntentOperationKind::ModifyExisting) != has_existing {
        return Err(EditIntentError::new(
            "intent.operation_targets",
            "modify_existing must correspond to at least one existing_file target",
        ));
    }
    if operations.contains(&EditIntentOperationKind::CreateTextFile) != has_prospective {
        return Err(EditIntentError::new(
            "intent.operation_targets",
            "create_text_file must correspond to at least one prospective_file target",
        ));
    }

    match intent.selection_mode {
        EditIntentSelectionMode::ExplicitReferences
            if !intent
                .targets
                .iter()
                .all(|target| !target.reference_ids().is_empty()) =>
        {
            return Err(EditIntentError::new(
                "intent.selection_mode",
                "explicit_references mode requires reference IDs on every target",
            ));
        }
        EditIntentSelectionMode::Hybrid if !has_explicit_reference => {
            return Err(EditIntentError::new(
                "intent.selection_mode",
                "hybrid mode requires at least one explicit reference",
            ));
        }
        _ => {}
    }

    Ok(())
}

fn validate_reference_inventory(
    references: &[String],
) -> Result<BTreeSet<String>, EditIntentError> {
    if references.len() > MAX_EDIT_INTENT_REFERENCES_PER_TARGET {
        return Err(EditIntentError::new(
            "intent.reference_limit",
            format!(
                "target references exceed the per-target limit of {MAX_EDIT_INTENT_REFERENCES_PER_TARGET}"
            ),
        ));
    }
    let mut result = BTreeSet::new();
    for reference_id in references {
        validate_identifier("reference_id", reference_id)?;
        if !result.insert(reference_id.clone()) {
            return Err(EditIntentError::new(
                "intent.reference_duplicate",
                format!("reference ID `{reference_id}` is duplicated"),
            ));
        }
    }
    Ok(result)
}

fn ensure_reference_subset(
    path: &str,
    references: &BTreeSet<String>,
    allowed: &BTreeSet<String>,
) -> Result<(), EditIntentError> {
    if let Some(reference_id) = references.iter().find(|item| !allowed.contains(*item)) {
        return Err(EditIntentError::new(
            "intent.reference_not_trusted",
            format!("target `{path}` uses untrusted reference ID `{reference_id}`"),
        ));
    }
    Ok(())
}

fn canonicalize_intent(mut intent: EditIntent) -> EditIntent {
    for target in &mut intent.targets {
        target.reference_ids_mut().sort();
    }
    intent.targets.sort_by(|left, right| {
        left.path()
            .cmp(right.path())
            .then_with(|| left.sort_rank().cmp(&right.sort_rank()))
    });
    intent.constraints.allowed_operations.sort();
    intent.constraints.allowed_extensions.sort();
    intent
}

fn validate_identifier(name: &str, value: &str) -> Result<(), EditIntentError> {
    if value.is_empty()
        || value.len() > MAX_EDIT_IDENTIFIER_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(EditIntentError::new(
            format!("intent.{name}"),
            format!("{name} is empty, too long, or contains control characters"),
        ));
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<String, EditIntentError> {
    if path.is_empty()
        || path.len() > MAX_EDIT_INTENT_PATH_BYTES
        || path
            .chars()
            .any(|character| matches!(character, '\\' | '\0' | ':'))
        || path.chars().any(char::is_control)
    {
        return Err(EditIntentError::new(
            "intent.path",
            format!("path `{path}` is empty, oversized, non-portable, or contains control data"),
        ));
    }

    let candidate = Path::new(path);
    if candidate.is_absolute() {
        return Err(EditIntentError::new(
            "intent.path",
            format!("path `{path}` must be relative"),
        ));
    }

    let mut parts = Vec::new();
    for component in candidate.components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_str().ok_or_else(|| {
                    EditIntentError::new("intent.path", format!("path `{path}` is not valid UTF-8"))
                })?;
                if part.is_empty()
                    || part.len() > 255
                    || part.ends_with(' ')
                    || part.ends_with('.')
                    || is_reserved_windows_name(part)
                {
                    return Err(EditIntentError::new(
                        "intent.path",
                        format!("path component `{part}` is not portable"),
                    ));
                }
                parts.push(part);
            }
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err(EditIntentError::new(
                    "intent.path",
                    format!("path `{path}` contains a forbidden component"),
                ));
            }
        }
    }

    let normalized = parts.join("/");
    if normalized != path {
        return Err(EditIntentError::new(
            "intent.path",
            format!("path `{path}` is not in canonical workspace-relative form"),
        ));
    }
    let first = parts.first().map(|part| part.to_ascii_lowercase());
    if matches!(first.as_deref(), Some(".git" | ".opticcode")) {
        return Err(EditIntentError::new(
            "intent.path",
            format!("path `{path}` targets protected repository metadata"),
        ));
    }
    Ok(normalized)
}

fn extension_for_path(path: &str) -> Result<String, EditIntentError> {
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| {
            EditIntentError::new(
                "intent.target_extension",
                format!("target `{path}` has no valid extension"),
            )
        })?;
    Ok(extension)
}

fn is_reserved_windows_name(part: &str) -> bool {
    let stem = part
        .split('.')
        .next()
        .unwrap_or(part)
        .trim_end_matches([' ', '.'])
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0')
}

fn limits_exceed(left: EditPlanLimits, right: EditPlanLimits) -> bool {
    left.max_files > right.max_files
        || left.max_created_files > right.max_created_files
        || left.max_file_bytes > right.max_file_bytes
        || left.max_proposal_bytes > right.max_proposal_bytes
        || left.max_hunks > right.max_hunks
        || left.max_added_lines > right.max_added_lines
        || left.max_deleted_lines > right.max_deleted_lines
        || left.max_changed_lines > right.max_changed_lines
        || left.global_timeout_seconds > right.global_timeout_seconds
}

fn operation_name(operation: EditIntentOperationKind) -> &'static str {
    match operation {
        EditIntentOperationKind::ModifyExisting => "modify_existing",
        EditIntentOperationKind::CreateTextFile => "create_text_file",
    }
}

fn is_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_git_oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
