use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    EditIntentOperationKind, EditIntentTarget, EditPlanExpectations, EditPlanLimits, LineEnding,
    ProposalIntentBinding, ValidatedEditIntent, ALLOWED_EDIT_EXTENSIONS, MAX_EDIT_FILE_BYTES,
    MAX_EDIT_PROPOSAL_BYTES,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedEditFile {
    pub path: String,
    pub content_hash: String,
    pub bytes: usize,
    pub line_ending: LineEnding,
    pub line_anchors: Vec<TrustedEditLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedEditLine {
    pub start: usize,
    pub end: usize,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct EditGenerationInput<'a> {
    pub task: &'a str,
    pub policy_summary: &'a str,
    pub selected_context: &'a str,
    pub user_references: &'a str,
    pub available_files: &'a [String],
    pub trusted_files: &'a [TrustedEditFile],
    pub expected: &'a EditPlanExpectations,
}

#[derive(Debug, Clone)]
pub struct BoundEditGenerationInput<'a> {
    pub base: EditGenerationInput<'a>,
    pub intent: &'a ValidatedEditIntent,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditGenerationAttempts {
    pub primary_attempted: bool,
    pub format_correction_attempted: bool,
    pub format_correction_succeeded: bool,
    pub primary_error_code: Option<String>,
}

pub fn build_edit_generation_prompt(input: EditGenerationInput<'_>) -> Result<String> {
    let expected = input.expected;
    let schema = edit_plan_contract_json(expected)?;
    let trusted_files = render_trusted_files(input.trusted_files)?;
    let mut files = input.available_files.to_vec();
    files.sort();
    files.dedup();
    files.truncate(2_000);
    let prompt = format!(
        concat!(
            "[SYSTEM]\n",
            "You are OpticCode's local structured edit planner for Java 8 and Minecraft 1.8.8. ",
            "Return exactly one JSON object. Do not use Markdown, prose, comments, or code fences. ",
            "The runtime treats every byte as untrusted and will reject any mismatch.\n\n",
            "[SECURITY_POLICY]\n{}\n",
            "Only modify existing UTF-8 text files or create at most one allowlisted UTF-8 text file. ",
            "Never delete, rename, move, download, install, edit build wrappers, touch .git/.opticcode, ",
            "or request network access. Use UTF-8 byte offsets, never guessed line numbers. ",
            "A modify operation is permitted only for a path listed in TRUSTED_FILE_SNAPSHOTS. ",
            "Copy its path, full-file BLAKE3 hash, and line ending exactly; never calculate or invent them. ",
            "Prefer replacing one or more complete line_anchors: copy start, end, and content exactly ",
            "into range and expected_old, then alter only replacement. The runtime rejects guessed offsets.\n\n",
            "A modify object has exactly these fields: type=modify, path, expected_file_hash, ",
            "encoding=utf8, line_ending, range={{start,end}}, expected_old, replacement, reason, ",
            "optional symbol, and provenance.\n\n",
            "For a create operation, replace the modify object with only these fields: ",
            "type=create, path, extension, encoding, line_ending, content, reason, provenance, ",
            "expected_absent=true, and declared_size. Never add helper or alternative fields.\n\n",
            "[TRUSTED_IDENTITIES]\n",
            "plan_id={}\nrequest_id={}\nworkspace_id={}\nworkspace_root_hash={}\n",
            "profile={}\nprovider={}\nmodel={}\nbase_head={}\nworking_tree_digest={}\n",
            "expires_at_unix_ms={}\n\n",
            "[TRUSTED_FILE_SNAPSHOTS]\n{}\n\n",
            "[EDIT_PLAN_JSON_CONTRACT]\n{}\n\n",
            "[AVAILABLE_FILES]\n{}\n\n",
            "[USER_REFERENCES]\n{}\n\n",
            "[SELECTED_CONTEXT]\n{}\n\n",
            "[TASK]\n{}\n\n",
            "[FINAL_INSTRUCTION]\nReturn exactly one complete EditPlan JSON object matching the contract."
        ),
        bounded(input.policy_summary, 16 * 1024),
        expected.plan_id,
        expected.request_id,
        expected.workspace_id,
        expected.workspace_root_hash,
        expected.profile,
        expected.provider.as_str(),
        expected.model,
        expected.base_head,
        expected.working_tree_digest,
        expected
            .now_unix_ms
            .saturating_add(crate::DEFAULT_PROPOSAL_TTL_SECONDS.saturating_mul(1_000)),
        trusted_files,
        schema,
        files.join("\n"),
        bounded(input.user_references, 256 * 1024),
        bounded(input.selected_context, 1024 * 1024),
        bounded(input.task, 64 * 1024),
    );
    if prompt.len() > 4 * 1024 * 1024 {
        anyhow::bail!("structured edit prompt exceeds its 4 MiB runtime bound");
    }
    Ok(prompt)
}

pub fn build_bound_edit_generation_prompt(input: BoundEditGenerationInput<'_>) -> Result<String> {
    let expected = input.base.expected;
    validate_generation_intent_binding(expected, input.intent)?;
    let prompt = build_edit_generation_prompt(input.base)?;
    bind_prompt_to_intent(prompt, expected, input.intent)
}
pub fn build_format_correction_prompt(
    invalid_output: &str,
    error_code: &str,
    error_message: &str,
    expected: &EditPlanExpectations,
    trusted_files: &[TrustedEditFile],
) -> Result<String> {
    let schema = edit_plan_contract_json(expected)?;
    let trusted_files = render_trusted_files(trusted_files)?;
    Ok(format!(
        concat!(
            "[SYSTEM]\n",
            "Correct only the JSON formatting/schema error in the previous EditPlan. ",
            "Do not change the requested behavior, operations, paths, ranges, old content, or new content. ",
            "Do not add context. Return exactly one JSON object and nothing else.\n\n",
            "[STRUCTURED_ERROR]\ncode={}\nmessage={}\n\n",
            "[TRUSTED_FILE_SNAPSHOTS]\n{}\n\n",
            "[REQUIRED_CONTRACT]\n{}\n\n",
            "[PREVIOUS_OUTPUT]\n{}\n\n",
            "[FINAL_INSTRUCTION]\nReturn the same plan corrected to exactly match the contract."
        ),
        bounded(error_code, 256),
        bounded(error_message, 8 * 1024),
        trusted_files,
        schema,
        bounded(invalid_output, MAX_EDIT_PROPOSAL_BYTES),
    ))
}

pub fn build_bound_format_correction_prompt(
    invalid_output: &str,
    error_code: &str,
    error_message: &str,
    expected: &EditPlanExpectations,
    trusted_files: &[TrustedEditFile],
    intent: &ValidatedEditIntent,
) -> Result<String> {
    validate_generation_intent_binding(expected, intent)?;
    let prompt = build_format_correction_prompt(
        invalid_output,
        error_code,
        error_message,
        expected,
        trusted_files,
    )?;
    bind_prompt_to_intent(prompt, expected, intent)
}
pub fn edit_plan_output_schema(
    expected: &EditPlanExpectations,
    trusted_files: &[TrustedEditFile],
) -> serde_json::Value {
    let mut operation_variants = trusted_files
        .iter()
        .map(modify_operation_schema)
        .collect::<Vec<_>>();
    operation_variants.push(create_operation_schema());
    let bounded_text = serde_json::json!({"type": "string"});
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": [
            "schema_version", "plan_id", "request_id", "workspace_id",
            "workspace_root_hash", "profile", "provider", "model", "base_head",
            "working_tree_digest", "context_used", "user_references", "summary",
            "rationale_summary", "operations", "validations", "risks", "limitations",
            "limits", "expires_at_unix_ms"
        ],
        "properties": {
            "schema_version": {"const": crate::EDIT_PLAN_SCHEMA_VERSION},
            "plan_id": {"const": expected.plan_id},
            "request_id": {"const": expected.request_id},
            "workspace_id": {"const": expected.workspace_id},
            "workspace_root_hash": {"const": expected.workspace_root_hash},
            "profile": {"const": expected.profile},
            "provider": {"const": expected.provider.as_str()},
            "model": {"const": expected.model},
            "base_head": {"const": expected.base_head},
            "working_tree_digest": {"const": expected.working_tree_digest},
            "context_used": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["source", "provenance"],
                    "properties": {
                        "source": bounded_text.clone(),
                        "provenance": bounded_text.clone(),
                        "content_hash": {"type": "string"}
                    }
                }
            },
            "user_references": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["reference_id", "kind", "provenance"],
                    "properties": {
                        "reference_id": bounded_text.clone(),
                        "kind": bounded_text.clone(),
                        "path": bounded_text.clone(),
                        "provenance": bounded_text.clone()
                    }
                }
            },
            "summary": bounded_text.clone(),
            "rationale_summary": bounded_text.clone(),
            "operations": {
                "type": "array",
                "items": {"oneOf": operation_variants}
            },
            "validations": {
                "type": "array",
                "items": {"enum": ["reparse_java", "build_offline", "test_offline"]}
            },
            "risks": {
                "type": "array",
                "items": bounded_text.clone()
            },
            "limitations": {
                "type": "array",
                "items": bounded_text
            },
            "limits": {"const": expected.limits},
            "expires_at_unix_ms": {
                "const": expected.now_unix_ms.saturating_add(
                    crate::DEFAULT_PROPOSAL_TTL_SECONDS.saturating_mul(1_000)
                )
            }
        }
    })
}

pub fn edit_plan_output_schema_for_intent(
    expected: &EditPlanExpectations,
    trusted_files: &[TrustedEditFile],
    intent: &ValidatedEditIntent,
) -> Result<serde_json::Value> {
    validate_generation_intent_binding(expected, intent)?;

    let allowed_operations = intent
        .intent
        .constraints
        .allowed_operations
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    let mut operation_variants = Vec::new();

    for target in &intent.intent.targets {
        match target {
            EditIntentTarget::ExistingFile {
                path, content_hash, ..
            } => {
                if !allowed_operations.contains(&EditIntentOperationKind::ModifyExisting) {
                    anyhow::bail!(
                        "validated intent contains an existing target without modify permission"
                    );
                }
                let trusted = trusted_files
                    .iter()
                    .find(|file| file.path == *path)
                    .with_context(|| {
                        format!("validated intent target {path} has no trusted file snapshot")
                    })?;
                if trusted.content_hash != *content_hash {
                    anyhow::bail!(
                        "validated intent target {path} does not match its trusted file hash"
                    );
                }
                operation_variants.push(modify_operation_schema(trusted));
            }
            EditIntentTarget::ProspectiveFile {
                path, extension, ..
            } => {
                if !allowed_operations.contains(&EditIntentOperationKind::CreateTextFile) {
                    anyhow::bail!(
                        "validated intent contains a prospective target without create permission"
                    );
                }
                operation_variants.push(create_operation_schema_for_target(path, extension));
            }
        }
    }

    if operation_variants.is_empty() {
        anyhow::bail!("validated edit intent exposes no generation operation");
    }

    let mut schema = edit_plan_output_schema(expected, trusted_files);
    schema["properties"]["operations"]["items"]["oneOf"] =
        serde_json::Value::Array(operation_variants);
    schema["properties"]["limits"] = serde_json::json!({"const": intent.intent.constraints.limits});
    Ok(schema)
}
fn modify_operation_schema(file: &TrustedEditFile) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": [
            "type", "path", "expected_file_hash", "encoding", "line_ending", "range",
            "expected_old", "replacement", "reason", "provenance"
        ],
        "properties": {
            "type": {"const": "modify"},
            "path": {"const": file.path},
            "expected_file_hash": {"const": file.content_hash},
            "encoding": {"const": "utf8"},
            "line_ending": {"const": file.line_ending.as_str()},
            "range": {
                "type": "object",
                "additionalProperties": false,
                "required": ["start", "end"],
                "properties": {
                    "start": {"type": "integer"},
                    "end": {"type": "integer"}
                }
            },
            "expected_old": {"type": "string"},
            "replacement": {"type": "string"},
            "reason": {"type": "string"},
            "symbol": {"type": "string"},
            "provenance": {
                "type": "array",
                "items": {"type": "string"}
            }
        }
    })
}

fn create_operation_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": [
            "type", "path", "extension", "encoding", "line_ending", "content", "reason",
            "provenance", "expected_absent", "declared_size"
        ],
        "properties": {
            "type": {"const": "create"},
            "path": {"type": "string"},
            "extension": {"enum": ALLOWED_EDIT_EXTENSIONS},
            "encoding": {"const": "utf8"},
            "line_ending": {"enum": ["none", "lf", "crlf"]},
            "content": {"type": "string"},
            "reason": {"type": "string"},
            "provenance": {
                "type": "array",
                "items": {"type": "string"}
            },
            "expected_absent": {"const": true},
            "declared_size": {"type": "integer"}
        }
    })
}

fn create_operation_schema_for_target(path: &str, extension: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": [
            "type", "path", "extension", "encoding", "line_ending", "content", "reason",
            "provenance", "expected_absent", "declared_size"
        ],
        "properties": {
            "type": {"const": "create"},
            "path": {"const": path},
            "extension": {"const": extension},
            "encoding": {"const": "utf8"},
            "line_ending": {"enum": ["none", "lf", "crlf"]},
            "content": {"type": "string"},
            "reason": {"type": "string"},
            "provenance": {
                "type": "array",
                "items": {"type": "string"}
            },
            "expected_absent": {"const": true},
            "declared_size": {"type": "integer"}
        }
    })
}
pub fn new_edit_id(prefix: &str) -> Result<String> {
    if prefix.is_empty()
        || prefix.len() > 32
        || !prefix
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
    {
        anyhow::bail!("edit ID prefix is invalid");
    }
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).context("failed to obtain random bytes for edit identity")?;
    let suffix = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("{prefix}-{suffix}"))
}

fn edit_plan_contract_json(expected: &EditPlanExpectations) -> Result<String> {
    edit_plan_contract_json_with_limits(expected, expected.limits)
}

fn edit_plan_contract_json_with_limits(
    expected: &EditPlanExpectations,
    limits: EditPlanLimits,
) -> Result<String> {
    let value = serde_json::json!({
        "schema_version": crate::EDIT_PLAN_SCHEMA_VERSION,
        "plan_id": expected.plan_id,
        "request_id": expected.request_id,
        "workspace_id": expected.workspace_id,
        "workspace_root_hash": expected.workspace_root_hash,
        "profile": expected.profile,
        "provider": expected.provider.as_str(),
        "model": expected.model,
        "base_head": expected.base_head,
        "working_tree_digest": expected.working_tree_digest,
        "context_used": [{"source": "relative source identifier", "provenance": "symbol|rag|user_reference", "content_hash": null}],
        "user_references": [{"reference_id": "id", "kind": "file|range|selection|symbol", "path": "relative/path.java", "provenance": "user_reference"}],
        "summary": "short user-facing summary",
        "rationale_summary": "short decision summary without private chain-of-thought",
        "operations": [],
        "validations": ["reparse_java", "build_offline", "test_offline"],
        "risks": ["bounded risk"],
        "limitations": ["bounded limitation"],
        "limits": limits,
        "expires_at_unix_ms": expected.now_unix_ms.saturating_add(crate::DEFAULT_PROPOSAL_TTL_SECONDS.saturating_mul(1_000))
    });
    serde_json::to_string_pretty(&value).context("failed to render edit plan contract")
}

fn render_trusted_files(files: &[TrustedEditFile]) -> Result<String> {
    if files.len() > crate::MAX_EDIT_FILES {
        anyhow::bail!("trusted edit file inventory exceeds the runtime file limit");
    }
    if files.iter().any(|file| {
        file.path.is_empty()
            || file.path.len() > 4_096
            || file.content_hash.len() != 64
            || !file
                .content_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || file.bytes > MAX_EDIT_FILE_BYTES
            || !valid_line_anchors(file)
    }) {
        anyhow::bail!("trusted edit file inventory contains invalid metadata");
    }
    if files.is_empty() {
        return Ok("[]".to_string());
    }
    let rendered = serde_json::to_string_pretty(files)
        .context("failed to render trusted edit file inventory")?;
    if rendered.len() > MAX_EDIT_PROPOSAL_BYTES {
        anyhow::bail!("trusted edit file inventory exceeds its 2 MiB prompt bound");
    }
    Ok(rendered)
}

fn validate_generation_intent_binding(
    expected: &EditPlanExpectations,
    intent: &ValidatedEditIntent,
) -> Result<()> {
    let binding = ProposalIntentBinding::from_validated(intent).map_err(anyhow::Error::new)?;
    binding.validate().map_err(anyhow::Error::new)?;

    for (name, actual, trusted) in [
        (
            "request_id",
            expected.request_id.as_str(),
            intent.intent.request_id.as_str(),
        ),
        (
            "workspace_id",
            expected.workspace_id.as_str(),
            intent.intent.workspace_id.as_str(),
        ),
        (
            "workspace_root_hash",
            expected.workspace_root_hash.as_str(),
            intent.intent.workspace_root_hash.as_str(),
        ),
        (
            "base_head",
            expected.base_head.as_str(),
            intent.intent.base_head.as_str(),
        ),
        (
            "working_tree_digest",
            expected.working_tree_digest.as_str(),
            intent.intent.working_tree_digest.as_str(),
        ),
    ] {
        if actual != trusted {
            anyhow::bail!(
                "validated edit intent {name} is not bound to the generation expectations"
            );
        }
    }
    Ok(())
}

fn bind_prompt_to_intent(
    mut prompt: String,
    expected: &EditPlanExpectations,
    intent: &ValidatedEditIntent,
) -> Result<String> {
    let legacy_contract = edit_plan_contract_json(expected)?;
    let bound_contract =
        edit_plan_contract_json_with_limits(expected, intent.intent.constraints.limits)?;
    if !prompt.contains(&legacy_contract) {
        anyhow::bail!("structured edit prompt is missing its EditPlan contract marker");
    }
    prompt = prompt.replacen(&legacy_contract, &bound_contract, 1);

    let marker = "[TRUSTED_FILE_SNAPSHOTS]";
    if !prompt.contains(marker) {
        anyhow::bail!("structured edit prompt is missing its trusted snapshot marker");
    }
    let section = format!(
        concat!(
            "[VALIDATED_EDIT_INTENT]\n",
            "intent_hash={}\n",
            "{}\n",
            "This validated intent is authoritative. AVAILABLE_FILES is discovery only, ",
            "never authorization. Emit operations only for exact intent targets and kinds.\n\n",
            "{}"
        ),
        intent.intent_hash, intent.canonical_json, marker
    );
    prompt = prompt.replacen(marker, &section, 1);

    if prompt.len() > 4 * 1024 * 1024 {
        anyhow::bail!("intent-bound edit prompt exceeds its 4 MiB runtime bound");
    }
    Ok(prompt)
}
fn valid_line_anchors(file: &TrustedEditFile) -> bool {
    if file.bytes == 0 {
        return file.line_anchors.is_empty();
    }
    let mut next = 0usize;
    for anchor in &file.line_anchors {
        if anchor.start != next
            || anchor.end <= anchor.start
            || anchor.end.saturating_sub(anchor.start) != anchor.content.len()
        {
            return false;
        }
        next = anchor.end;
    }
    next == file.bytes
}

fn bounded(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[bounded]", &value[..end])
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn expectations() -> EditPlanExpectations {
        EditPlanExpectations {
            request_id: "request-1".to_string(),
            plan_id: "plan-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            workspace_root: PathBuf::from("fixture"),
            workspace_root_hash: "a".repeat(64),
            profile: "minecraft-java-1.8".to_string(),
            provider: opticcode_llm::ProviderId::Ollama,
            model: "fixture".to_string(),
            base_head: "b".repeat(40),
            working_tree_digest: "c".repeat(64),
            now_unix_ms: 1_000,
            limits: EditPlanLimits::default(),
        }
    }

    #[test]
    fn generation_prompt_has_stable_security_and_exact_identities() {
        let expected = expectations();
        let trusted = TrustedEditFile {
            path: "src/Example.java".to_string(),
            content_hash: "d".repeat(64),
            bytes: 16,
            line_ending: LineEnding::Lf,
            line_anchors: vec![TrustedEditLine {
                start: 0,
                end: 16,
                content: "class Example {}".to_string(),
            }],
        };
        let prompt = build_edit_generation_prompt(EditGenerationInput {
            task: "Change the selected method.",
            policy_summary: "deny by default",
            selected_context: "class Example {}",
            user_references: "selection: Example.java",
            available_files: &["src/Example.java".to_string()],
            trusted_files: std::slice::from_ref(&trusted),
            expected: &expected,
        })
        .unwrap();
        assert!(prompt.contains("Return exactly one JSON object"));
        assert!(prompt.contains("Never delete, rename, move"));
        assert!(prompt.contains("plan_id=plan-1"));
        assert!(prompt.contains("expected_old"));
        assert!(prompt.contains(&trusted.content_hash));
        assert!(!prompt.contains("64 lowercase hex BLAKE3"));
        assert!(prompt.contains("\"start\": 0"));
    }

    #[test]
    fn generation_contract_is_a_real_edit_plan_without_helper_fields() {
        let contract = edit_plan_contract_json(&expectations()).unwrap();
        let plan = serde_json::from_str::<crate::EditPlan>(&contract).unwrap();

        assert_eq!(plan.plan_id, "plan-1");
        assert!(plan.operations.is_empty());
        assert!(!contract.contains("creation_operation_alternative"));
    }

    #[test]
    fn format_correction_contains_no_new_project_context() {
        let expected = expectations();
        let trusted = TrustedEditFile {
            path: "src/Example.java".to_string(),
            content_hash: "d".repeat(64),
            bytes: 16,
            line_ending: LineEnding::Lf,
            line_anchors: vec![TrustedEditLine {
                start: 0,
                end: 16,
                content: "class Example {}".to_string(),
            }],
        };
        let correction = build_format_correction_prompt(
            "{invalid}",
            "plan.invalid_json",
            "missing quote",
            &expected,
            std::slice::from_ref(&trusted),
        )
        .unwrap();
        assert!(correction.contains("{invalid}"));
        assert!(correction.contains("missing quote"));
        assert!(correction.contains(&trusted.content_hash));
        assert!(correction.contains("class Example"));
        assert!(!correction.contains("Change the selected method"));
    }

    #[test]
    fn native_output_schema_binds_identity_and_operation_shapes() {
        let expected = expectations();
        let trusted = TrustedEditFile {
            path: "src/Example.java".to_string(),
            content_hash: "d".repeat(64),
            bytes: 16,
            line_ending: LineEnding::Lf,
            line_anchors: vec![TrustedEditLine {
                start: 0,
                end: 16,
                content: "class Example {}".to_string(),
            }],
        };
        let schema = edit_plan_output_schema(&expected, &[trusted]);
        let rendered = serde_json::to_string(&schema).unwrap();

        assert_eq!(schema["properties"]["plan_id"]["const"], "plan-1");
        let modify = &schema["properties"]["operations"]["items"]["oneOf"][0];
        assert_eq!(modify["properties"]["path"]["const"], "src/Example.java");
        assert_eq!(modify["properties"]["provenance"]["type"], "array");
        assert!(rendered.len() < opticcode_llm::MAX_OUTPUT_SCHEMA_BYTES);
    }

    #[test]
    fn generated_ids_are_bounded_and_distinct() {
        let first = new_edit_id("proposal").unwrap();
        let second = new_edit_id("proposal").unwrap();
        assert_ne!(first, second);
        assert!(first.len() < 64);
    }
}
