use std::fs;
use std::path::Path;
use std::process::Command;

use opticcode_edit::{
    apply_verified_proposal, canonical_root_hash, content_hash, rollback_edit_proposal,
    unix_millis, validate_edit_plan, verify_edit_proposal, working_tree_digest, ByteRange,
    EditContextReference, EditOperation, EditPlan, EditPlanExpectations, EditPlanLimits,
    EditRuntimeOptions, EditValidationKind, LineEnding, ProposalState, ProposalStore, TextEncoding,
};
use opticcode_llm::ProviderId;
use opticcode_policy::{NativeConfirmation, PolicyEngine};
use opticcode_tools::git_state::capture_git_state;
use opticcode_tools::process_runner::CancellationToken;

#[test]
fn verified_edit_applies_and_rolls_back_with_exact_native_approvals() {
    if !maven_available() {
        eprintln!("skipping real offline Maven edit test because mvn is unavailable");
        return;
    }
    let workspace = tempfile::tempdir().unwrap();
    initialize_project(workspace.path());
    let workspace_root = fs::canonicalize(workspace.path()).unwrap();
    let source_path = workspace_root.join("src/main/java/dev/test/Example.java");
    let base = fs::read_to_string(&source_path).unwrap();
    let proposed = base.replace("return 1;", "return 2;");
    let start = base.find("return 1;").unwrap();
    let end = start + "return 1;".len();
    let head = git_output(&workspace_root, &["rev-parse", "HEAD"]);
    let root_hash = canonical_root_hash(&workspace_root).unwrap();
    let state = capture_git_state(&workspace_root).unwrap();
    let digest = working_tree_digest(&root_hash, &head, &serde_json::to_vec(&state).unwrap());
    let now = unix_millis();
    let expectations = EditPlanExpectations {
        request_id: "plan-request".to_string(),
        plan_id: "proposal-e2e".to_string(),
        workspace_id: "workspace-e2e".to_string(),
        workspace_root: workspace_root.clone(),
        workspace_root_hash: root_hash.clone(),
        profile: "minecraft-java-1.8".to_string(),
        provider: ProviderId::Ollama,
        model: "fixture-model".to_string(),
        base_head: head.clone(),
        working_tree_digest: digest.clone(),
        now_unix_ms: now,
        limits: EditPlanLimits::default(),
    };
    let plan = EditPlan {
        schema_version: 1,
        plan_id: "proposal-e2e".to_string(),
        request_id: "plan-request".to_string(),
        workspace_id: "workspace-e2e".to_string(),
        workspace_root_hash: root_hash.clone(),
        profile: "minecraft-java-1.8".to_string(),
        provider: ProviderId::Ollama,
        model: "fixture-model".to_string(),
        base_head: head,
        working_tree_digest: digest,
        context_used: vec![EditContextReference {
            source: "src/main/java/dev/test/Example.java".to_string(),
            provenance: "user_reference".to_string(),
            content_hash: Some(content_hash(base.as_bytes())),
        }],
        user_references: Vec::new(),
        summary: "Return the requested fixture value.".to_string(),
        rationale_summary: "The selected method contains the exact requested literal.".to_string(),
        operations: vec![EditOperation::Modify {
            path: "src/main/java/dev/test/Example.java".to_string(),
            expected_file_hash: content_hash(base.as_bytes()),
            encoding: TextEncoding::Utf8,
            line_ending: LineEnding::Lf,
            range: ByteRange { start, end },
            expected_old: "return 1;".to_string(),
            replacement: "return 2;".to_string(),
            reason: "Apply the requested behavior.".to_string(),
            symbol: Some("dev.test.Example.value".to_string()),
            provenance: vec!["user_reference".to_string()],
        }],
        validations: vec![
            EditValidationKind::ReparseJava,
            EditValidationKind::BuildOffline,
            EditValidationKind::TestOffline,
        ],
        risks: vec!["The method returns a different fixture value.".to_string()],
        limitations: vec!["This validates the bounded edit pipeline.".to_string()],
        limits: EditPlanLimits::default(),
        expires_at_unix_ms: now + 60 * 60 * 1_000,
    };
    let validated = validate_edit_plan(plan, &expectations).unwrap();
    assert_eq!(validated.files[0].proposed_content, proposed);

    let policy_state = tempfile::tempdir().unwrap();
    let proposal_state = tempfile::tempdir().unwrap();
    let store = ProposalStore::open(proposal_state.path(), &root_hash).unwrap();
    store.create(validated).unwrap();
    let policy = PolicyEngine::open(policy_state.path()).unwrap();
    let cancellation = CancellationToken::new();

    let verified = verify_edit_proposal(
        &store,
        "proposal-e2e",
        &policy,
        &runtime_options(&workspace_root, "verify-request"),
        &cancellation,
    )
    .unwrap();
    assert_eq!(
        verified.state,
        ProposalState::Verified,
        "verification report: {:#?}",
        verified.verification
    );
    assert!(verified.verification.as_ref().unwrap().success);
    assert_eq!(fs::read_to_string(&source_path).unwrap(), base);

    let applied = apply_verified_proposal(
        &store,
        "proposal-e2e",
        &policy,
        &runtime_options(&workspace_root, "apply-request"),
        &NativeConfirmation::explicit("vscode", "apply-confirmation-e2e").unwrap(),
        &cancellation,
    )
    .unwrap();
    assert_eq!(applied.state, ProposalState::RollbackAvailable);
    assert_eq!(fs::read_to_string(&source_path).unwrap(), proposed);

    let rolled_back = rollback_edit_proposal(
        &store,
        "proposal-e2e",
        &policy,
        &runtime_options(&workspace_root, "rollback-request"),
        &NativeConfirmation::explicit("vscode", "rollback-confirmation-e2e").unwrap(),
        &cancellation,
    )
    .unwrap();
    assert_eq!(rolled_back.state, ProposalState::RolledBack);
    assert_eq!(fs::read_to_string(&source_path).unwrap(), base);

    let repeated = rollback_edit_proposal(
        &store,
        "proposal-e2e",
        &policy,
        &runtime_options(&workspace_root, "rollback-repeat"),
        &NativeConfirmation::explicit("vscode", "rollback-confirmation-repeat").unwrap(),
        &cancellation,
    )
    .unwrap();
    assert_eq!(repeated.state, ProposalState::RolledBack);
}

fn runtime_options(root: &Path, request_id: &str) -> EditRuntimeOptions {
    EditRuntimeOptions::new(root, "workspace-e2e", request_id, "minecraft-java-1.8")
}

fn initialize_project(root: &Path) {
    fs::create_dir_all(root.join("src/main/java/dev/test")).unwrap();
    fs::write(
        root.join("src/main/java/dev/test/Example.java"),
        "package dev.test;\npublic final class Example {\n    public int value() { return 1; }\n}\n",
    )
    .unwrap();
    fs::write(
        root.join("pom.xml"),
        concat!(
            "<project xmlns=\"http://maven.apache.org/POM/4.0.0\">\n",
            "  <modelVersion>4.0.0</modelVersion>\n",
            "  <groupId>dev.test</groupId><artifactId>fixture</artifactId><version>1.0</version>\n",
            "  <properties><maven.compiler.source>1.8</maven.compiler.source>",
            "<maven.compiler.target>1.8</maven.compiler.target></properties>\n",
            "</project>\n"
        ),
    )
    .unwrap();
    fs::write(
        root.join(".gitignore"),
        "target/\n.gradle/\nbuild/\n.opticcode/\n",
    )
    .unwrap();
    git(root, &["init", "--quiet"]);
    git(root, &["add", "--all"]);
    git(
        root,
        &[
            "-c",
            "user.name=OpticCode Test",
            "-c",
            "user.email=opticcode-test@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ],
    );
}

fn maven_available() -> bool {
    Command::new(if cfg!(windows) { "where.exe" } else { "which" })
        .arg(if cfg!(windows) { "mvn.cmd" } else { "mvn" })
        .output()
        .is_ok_and(|output| output.status.success())
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}
