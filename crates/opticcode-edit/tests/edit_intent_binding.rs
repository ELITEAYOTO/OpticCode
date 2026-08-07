use std::path::PathBuf;

use opticcode_edit::{
    build_bound_edit_generation_prompt, edit_plan_output_schema_for_intent, validate_edit_intent,
    validate_edit_plan_against_intent, BoundEditGenerationInput, ByteRange, EditGenerationInput,
    EditIntent, EditIntentAllowedExistingTarget, EditIntentConstraints, EditIntentExpectations,
    EditIntentSelectionMode, EditIntentTarget, EditIntentTargetProvenance, EditOperation, EditPlan,
    EditPlanExpectations, EditPlanLimits, EditValidationKind, LineEnding, ProposalFileSnapshot,
    ProposalFileStatus, ProposalStore, TextEncoding, TrustedEditFile, TrustedEditLine,
    ValidatedEditIntent, ValidatedEditPlan, EDIT_INTENT_SCHEMA_VERSION,
};
use opticcode_llm::ProviderId;

const NOW: u64 = 1_800_000_000_000;
const ROOT_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TREE_DIGEST: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const BASE_HEAD: &str = "cccccccccccccccccccccccccccccccccccccccc";
const FILE_HASH: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

#[test]
fn bound_prompt_schema_and_store_preserve_the_exact_intent() {
    let expected = plan_expectations();
    let intent = validated_intent();
    let trusted = trusted_file();

    let prompt = build_bound_edit_generation_prompt(BoundEditGenerationInput {
        base: EditGenerationInput {
            task: "Update the selected fixture.",
            policy_summary: "deny by default",
            selected_context: "class Example { int value = 1; }\n",
            user_references: r#"[{"reference_id":"ref-example"}]"#,
            available_files: &["src/Example.java".to_string()],
            trusted_files: std::slice::from_ref(&trusted),
            expected: &expected,
        },
        intent: &intent,
    })
    .unwrap();

    assert!(prompt.contains("[VALIDATED_EDIT_INTENT]"));
    assert!(prompt.contains(&intent.intent_hash));
    assert!(prompt.contains("AVAILABLE_FILES is discovery only"));
    assert!(prompt.contains("\"max_created_files\": 0"));

    let schema =
        edit_plan_output_schema_for_intent(&expected, std::slice::from_ref(&trusted), &intent)
            .unwrap();
    let operations = &schema["properties"]["operations"]["items"]["oneOf"];
    assert_eq!(operations.as_array().unwrap().len(), 1);
    assert_eq!(
        operations[0]["properties"]["path"]["const"],
        "src/Example.java"
    );
    assert_eq!(
        schema["properties"]["limits"]["const"]["max_created_files"],
        0
    );

    let validated_plan = validated_plan();
    validate_edit_plan_against_intent(&validated_plan, &intent).unwrap();

    let state = tempfile::tempdir().unwrap();
    let store = ProposalStore::open(state.path(), ROOT_HASH).unwrap();
    let created = store.create_with_intent(validated_plan, &intent).unwrap();
    let binding = created
        .intent
        .as_ref()
        .expect("intent binding is persisted");
    assert_eq!(binding.intent_id, intent.intent.intent_id);
    assert_eq!(binding.intent_hash, intent.intent_hash);
    binding.validate().unwrap();
    let persisted = serde_json::to_string(binding).unwrap();
    assert!(!persisted.contains(&intent.intent.task));
    assert!(persisted.contains(&binding.task_hash));

    let loaded = store.load(&created.proposal_id).unwrap();
    assert_eq!(loaded.intent, created.intent);
}

#[test]
fn plan_binding_rejects_untrusted_paths_hashes_and_creations() {
    let intent = validated_intent();

    let mut outside = validated_plan();
    if let EditOperation::Modify { path, .. } = &mut outside.plan.operations[0] {
        *path = "src/Other.java".to_string();
    }
    assert_eq!(
        validate_edit_plan_against_intent(&outside, &intent)
            .unwrap_err()
            .code,
        "intent.plan_target_not_allowed"
    );

    let mut hash_drift = validated_plan();
    if let EditOperation::Modify {
        expected_file_hash, ..
    } = &mut hash_drift.plan.operations[0]
    {
        *expected_file_hash = ROOT_HASH.to_string();
    }
    assert_eq!(
        validate_edit_plan_against_intent(&hash_drift, &intent)
            .unwrap_err()
            .code,
        "intent.plan_hash_mismatch"
    );

    let mut creation = validated_plan();
    creation.plan.operations = vec![EditOperation::Create {
        path: "src/New.java".to_string(),
        extension: "java".to_string(),
        encoding: TextEncoding::Utf8,
        line_ending: LineEnding::Lf,
        content: "class New {}\n".to_string(),
        reason: "Untrusted creation.".to_string(),
        provenance: vec!["fixture".to_string()],
        expected_absent: true,
        declared_size: "class New {}\n".len(),
    }];
    creation.files = vec![ProposalFileSnapshot {
        path: "src/New.java".to_string(),
        status: ProposalFileStatus::Created,
        encoding: TextEncoding::Utf8,
        line_ending: LineEnding::Lf,
        base_content: None,
        base_hash: None,
        proposed_content: "class New {}\n".to_string(),
        proposed_hash: opticcode_edit::content_hash(b"class New {}\n"),
        proposed_bytes: "class New {}\n".len(),
    }];
    assert_eq!(
        validate_edit_plan_against_intent(&creation, &intent)
            .unwrap_err()
            .code,
        "intent.plan_target_not_allowed"
    );
}

#[test]
fn stored_binding_rejects_tampering() {
    let intent = validated_intent();
    let mut binding = opticcode_edit::ProposalIntentBinding::from_validated(&intent).unwrap();
    binding.intent_hash = ROOT_HASH.to_string();
    assert_eq!(binding.validate().unwrap_err().code, "intent.binding_hash");
}

fn validated_intent() -> ValidatedEditIntent {
    let limits = runtime_limits();
    let expected = EditIntentExpectations {
        request_id: "request-bound".to_string(),
        workspace_id: "workspace-bound".to_string(),
        workspace_root_hash: ROOT_HASH.to_string(),
        base_head: BASE_HEAD.to_string(),
        working_tree_digest: TREE_DIGEST.to_string(),
        now_unix_ms: NOW,
        limits: EditPlanLimits::default(),
        allowed_existing_targets: vec![EditIntentAllowedExistingTarget {
            path: "src/Example.java".to_string(),
            content_hash: FILE_HASH.to_string(),
            reference_ids: vec!["ref-example".to_string()],
        }],
        allowed_create_targets: Vec::new(),
    };
    validate_edit_intent(
        EditIntent {
            schema_version: EDIT_INTENT_SCHEMA_VERSION,
            intent_id: "intent-bound".to_string(),
            request_id: expected.request_id.clone(),
            workspace_id: expected.workspace_id.clone(),
            workspace_root_hash: expected.workspace_root_hash.clone(),
            base_head: expected.base_head.clone(),
            working_tree_digest: expected.working_tree_digest.clone(),
            task: "Update the selected fixture.".to_string(),
            selection_mode: EditIntentSelectionMode::ExplicitReferences,
            targets: vec![EditIntentTarget::ExistingFile {
                path: "src/Example.java".to_string(),
                content_hash: FILE_HASH.to_string(),
                reference_ids: vec!["ref-example".to_string()],
                provenance: EditIntentTargetProvenance::UserReference,
            }],
            constraints: EditIntentConstraints::modify_only(limits),
            created_at_unix_ms: NOW,
            expires_at_unix_ms: NOW + 10 * 60 * 1_000,
        },
        &expected,
    )
    .unwrap()
}

fn plan_expectations() -> EditPlanExpectations {
    EditPlanExpectations {
        request_id: "request-bound".to_string(),
        plan_id: "plan-bound".to_string(),
        workspace_id: "workspace-bound".to_string(),
        workspace_root: PathBuf::from("fixture"),
        workspace_root_hash: ROOT_HASH.to_string(),
        profile: "minecraft-java-1.8".to_string(),
        provider: ProviderId::Ollama,
        model: "fixture".to_string(),
        base_head: BASE_HEAD.to_string(),
        working_tree_digest: TREE_DIGEST.to_string(),
        now_unix_ms: NOW,
        limits: EditPlanLimits::default(),
    }
}

fn validated_plan() -> ValidatedEditPlan {
    let base = "class Example { int value = 1; }\n".to_string();
    let proposed = "class Example { int value = 2; }\n".to_string();
    let start = base.find('1').unwrap();
    ValidatedEditPlan {
        plan: EditPlan {
            schema_version: 1,
            plan_id: "plan-bound".to_string(),
            request_id: "request-bound".to_string(),
            workspace_id: "workspace-bound".to_string(),
            workspace_root_hash: ROOT_HASH.to_string(),
            profile: "minecraft-java-1.8".to_string(),
            provider: ProviderId::Ollama,
            model: "fixture".to_string(),
            base_head: BASE_HEAD.to_string(),
            working_tree_digest: TREE_DIGEST.to_string(),
            context_used: Vec::new(),
            user_references: Vec::new(),
            summary: "Update the fixture.".to_string(),
            rationale_summary: "The selected literal is trusted.".to_string(),
            operations: vec![EditOperation::Modify {
                path: "src/Example.java".to_string(),
                expected_file_hash: FILE_HASH.to_string(),
                encoding: TextEncoding::Utf8,
                line_ending: LineEnding::Lf,
                range: ByteRange {
                    start,
                    end: start + 1,
                },
                expected_old: "1".to_string(),
                replacement: "2".to_string(),
                reason: "Apply the requested fixture change.".to_string(),
                symbol: Some("Example.value".to_string()),
                provenance: vec!["ref-example".to_string()],
            }],
            validations: vec![
                EditValidationKind::ReparseJava,
                EditValidationKind::BuildOffline,
            ],
            risks: Vec::new(),
            limitations: Vec::new(),
            limits: runtime_limits(),
            expires_at_unix_ms: NOW + 60 * 60 * 1_000,
        },
        files: vec![ProposalFileSnapshot {
            path: "src/Example.java".to_string(),
            status: ProposalFileStatus::Modified,
            encoding: TextEncoding::Utf8,
            line_ending: LineEnding::Lf,
            base_content: Some(base),
            base_hash: Some(FILE_HASH.to_string()),
            proposed_content: proposed.clone(),
            proposed_hash: opticcode_edit::content_hash(proposed.as_bytes()),
            proposed_bytes: proposed.len(),
        }],
        estimated_added_lines: 1,
        estimated_deleted_lines: 1,
        total_snapshot_bytes: 72,
    }
}

fn trusted_file() -> TrustedEditFile {
    let content = "class Example { int value = 1; }\n";
    TrustedEditFile {
        path: "src/Example.java".to_string(),
        content_hash: FILE_HASH.to_string(),
        bytes: content.len(),
        line_ending: LineEnding::Lf,
        line_anchors: vec![TrustedEditLine {
            start: 0,
            end: content.len(),
            content: content.to_string(),
        }],
    }
}

fn runtime_limits() -> EditPlanLimits {
    EditPlanLimits {
        max_created_files: 0,
        ..EditPlanLimits::default()
    }
}
