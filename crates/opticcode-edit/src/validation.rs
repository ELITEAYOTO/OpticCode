use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use opticcode_tools::rag::{inspect_sensitive_text, read_safe_workspace_file};

use crate::{
    ByteRange, EditOperation, EditPlan, EditPlanExpectations, EditValidationKind, LineEnding,
    ProposalFileSnapshot, ProposalFileStatus, TextEncoding, ValidatedEditPlan,
    ALLOWED_EDIT_EXTENSIONS, DEFAULT_PROPOSAL_TTL_SECONDS, EDIT_PLAN_SCHEMA_VERSION,
    MAX_EDIT_IDENTIFIER_BYTES, MAX_EDIT_LIST_ITEMS, MAX_EDIT_REASON_CHARS,
};

const MAX_EXPECTED_OLD_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditPlanError {
    pub code: String,
    pub message: String,
}

impl EditPlanError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for EditPlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for EditPlanError {}

pub fn parse_edit_plan_json(raw: &str) -> Result<EditPlan, EditPlanError> {
    if raw.is_empty() || raw.len() > crate::MAX_EDIT_PROPOSAL_BYTES {
        return Err(EditPlanError::new(
            "plan.size",
            "structured edit output is empty or exceeds the proposal byte limit",
        ));
    }
    let trimmed = raw.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return Err(EditPlanError::new(
            "plan.trailing_text",
            "the model output must contain exactly one JSON object and no prose or code fence",
        ));
    }
    serde_json::from_str::<EditPlan>(trimmed).map_err(|error| {
        EditPlanError::new(
            "plan.invalid_json",
            format!("the edit plan does not match schema v1: {error}"),
        )
    })
}

pub fn validate_edit_plan(
    plan: EditPlan,
    expected: &EditPlanExpectations,
) -> Result<ValidatedEditPlan, EditPlanError> {
    validate_envelope(&plan, expected)?;
    validate_bounded_text(&plan.summary, "summary")?;
    validate_bounded_text(&plan.rationale_summary, "rationale_summary")?;
    validate_string_list(&plan.risks, "risks")?;
    validate_string_list(&plan.limitations, "limitations")?;
    if plan.context_used.len() > MAX_EDIT_LIST_ITEMS
        || plan.user_references.len() > MAX_EDIT_LIST_ITEMS
        || plan.validations.len() > MAX_EDIT_LIST_ITEMS
    {
        return Err(EditPlanError::new(
            "plan.list_limit",
            "context, references, validations, risks, or limitations exceed runtime bounds",
        ));
    }
    let validations = plan.validations.iter().copied().collect::<BTreeSet<_>>();
    if validations.len() != plan.validations.len()
        || !validations.contains(&EditValidationKind::ReparseJava)
        || !validations.contains(&EditValidationKind::BuildOffline)
    {
        return Err(EditPlanError::new(
            "plan.validation_contract",
            "every edit plan must request unique reparse_java and build_offline validations",
        ));
    }
    if !plan.limits.bounded_by_hard_maxima() || exceeds_effective_limits(&plan, expected) {
        return Err(EditPlanError::new(
            "plan.limit_escalation",
            "the plan attempted to raise or invalidate a trusted runtime limit",
        ));
    }
    if plan.operations.is_empty() || plan.operations.len() > plan.limits.max_hunks {
        return Err(EditPlanError::new(
            "plan.operation_limit",
            "the plan must contain a bounded non-empty operation list",
        ));
    }

    let root = canonical_real_directory(&expected.workspace_root)?;
    let mut grouped: BTreeMap<String, Vec<&EditOperation>> = BTreeMap::new();
    for operation in &plan.operations {
        validate_relative_path(operation.path())?;
        validate_extension(operation.path())?;
        grouped
            .entry(normalize_relative(operation.path()))
            .or_default()
            .push(operation);
    }
    if grouped.len() > plan.limits.max_files {
        return Err(EditPlanError::new(
            "plan.file_limit",
            format!(
                "proposal touches {} files; maximum is {}",
                grouped.len(),
                plan.limits.max_files
            ),
        ));
    }
    let creations = plan
        .operations
        .iter()
        .filter(|item| item.is_creation())
        .count();
    if creations > plan.limits.max_created_files {
        return Err(EditPlanError::new(
            "plan.creation_limit",
            format!(
                "proposal creates {creations} files; maximum is {}",
                plan.limits.max_created_files
            ),
        ));
    }

    let mut snapshots = Vec::with_capacity(grouped.len());
    let mut total_snapshot_bytes = 0usize;
    let mut estimated_added_lines = 0usize;
    let mut estimated_deleted_lines = 0usize;
    for (path, operations) in grouped {
        let snapshot = if operations.iter().any(|operation| operation.is_creation()) {
            validate_creation_group(&root, &path, &operations, &plan)?
        } else {
            validate_modification_group(&root, &path, &operations, &plan)?
        };
        let (added, deleted) = estimate_group_lines(&operations);
        estimated_added_lines = estimated_added_lines.saturating_add(added);
        estimated_deleted_lines = estimated_deleted_lines.saturating_add(deleted);
        total_snapshot_bytes = total_snapshot_bytes
            .saturating_add(snapshot.base_content.as_ref().map_or(0, String::len))
            .saturating_add(snapshot.proposed_content.len());
        snapshots.push(snapshot);
    }
    if estimated_added_lines > plan.limits.max_added_lines
        || estimated_deleted_lines > plan.limits.max_deleted_lines
        || estimated_added_lines.saturating_add(estimated_deleted_lines)
            > plan.limits.max_changed_lines
    {
        return Err(EditPlanError::new(
            "plan.line_limit",
            format!(
                "proposal line estimate is +{estimated_added_lines}/-{estimated_deleted_lines} and exceeds configured bounds"
            ),
        ));
    }
    if total_snapshot_bytes > plan.limits.max_proposal_bytes.saturating_mul(2) {
        return Err(EditPlanError::new(
            "plan.snapshot_limit",
            "base and proposed review snapshots exceed the bounded storage limit",
        ));
    }

    Ok(ValidatedEditPlan {
        plan,
        files: snapshots,
        estimated_added_lines,
        estimated_deleted_lines,
        total_snapshot_bytes,
    })
}

fn validate_envelope(
    plan: &EditPlan,
    expected: &EditPlanExpectations,
) -> Result<(), EditPlanError> {
    if plan.schema_version != EDIT_PLAN_SCHEMA_VERSION {
        return Err(EditPlanError::new(
            "plan.schema_version",
            format!(
                "unsupported edit plan schema version {}",
                plan.schema_version
            ),
        ));
    }
    let identifiers = [
        ("plan_id", plan.plan_id.as_str(), expected.plan_id.as_str()),
        (
            "request_id",
            plan.request_id.as_str(),
            expected.request_id.as_str(),
        ),
        (
            "workspace_id",
            plan.workspace_id.as_str(),
            expected.workspace_id.as_str(),
        ),
        ("profile", plan.profile.as_str(), expected.profile.as_str()),
        ("model", plan.model.as_str(), expected.model.as_str()),
        (
            "base_head",
            plan.base_head.as_str(),
            expected.base_head.as_str(),
        ),
        (
            "working_tree_digest",
            plan.working_tree_digest.as_str(),
            expected.working_tree_digest.as_str(),
        ),
        (
            "workspace_root_hash",
            plan.workspace_root_hash.as_str(),
            expected.workspace_root_hash.as_str(),
        ),
    ];
    for (name, actual, trusted) in identifiers {
        if actual.is_empty()
            || actual.len() > MAX_EDIT_IDENTIFIER_BYTES
            || actual.contains(['\r', '\n', '\0'])
            || actual != trusted
        {
            return Err(EditPlanError::new(
                format!("plan.{name}_mismatch"),
                format!("{name} does not match the trusted request state"),
            ));
        }
    }
    if plan.provider != expected.provider {
        return Err(EditPlanError::new(
            "plan.provider_mismatch",
            "provider does not match the trusted request",
        ));
    }
    if !is_hash(&plan.workspace_root_hash) || !is_hash(&plan.working_tree_digest) {
        return Err(EditPlanError::new(
            "plan.hash_invalid",
            "workspace and working-tree digests must be 64 hexadecimal BLAKE3 values",
        ));
    }
    if !is_git_oid(&plan.base_head) {
        return Err(EditPlanError::new(
            "plan.head_invalid",
            "base HEAD must be a 40- or 64-character hexadecimal object ID",
        ));
    }
    let max_expiry = expected
        .now_unix_ms
        .saturating_add(DEFAULT_PROPOSAL_TTL_SECONDS.saturating_mul(1_000));
    if plan.expires_at_unix_ms <= expected.now_unix_ms || plan.expires_at_unix_ms > max_expiry {
        return Err(EditPlanError::new(
            "plan.expiry_invalid",
            "proposal expiry must be in the future and no more than 24 hours away",
        ));
    }
    Ok(())
}

fn exceeds_effective_limits(plan: &EditPlan, expected: &EditPlanExpectations) -> bool {
    let left = plan.limits;
    let right = expected.limits;
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

fn validate_modification_group(
    root: &Path,
    path: &str,
    operations: &[&EditOperation],
    plan: &EditPlan,
) -> Result<ProposalFileSnapshot, EditPlanError> {
    let file = read_safe_workspace_file(root, Path::new(path), plan.limits.max_file_bytes as u64)
        .map_err(|error| EditPlanError::new(error.rule_id, error.message))?;
    let base = file.content;
    if inspect_sensitive_text(&base).is_some() {
        return Err(EditPlanError::new(
            "secret.source_content",
            format!("source file {path} contains material excluded by secret scanning"),
        ));
    }
    let actual_hash = content_hash(base.as_bytes());
    let actual_line_ending = detect_line_ending(&base)?;
    let mut ranges = Vec::with_capacity(operations.len());
    for operation in operations {
        let EditOperation::Modify {
            expected_file_hash,
            encoding,
            line_ending,
            range,
            expected_old,
            replacement,
            reason,
            provenance,
            symbol,
            ..
        } = operation
        else {
            return Err(EditPlanError::new(
                "plan.mixed_operations",
                "a path cannot be both created and modified in one proposal",
            ));
        };
        if *encoding != TextEncoding::Utf8 || *line_ending != actual_line_ending {
            return Err(EditPlanError::new(
                "file.encoding_or_line_ending",
                format!("encoding or line ending expectation does not match {path}"),
            ));
        }
        if expected_file_hash != &actual_hash || !is_hash(expected_file_hash) {
            return Err(EditPlanError::new(
                "file.hash_mismatch",
                format!("expected BLAKE3 hash does not match {path}"),
            ));
        }
        validate_operation_metadata(reason, provenance, symbol.as_deref())?;
        validate_range(&base, *range, expected_old)?;
        if expected_old.len() > MAX_EXPECTED_OLD_BYTES {
            return Err(EditPlanError::new(
                "operation.expected_old_limit",
                "expected old content exceeds its bounded size",
            ));
        }
        if inspect_sensitive_text(replacement).is_some() {
            return Err(EditPlanError::new(
                "secret.replacement_content",
                format!("replacement for {path} contains secret-like material"),
            ));
        }
        ranges.push(*range);
    }
    ranges.sort_by_key(|range| (range.start, range.end));
    if ranges.windows(2).any(|pair| pair[0].end > pair[1].start) {
        return Err(EditPlanError::new(
            "operation.overlap",
            format!("byte ranges overlap in {path}"),
        ));
    }

    let mut proposed = base.clone();
    let mut sorted = operations.to_vec();
    sorted.sort_by_key(|operation| match operation {
        EditOperation::Modify { range, .. } => std::cmp::Reverse((range.start, range.end)),
        EditOperation::Create { .. } => std::cmp::Reverse((0, 0)),
    });
    for operation in sorted {
        if let EditOperation::Modify {
            range, replacement, ..
        } = operation
        {
            proposed.replace_range(range.start..range.end, replacement);
        }
    }
    if proposed.len() > plan.limits.max_file_bytes {
        return Err(EditPlanError::new(
            "file.size_after",
            format!("proposed file {path} exceeds the file byte limit"),
        ));
    }
    let proposed_line_ending = detect_line_ending(&proposed)?;
    if actual_line_ending != LineEnding::None && proposed_line_ending != actual_line_ending {
        return Err(EditPlanError::new(
            "file.line_ending_changed",
            format!("proposal changes the line-ending convention of {path}"),
        ));
    }

    Ok(ProposalFileSnapshot {
        path: path.to_string(),
        status: ProposalFileStatus::Modified,
        encoding: TextEncoding::Utf8,
        line_ending: actual_line_ending,
        base_hash: Some(actual_hash),
        base_content: Some(base),
        proposed_hash: content_hash(proposed.as_bytes()),
        proposed_bytes: proposed.len(),
        proposed_content: proposed,
    })
}

fn validate_creation_group(
    root: &Path,
    path: &str,
    operations: &[&EditOperation],
    plan: &EditPlan,
) -> Result<ProposalFileSnapshot, EditPlanError> {
    if operations.len() != 1 {
        return Err(EditPlanError::new(
            "plan.creation_duplicate",
            "a new file must be represented by exactly one creation operation",
        ));
    }
    let EditOperation::Create {
        extension,
        encoding,
        line_ending,
        content,
        reason,
        provenance,
        expected_absent,
        declared_size,
        ..
    } = operations[0]
    else {
        return Err(EditPlanError::new(
            "plan.mixed_operations",
            "a path cannot be both created and modified in one proposal",
        ));
    };
    validate_operation_metadata(reason, provenance, None)?;
    if *encoding != TextEncoding::Utf8 || !*expected_absent || *declared_size != content.len() {
        return Err(EditPlanError::new(
            "creation.contract",
            "new file encoding, expected-absent marker, or declared byte size is invalid",
        ));
    }
    let actual_extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if extension.trim_start_matches('.').to_ascii_lowercase() != actual_extension {
        return Err(EditPlanError::new(
            "creation.extension_mismatch",
            "declared extension does not match the new file path",
        ));
    }
    if content.len() > plan.limits.max_file_bytes || inspect_sensitive_text(content).is_some() {
        return Err(EditPlanError::new(
            "creation.content_refused",
            "new file content exceeds bounds or contains secret-like material",
        ));
    }
    if detect_line_ending(content)? != *line_ending {
        return Err(EditPlanError::new(
            "creation.line_ending_mismatch",
            "declared line ending does not match new file content",
        ));
    }
    let absolute = root.join(Path::new(path));
    match fs::symlink_metadata(&absolute) {
        Ok(_) => {
            return Err(EditPlanError::new(
                "creation.already_exists",
                format!("new file target already exists: {path}"),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(EditPlanError::new(
                "creation.inspect_failed",
                format!("cannot inspect new file target {path}: {error}"),
            ));
        }
    }
    let parent = absolute
        .parent()
        .ok_or_else(|| EditPlanError::new("creation.parent", "new file has no workspace parent"))?;
    let parent = canonical_real_directory(parent)?;
    if !parent.starts_with(root) {
        return Err(EditPlanError::new(
            "path.outside_workspace",
            "new file parent resolves outside the workspace",
        ));
    }
    Ok(ProposalFileSnapshot {
        path: path.to_string(),
        status: ProposalFileStatus::Created,
        encoding: TextEncoding::Utf8,
        line_ending: *line_ending,
        base_content: None,
        base_hash: None,
        proposed_hash: content_hash(content.as_bytes()),
        proposed_bytes: content.len(),
        proposed_content: content.clone(),
    })
}

fn validate_range(source: &str, range: ByteRange, expected_old: &str) -> Result<(), EditPlanError> {
    if range.start > range.end
        || range.end > source.len()
        || !source.is_char_boundary(range.start)
        || !source.is_char_boundary(range.end)
    {
        return Err(EditPlanError::new(
            "operation.range_invalid",
            "byte range is reversed, out of bounds, or splits UTF-8",
        ));
    }
    if source.get(range.start..range.end) != Some(expected_old) {
        return Err(EditPlanError::new(
            "operation.expected_old_mismatch",
            "expected old content does not match the exact byte range",
        ));
    }
    Ok(())
}

fn validate_operation_metadata(
    reason: &str,
    provenance: &[String],
    symbol: Option<&str>,
) -> Result<(), EditPlanError> {
    validate_bounded_text(reason, "operation reason")?;
    if provenance.is_empty() || provenance.len() > MAX_EDIT_LIST_ITEMS {
        return Err(EditPlanError::new(
            "operation.provenance",
            "every operation needs a bounded non-empty provenance list",
        ));
    }
    validate_string_list(provenance, "operation provenance")?;
    if symbol.is_some_and(|value| value.is_empty() || value.chars().count() > 512) {
        return Err(EditPlanError::new(
            "operation.symbol",
            "target symbol is empty or too long",
        ));
    }
    Ok(())
}

fn validate_bounded_text(value: &str, name: &str) -> Result<(), EditPlanError> {
    if value.trim().is_empty()
        || value.chars().count() > MAX_EDIT_REASON_CHARS
        || value.contains('\0')
    {
        return Err(EditPlanError::new(
            "plan.text_limit",
            format!("{name} is empty, too long, or contains NUL"),
        ));
    }
    Ok(())
}

fn validate_string_list(values: &[String], name: &str) -> Result<(), EditPlanError> {
    if values.len() > MAX_EDIT_LIST_ITEMS {
        return Err(EditPlanError::new(
            "plan.list_limit",
            format!("{name} exceeds its item limit"),
        ));
    }
    for value in values {
        validate_bounded_text(value, name)?;
    }
    Ok(())
}

fn estimate_group_lines(operations: &[&EditOperation]) -> (usize, usize) {
    operations.iter().fold(
        (0usize, 0usize),
        |(added, deleted), operation| match operation {
            EditOperation::Modify {
                expected_old,
                replacement,
                ..
            } => (
                added.saturating_add(line_count(replacement)),
                deleted.saturating_add(line_count(expected_old)),
            ),
            EditOperation::Create { content, .. } => {
                (added.saturating_add(line_count(content)), deleted)
            }
        },
    )
}

fn line_count(value: &str) -> usize {
    if value.is_empty() {
        0
    } else {
        value
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
            .saturating_add(1)
    }
}

fn validate_relative_path(value: &str) -> Result<(), EditPlanError> {
    if value.is_empty() || value.len() > 1_024 || value.contains(['\0', '\r', '\n']) {
        return Err(EditPlanError::new(
            "path.invalid",
            "edit path is empty, too long, or contains control characters",
        ));
    }
    let path = Path::new(value);
    if path.is_absolute() {
        return Err(EditPlanError::new(
            "path.absolute",
            "absolute edit paths are refused",
        ));
    }
    let mut count = 0usize;
    for component in path.components() {
        let Component::Normal(name) = component else {
            return Err(EditPlanError::new(
                "path.traversal",
                "edit path contains traversal, root, prefix, or current-directory components",
            ));
        };
        count += 1;
        let text = name.to_string_lossy();
        if text.is_empty() || text.contains(':') || windows_reserved_name(&text) {
            return Err(EditPlanError::new(
                "path.windows_unsafe",
                "edit path contains a Windows-unsafe component",
            ));
        }
    }
    if count == 0 || sensitive_path(value) {
        return Err(EditPlanError::new(
            "path.sensitive",
            "Git metadata, build wrappers, executable files, and secret-like names are refused",
        ));
    }
    Ok(())
}

fn validate_extension(value: &str) -> Result<(), EditPlanError> {
    let extension = Path::new(value)
        .extension()
        .and_then(|item| item.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if !ALLOWED_EDIT_EXTENSIONS.contains(&extension.as_str()) {
        return Err(EditPlanError::new(
            "path.extension_refused",
            format!("file extension for {value} is outside the Minecraft V1 text allowlist"),
        ));
    }
    Ok(())
}

fn sensitive_path(value: &str) -> bool {
    let normalized = value.replace('\\', "/").to_ascii_lowercase();
    let name = normalized.rsplit('/').next().unwrap_or(&normalized);
    normalized == ".git"
        || normalized.starts_with(".git/")
        || normalized.contains("/.git/")
        || normalized == ".opticcode"
        || normalized.starts_with(".opticcode/")
        || normalized.starts_with(".mvn/")
        || normalized.starts_with("gradle/wrapper/")
        || matches!(name, "mvnw" | "mvnw.cmd" | "gradlew" | "gradlew.bat")
        || name.starts_with(".env")
        || name.ends_with(".key")
        || name.ends_with(".pem")
        || name.ends_with(".pfx")
        || name.ends_with(".p12")
        || name.contains("credential")
        || name.contains("secret")
        || name.contains("token")
        || name == "id_rsa"
        || name == "id_ed25519"
}

fn windows_reserved_name(value: &str) -> bool {
    let stem = value
        .trim_end_matches([' ', '.'])
        .split('.')
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|suffix| {
                matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            })
}

fn canonical_real_directory(path: &Path) -> Result<PathBuf, EditPlanError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        EditPlanError::new(
            "path.inspect_failed",
            format!("failed to inspect {}: {error}", path.display()),
        )
    })?;
    if !metadata.is_dir() || metadata_is_link_or_reparse(&metadata) {
        return Err(EditPlanError::new(
            "path.reparse_refused",
            format!(
                "directory is a symlink, junction, reparse point, or non-directory: {}",
                path.display()
            ),
        ));
    }
    fs::canonicalize(path).map_err(|error| {
        EditPlanError::new(
            "path.resolve_failed",
            format!("failed to resolve {}: {error}", path.display()),
        )
    })
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

pub fn detect_line_ending(value: &str) -> Result<LineEnding, EditPlanError> {
    let bytes = value.as_bytes();
    let mut crlf = 0usize;
    let mut lf = 0usize;
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'\n' {
            if index > 0 && bytes[index - 1] == b'\r' {
                crlf += 1;
            } else {
                lf += 1;
            }
        }
        index += 1;
    }
    if crlf > 0 && lf > 0 {
        return Err(EditPlanError::new(
            "file.mixed_line_endings",
            "mixed CRLF and LF line endings are not editable in CHAT-EDIT-001",
        ));
    }
    Ok(if crlf > 0 {
        LineEnding::Crlf
    } else if lf > 0 {
        LineEnding::Lf
    } else {
        LineEnding::None
    })
}

pub fn content_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

pub fn canonical_root_hash(path: &Path) -> Result<String, EditPlanError> {
    let canonical = canonical_real_directory(path)?;
    let mut value = canonical.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        value.make_ascii_lowercase();
    }
    Ok(content_hash(value.as_bytes()))
}

pub fn working_tree_digest(root_hash: &str, head: &str, state_json: &[u8]) -> String {
    let stable_state = serde_json::from_slice::<serde_json::Value>(state_json)
        .ok()
        .and_then(|mut value| {
            value.as_object_mut()?.remove("metrics");
            serde_json::to_vec(&value).ok()
        })
        .unwrap_or_else(|| state_json.to_vec());
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"opticcode-working-tree-v1\0");
    hasher.update(root_hash.as_bytes());
    hasher.update(b"\0");
    hasher.update(head.as_bytes());
    hasher.update(b"\0");
    hasher.update(&stable_state);
    hasher.finalize().to_hex().to_string()
}

pub fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn normalize_relative(value: &str) -> String {
    value.replace('\\', "/")
}

fn is_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_git_oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::{EditPlanLimits, EditValidationKind};

    fn fixture() -> (TempDir, EditPlanExpectations, EditPlan) {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("src/main/java/dev/test")).unwrap();
        let source = "package dev.test;\r\nclass Example {\r\n    int value = 1;\r\n}\r\n";
        fs::write(
            temp.path().join("src/main/java/dev/test/Example.java"),
            source,
        )
        .unwrap();
        let now = unix_millis();
        let root_hash = canonical_root_hash(temp.path()).unwrap();
        let expectations = EditPlanExpectations {
            request_id: "request-1".to_string(),
            plan_id: "plan-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            workspace_root: temp.path().to_path_buf(),
            workspace_root_hash: root_hash.clone(),
            profile: "minecraft-java-1.8".to_string(),
            provider: opticcode_llm::ProviderId::Ollama,
            model: "fixture-model".to_string(),
            base_head: "a".repeat(40),
            working_tree_digest: "b".repeat(64),
            now_unix_ms: now,
            limits: EditPlanLimits::default(),
        };
        let start = source.find("1").unwrap();
        let mut plan = EditPlan {
            schema_version: EDIT_PLAN_SCHEMA_VERSION,
            plan_id: expectations.plan_id.clone(),
            request_id: expectations.request_id.clone(),
            workspace_id: expectations.workspace_id.clone(),
            workspace_root_hash: root_hash,
            profile: expectations.profile.clone(),
            provider: expectations.provider,
            model: expectations.model.clone(),
            base_head: expectations.base_head.clone(),
            working_tree_digest: expectations.working_tree_digest.clone(),
            context_used: Vec::new(),
            user_references: Vec::new(),
            summary: "Change the fixture value.".to_string(),
            rationale_summary: "The selected constant should be updated.".to_string(),
            operations: vec![EditOperation::Modify {
                path: "src/main/java/dev/test/Example.java".to_string(),
                expected_file_hash: content_hash(source.as_bytes()),
                encoding: TextEncoding::Utf8,
                line_ending: LineEnding::Crlf,
                range: ByteRange {
                    start,
                    end: start + 1,
                },
                expected_old: "1".to_string(),
                replacement: "2".to_string(),
                reason: "Use the requested value.".to_string(),
                symbol: Some("Example.value".to_string()),
                provenance: vec!["user_reference".to_string()],
            }],
            validations: vec![
                EditValidationKind::ReparseJava,
                EditValidationKind::BuildOffline,
            ],
            risks: vec!["Behavior changes from one constant to another.".to_string()],
            limitations: vec!["Fixture-only validation.".to_string()],
            limits: EditPlanLimits::default(),
            expires_at_unix_ms: now + 60_000,
        };
        plan.context_used.push(crate::EditContextReference {
            source: "fixture".to_string(),
            provenance: "test".to_string(),
            content_hash: None,
        });
        (temp, expectations, plan)
    }

    #[test]
    fn valid_plan_materializes_exact_crlf_utf8_snapshot() {
        let (_temp, expected, plan) = fixture();
        let validated = validate_edit_plan(plan, &expected).unwrap();

        assert_eq!(validated.files.len(), 1);
        assert!(validated.files[0].proposed_content.contains("value = 2"));
        assert_eq!(validated.files[0].line_ending, LineEnding::Crlf);
    }

    #[test]
    fn parser_rejects_prose_unknown_fields_and_unknown_versions() {
        let (_temp, _expected, plan) = fixture();
        let json = serde_json::to_string(&plan).unwrap();
        assert_eq!(
            parse_edit_plan_json(&format!("plan: {json}"))
                .unwrap_err()
                .code,
            "plan.trailing_text"
        );

        let mut value = serde_json::to_value(&plan).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("surprise".to_string(), serde_json::json!(true));
        assert_eq!(
            parse_edit_plan_json(&value.to_string()).unwrap_err().code,
            "plan.invalid_json"
        );

        let mut wrong = plan;
        wrong.schema_version = 99;
        assert_eq!(
            validate_edit_plan(wrong, &_expected).unwrap_err().code,
            "plan.schema_version"
        );
    }

    #[test]
    fn trusted_identity_hash_range_and_overlap_fail_closed() {
        let (_temp, expected, mut plan) = fixture();
        plan.request_id = "other".to_string();
        assert_eq!(
            validate_edit_plan(plan, &expected).unwrap_err().code,
            "plan.request_id_mismatch"
        );

        let (_temp, expected, mut plan) = fixture();
        if let EditOperation::Modify {
            expected_file_hash, ..
        } = &mut plan.operations[0]
        {
            *expected_file_hash = "c".repeat(64);
        }
        assert_eq!(
            validate_edit_plan(plan, &expected).unwrap_err().code,
            "file.hash_mismatch"
        );

        let (_temp, expected, mut plan) = fixture();
        let duplicate = plan.operations[0].clone();
        plan.operations.push(duplicate);
        assert_eq!(
            validate_edit_plan(plan, &expected).unwrap_err().code,
            "operation.overlap"
        );
    }

    #[test]
    fn absolute_traversal_secret_binary_delete_and_rename_shapes_are_refused() {
        for candidate in [
            "C:/outside/Example.java",
            "../outside.java",
            ".git/config.txt",
            ".env.txt",
            "src/main/resources/server.key",
            "mvnw.cmd",
            "plugin.jar",
        ] {
            assert!(
                validate_relative_path(candidate).is_err()
                    || validate_extension(candidate).is_err()
            );
        }
        let unknown_delete = r#"{"type":"delete","path":"x.java"}"#;
        assert!(serde_json::from_str::<EditOperation>(unknown_delete).is_err());
        let unknown_rename = r#"{"type":"rename","path":"x.java","to":"y.java"}"#;
        assert!(serde_json::from_str::<EditOperation>(unknown_rename).is_err());
    }

    #[test]
    fn creation_requires_absence_exact_size_allowed_extension_and_existing_parent() {
        let (_temp, expected, mut plan) = fixture();
        plan.operations = vec![EditOperation::Create {
            path: "src/main/java/dev/test/NewFile.java".to_string(),
            extension: ".java".to_string(),
            encoding: TextEncoding::Utf8,
            line_ending: LineEnding::Lf,
            content: "package dev.test;\nclass NewFile {}\n".to_string(),
            reason: "Add the requested type.".to_string(),
            provenance: vec!["user_request".to_string()],
            expected_absent: true,
            declared_size: 36,
        }];
        let content_len = match &plan.operations[0] {
            EditOperation::Create { content, .. } => content.len(),
            _ => 0,
        };
        if let EditOperation::Create { declared_size, .. } = &mut plan.operations[0] {
            *declared_size = content_len;
        }
        let validated = validate_edit_plan(plan.clone(), &expected).unwrap();
        assert_eq!(validated.files[0].status, ProposalFileStatus::Created);

        if let EditOperation::Create {
            expected_absent, ..
        } = &mut plan.operations[0]
        {
            *expected_absent = false;
        }
        assert_eq!(
            validate_edit_plan(plan, &expected).unwrap_err().code,
            "creation.contract"
        );
    }

    #[test]
    fn unicode_ranges_must_use_utf8_byte_boundaries() {
        let (_temp, expected, mut plan) = fixture();
        let source = "package dev.test;\r\nclass Example { String value = \"é\"; }\r\n";
        let file = expected
            .workspace_root
            .join("src/main/java/dev/test/Example.java");
        fs::write(&file, source).unwrap();
        let start = source.find('é').unwrap();
        if let EditOperation::Modify {
            expected_file_hash,
            range,
            expected_old,
            replacement,
            ..
        } = &mut plan.operations[0]
        {
            *expected_file_hash = content_hash(source.as_bytes());
            *range = ByteRange {
                start,
                end: start + 1,
            };
            *expected_old = "é".to_string();
            *replacement = "e".to_string();
        }
        assert_eq!(
            validate_edit_plan(plan, &expected).unwrap_err().code,
            "operation.range_invalid"
        );
    }

    #[test]
    fn configured_limits_can_only_move_down() {
        let (_temp, mut expected, mut plan) = fixture();
        expected.limits.max_files = 1;
        plan.limits.max_files = 2;
        assert_eq!(
            validate_edit_plan(plan, &expected).unwrap_err().code,
            "plan.limit_escalation"
        );
    }

    #[test]
    fn mandatory_validations_cannot_be_omitted_or_duplicated() {
        let (_temp, expected, mut plan) = fixture();
        plan.validations = vec![EditValidationKind::ReparseJava];
        assert_eq!(
            validate_edit_plan(plan, &expected).unwrap_err().code,
            "plan.validation_contract"
        );

        let (_temp, expected, mut plan) = fixture();
        plan.validations.push(EditValidationKind::BuildOffline);
        assert_eq!(
            validate_edit_plan(plan, &expected).unwrap_err().code,
            "plan.validation_contract"
        );
    }

    #[test]
    fn working_tree_digest_ignores_volatile_observation_metrics() {
        let first = serde_json::json!({
            "schema_version": 1,
            "root": "C:/fixture",
            "changes": [],
            "metrics": {"duration_us": 1, "status_entries": 0}
        });
        let second = serde_json::json!({
            "schema_version": 1,
            "root": "C:/fixture",
            "changes": [],
            "metrics": {"duration_us": 999, "status_entries": 0}
        });
        assert_eq!(
            working_tree_digest("a", "b", &serde_json::to_vec(&first).unwrap()),
            working_tree_digest("a", "b", &serde_json::to_vec(&second).unwrap())
        );
    }

    #[test]
    fn line_ending_detector_rejects_mixed_content() {
        assert_eq!(detect_line_ending("a\n").unwrap(), LineEnding::Lf);
        assert_eq!(detect_line_ending("a\r\n").unwrap(), LineEnding::Crlf);
        assert!(detect_line_ending("a\r\nb\n").is_err());
    }

    #[test]
    fn paths_are_unique_after_separator_normalization() {
        let values = ["src/main/A.java", "src\\main\\A.java"];
        let normalized = values
            .iter()
            .map(|value| normalize_relative(value))
            .collect::<BTreeSet<_>>();
        assert_eq!(normalized.len(), 1);
    }
}
