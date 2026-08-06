use std::fs;
use std::path::PathBuf;

use opticcode_edit::{
    parse_edit_intent_json, validate_edit_intent, EditIntent, EditIntentAllowedCreateTarget,
    EditIntentAllowedExistingTarget, EditIntentConstraints, EditIntentExpectations,
    EditIntentOperationKind, EditIntentSelectionMode, EditIntentTarget, EditIntentTargetProvenance,
    EditOffsetEncoding, EditPlanLimits, EDIT_INTENT_SCHEMA_VERSION,
};

const NOW: u64 = 1_800_000_000_000;
const ROOT_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TREE_DIGEST: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const BASE_HEAD: &str = "cccccccccccccccccccccccccccccccccccccccc";
const EXISTING_HASH: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

#[test]
fn intent_validation_is_bounded_bound_and_canonical() {
    let expected = expectations();
    let first = validate_edit_intent(intent(), &expected).unwrap();

    let mut reordered = intent();
    reordered.targets.reverse();
    reordered.constraints.allowed_operations.reverse();
    reordered.constraints.allowed_extensions.reverse();
    for target in &mut reordered.targets {
        match target {
            EditIntentTarget::ExistingFile { reference_ids, .. }
            | EditIntentTarget::ProspectiveFile { reference_ids, .. } => {
                reference_ids.reverse();
            }
        }
    }
    let second = validate_edit_intent(reordered, &expected).unwrap();

    assert_eq!(first.intent_hash, second.intent_hash);
    assert_eq!(first.canonical_json, second.canonical_json);
    assert_eq!(first.intent.schema_version, EDIT_INTENT_SCHEMA_VERSION);
    assert_eq!(
        first.intent.targets[0].path(),
        "src/main/java/dev/test/Example.java"
    );
    assert_eq!(
        first.intent.targets[1].path(),
        "src/main/java/dev/test/NewFeature.java"
    );
    assert_eq!(
        first.intent.constraints.allowed_operations,
        vec![
            EditIntentOperationKind::ModifyExisting,
            EditIntentOperationKind::CreateTextFile
        ]
    );
    assert_eq!(first.intent_hash.len(), 64);
}

#[test]
fn parser_rejects_unknown_fields_and_trailing_text() {
    let value = serde_json::to_value(intent()).unwrap();
    let mut object = value.as_object().unwrap().clone();
    object.insert("unexpected".to_string(), serde_json::json!(true));

    let error = parse_edit_intent_json(&serde_json::to_string(&object).unwrap()).unwrap_err();
    assert_eq!(error.code, "intent.invalid_json");

    let raw = format!("{}\nnot-json", serde_json::to_string(&intent()).unwrap());
    let error = parse_edit_intent_json(&raw).unwrap_err();
    assert_eq!(error.code, "intent.trailing_text");
}

#[test]
fn validation_rejects_untrusted_target_hash_reference_and_path() {
    let expected = expectations();

    let mut bad_hash = intent();
    if let EditIntentTarget::ExistingFile { content_hash, .. } = &mut bad_hash.targets[0] {
        *content_hash = ROOT_HASH.to_string();
    }
    assert_eq!(
        validate_edit_intent(bad_hash, &expected).unwrap_err().code,
        "intent.target_hash_mismatch"
    );

    let mut bad_reference = intent();
    if let EditIntentTarget::ExistingFile { reference_ids, .. } = &mut bad_reference.targets[0] {
        reference_ids.push("ref-untrusted".to_string());
    }
    assert_eq!(
        validate_edit_intent(bad_reference, &expected)
            .unwrap_err()
            .code,
        "intent.reference_not_trusted"
    );

    let mut bad_path = intent();
    if let EditIntentTarget::ExistingFile { path, .. } = &mut bad_path.targets[0] {
        *path = "../outside.java".to_string();
    }
    assert_eq!(
        validate_edit_intent(bad_path, &expected).unwrap_err().code,
        "intent.path"
    );
}

#[test]
fn validation_rejects_duplicates_expiry_and_limit_escalation() {
    let expected = expectations();

    let mut duplicate = intent();
    duplicate.targets.push(duplicate.targets[0].clone());
    assert_eq!(
        validate_edit_intent(duplicate, &expected).unwrap_err().code,
        "intent.target_duplicate"
    );

    let mut expired = intent();
    expired.expires_at_unix_ms = NOW;
    assert_eq!(
        validate_edit_intent(expired, &expected).unwrap_err().code,
        "intent.expiry"
    );

    let mut escalation = intent();
    escalation.constraints.limits.max_files += 1;
    assert_eq!(
        validate_edit_intent(escalation, &expected)
            .unwrap_err()
            .code,
        "intent.limit_escalation"
    );
}

#[test]
fn validation_requires_operations_to_match_declared_targets() {
    let expected = expectations();

    let mut missing_create_permission = intent();
    missing_create_permission
        .constraints
        .allowed_operations
        .retain(|operation| *operation != EditIntentOperationKind::CreateTextFile);
    missing_create_permission
        .constraints
        .limits
        .max_created_files = 0;
    assert_eq!(
        validate_edit_intent(missing_create_permission, &expected)
            .unwrap_err()
            .code,
        "intent.target_operation"
    );

    let mut no_create_target = intent();
    no_create_target
        .targets
        .retain(|target| matches!(target, EditIntentTarget::ExistingFile { .. }));
    assert_eq!(
        validate_edit_intent(no_create_target, &expected)
            .unwrap_err()
            .code,
        "intent.operation_targets"
    );
}

#[test]
fn published_json_schema_is_present_and_versioned() {
    let schema_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schemas/edit-intent.v1.schema.json");
    let raw = fs::read_to_string(&schema_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", schema_path.display()));
    let schema: serde_json::Value = serde_json::from_str(&raw).unwrap();

    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    assert_eq!(schema["properties"]["schema_version"]["const"], 1);
    assert_eq!(schema["additionalProperties"], false);
}

fn expectations() -> EditIntentExpectations {
    EditIntentExpectations {
        request_id: "request-edit-002".to_string(),
        workspace_id: "workspace-edit-002".to_string(),
        workspace_root_hash: ROOT_HASH.to_string(),
        base_head: BASE_HEAD.to_string(),
        working_tree_digest: TREE_DIGEST.to_string(),
        now_unix_ms: NOW,
        limits: EditPlanLimits::default(),
        allowed_existing_targets: vec![EditIntentAllowedExistingTarget {
            path: "src/main/java/dev/test/Example.java".to_string(),
            content_hash: EXISTING_HASH.to_string(),
            reference_ids: vec!["ref-example".to_string(), "ref-symbol".to_string()],
        }],
        allowed_create_targets: vec![EditIntentAllowedCreateTarget {
            path: "src/main/java/dev/test/NewFeature.java".to_string(),
            reference_ids: vec!["ref-create".to_string()],
        }],
    }
}

fn intent() -> EditIntent {
    EditIntent {
        schema_version: EDIT_INTENT_SCHEMA_VERSION,
        intent_id: "intent-edit-protocol-002".to_string(),
        request_id: "request-edit-002".to_string(),
        workspace_id: "workspace-edit-002".to_string(),
        workspace_root_hash: ROOT_HASH.to_string(),
        base_head: BASE_HEAD.to_string(),
        working_tree_digest: TREE_DIGEST.to_string(),
        task: "Update the selected Java behavior and add the explicitly requested helper file."
            .to_string(),
        selection_mode: EditIntentSelectionMode::Hybrid,
        targets: vec![
            EditIntentTarget::ExistingFile {
                path: "src/main/java/dev/test/Example.java".to_string(),
                content_hash: EXISTING_HASH.to_string(),
                reference_ids: vec!["ref-symbol".to_string(), "ref-example".to_string()],
                provenance: EditIntentTargetProvenance::UserReference,
            },
            EditIntentTarget::ProspectiveFile {
                path: "src/main/java/dev/test/NewFeature.java".to_string(),
                extension: "java".to_string(),
                reference_ids: vec!["ref-create".to_string()],
                provenance: EditIntentTargetProvenance::UserRequestedCreation,
            },
        ],
        constraints: EditIntentConstraints {
            allowed_operations: vec![
                EditIntentOperationKind::CreateTextFile,
                EditIntentOperationKind::ModifyExisting,
            ],
            allowed_extensions: vec!["json".to_string(), "java".to_string()],
            limits: EditPlanLimits::default(),
            offset_encoding: EditOffsetEncoding::Utf8Bytes,
            require_clean_worktree: true,
            require_offline_verification: true,
            require_native_confirmation: true,
        },
        created_at_unix_ms: NOW,
        expires_at_unix_ms: NOW + 10 * 60 * 1_000,
    }
}
