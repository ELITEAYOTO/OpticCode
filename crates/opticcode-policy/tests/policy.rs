use std::fs;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::Command;
use std::sync::{Arc, Barrier};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use opticcode_policy::{
    ActionOrigin, ActiveWorktree, ApplyPatchAction, ApprovalBinding, ApprovalError,
    ApprovalFileBinding, ApprovalState, ApprovalStore, AuditEvent, AuditQuery, AuditStore,
    CreateWorktreeAction, GitReadAction, GitReadOperation, GitRepositoryBoundary, GitWriteAction,
    GitWriteOperation, NativeConfirmation, NetworkIntent, PathTarget, PolicyAction, PolicyEngine,
    PolicyMode, PolicyRequest, PolicyWorkspace, ProcessLaunch, RunProcessAction,
    POLICY_PROTOCOL_ID, POLICY_SCHEMA_VERSION,
};
use tempfile::TempDir;

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

struct Fixture {
    workspace: TempDir,
    _state: TempDir,
    engine: PolicyEngine,
}

impl Fixture {
    fn new() -> Self {
        let workspace = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        fs::create_dir_all(workspace.path().join("src")).unwrap();
        fs::write(workspace.path().join("src/Main.java"), "class Main {}\n").unwrap();
        let engine = PolicyEngine::open(state.path()).unwrap();
        Self {
            workspace,
            _state: state,
            engine,
        }
    }

    fn request(&self, action: PolicyAction, mode: PolicyMode) -> PolicyRequest {
        PolicyRequest {
            schema_version: POLICY_SCHEMA_VERSION,
            protocol: POLICY_PROTOCOL_ID.to_string(),
            request_id: "request-001".to_string(),
            action_id: "action-001".to_string(),
            origin: ActionOrigin::Chat,
            profile: "minecraft-java-1.8".to_string(),
            client: opticcode_policy::PolicyClient {
                name: "policy-test".to_string(),
                version: "1.0.0".to_string(),
            },
            mode,
            workspace: PolicyWorkspace {
                workspace_id: "workspace-001".to_string(),
                root: self.workspace.path().to_path_buf(),
                repository: None,
                active_worktree: None,
                working_tree_digest: None,
                repository_clean: None,
            },
            action,
            approval_id: None,
        }
    }

    fn read(&self, path: impl Into<PathBuf>) -> PolicyRequest {
        self.request(
            PolicyAction::ReadFile(PathTarget {
                root: self.workspace.path().to_path_buf(),
                path: path.into(),
                range: None,
                expected_hash: None,
            }),
            PolicyMode::ReadOnly,
        )
    }

    fn observe_clean_repository(&self, request: &mut PolicyRequest) {
        request.workspace.repository = Some(repository_boundary(self.workspace.path(), HASH_A));
        request.workspace.working_tree_digest = Some(HASH_B.to_string());
        request.workspace.repository_clean = Some(true);
    }
}

#[test]
fn safe_read_is_allowed_and_revalidatable() {
    let fixture = Fixture::new();
    let preflight = fixture
        .engine
        .check(&fixture.read("src/Main.java"))
        .unwrap();
    assert!(preflight.report.allowed());
    assert_eq!(
        preflight.report.decision.rule_id(),
        "read.safe_workspace_file"
    );
    preflight.revalidate().unwrap();
    fs::write(fixture.workspace.path().join("src/Main.java"), "changed\n").unwrap();
    assert!(preflight.revalidate().is_err());
}

#[test]
fn schemas_are_deterministic_and_fail_closed_for_unknown_fields_modes_and_actions() {
    let fixture = Fixture::new();
    let request = fixture.read("src/Main.java");
    let first = fixture.engine.explain(&request).unwrap();
    let second = fixture.engine.explain(&request).unwrap();
    assert_eq!(
        serde_json::to_vec(&first.report).unwrap(),
        serde_json::to_vec(&second.report).unwrap()
    );

    let mut unknown_field = serde_json::to_value(&request).unwrap();
    unknown_field["unexpected_authority"] = serde_json::json!(true);
    assert!(serde_json::from_value::<PolicyRequest>(unknown_field).is_err());

    let mut unknown_mode = serde_json::to_value(&request).unwrap();
    unknown_mode["mode"] = serde_json::json!("unrestricted");
    assert!(serde_json::from_value::<PolicyRequest>(unknown_mode).is_err());

    let mut unknown_action = serde_json::to_value(&request).unwrap();
    unknown_action["action"] = serde_json::json!({"type": "future_untrusted_action"});
    let unknown = serde_json::from_value::<PolicyRequest>(unknown_action).unwrap();
    assert!(matches!(unknown.action, PolicyAction::Unknown));
    let report = fixture.engine.explain(&unknown).unwrap();
    assert_eq!(report.report.decision.rule_id(), "action.unknown");

    let mut unknown_payload = serde_json::to_value(&request).unwrap();
    unknown_payload["action"] = serde_json::json!({
        "type": "future_untrusted_action",
        "data": {"opaque": true}
    });
    assert!(serde_json::from_value::<PolicyRequest>(unknown_payload).is_err());
}

#[test]
fn safe_paths_with_spaces_unicode_and_windows_case_are_confined_structurally() {
    let fixture = Fixture::new();
    let relative = "src/\u{00c9}te Plugins/Classe.java";
    fs::create_dir_all(fixture.workspace.path().join("src/\u{00c9}te Plugins")).unwrap();
    fs::write(fixture.workspace.path().join(relative), "class Classe {}\n").unwrap();
    let report = fixture.engine.check(&fixture.read(relative)).unwrap();
    assert!(report.report.allowed());

    #[cfg(windows)]
    {
        let root = fixture
            .workspace
            .path()
            .to_string_lossy()
            .to_ascii_uppercase();
        let mut request = fixture.read(relative);
        request.workspace.root = PathBuf::from(&root);
        if let PolicyAction::ReadFile(target) = &mut request.action {
            target.root = PathBuf::from(root);
        }
        assert!(fixture.engine.check(&request).unwrap().report.allowed());
    }
}

#[test]
fn secret_and_outside_reads_are_denied() {
    let fixture = Fixture::new();
    fs::write(fixture.workspace.path().join(".env"), "TOKEN=secret-value").unwrap();
    let secret = fixture.engine.check(&fixture.read(".env")).unwrap();
    assert_eq!(secret.report.decision.kind(), "deny");
    assert_eq!(secret.report.decision.rule_id(), "path.sensitive");
    fs::write(
        fixture.workspace.path().join(".env.development"),
        "VALUE=test-only",
    )
    .unwrap();
    assert_eq!(
        fixture
            .engine
            .check(&fixture.read(".env.development"))
            .unwrap()
            .report
            .decision
            .rule_id(),
        "path.sensitive"
    );
    fs::write(
        fixture.workspace.path().join("src/SecretService.java"),
        "class SecretService {}\n",
    )
    .unwrap();
    assert!(fixture
        .engine
        .check(&fixture.read("src/SecretService.java"))
        .unwrap()
        .report
        .allowed());

    let outside = tempfile::NamedTempFile::new().unwrap();
    let denied = fixture
        .engine
        .check(&fixture.read(outside.path().to_path_buf()))
        .unwrap();
    assert_eq!(denied.report.decision.rule_id(), "path.outside_root");
}

#[test]
fn nested_repository_and_submodule_are_denied() {
    let fixture = Fixture::new();
    let nested = fixture.workspace.path().join("nested");
    fs::create_dir_all(nested.join(".git")).unwrap();
    fs::write(nested.join("A.java"), "class A {}").unwrap();
    let nested_report = fixture
        .engine
        .check(&fixture.read("nested/A.java"))
        .unwrap();
    assert_eq!(
        nested_report.report.decision.rule_id(),
        "git.nested_repository"
    );

    let submodule = fixture.workspace.path().join("module");
    fs::create_dir_all(&submodule).unwrap();
    fs::write(submodule.join(".git"), "gitdir: ../.git/modules/module").unwrap();
    fs::write(submodule.join("B.java"), "class B {}").unwrap();
    let submodule_report = fixture
        .engine
        .check(&fixture.read("module/B.java"))
        .unwrap();
    assert_eq!(
        submodule_report.report.decision.rule_id(),
        "git.nested_repository"
    );
}

#[cfg(windows)]
#[test]
fn junction_and_reparse_root_are_denied() {
    let fixture = Fixture::new();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("Outside.java"), "class Outside {}").unwrap();
    let junction = fixture.workspace.path().join("linked");
    create_junction(&junction, outside.path());
    let report = fixture
        .engine
        .check(&fixture.read("linked/Outside.java"))
        .unwrap();
    assert_eq!(report.report.decision.rule_id(), "path.symlink_or_reparse");

    let mut root_request = fixture.read("Outside.java");
    root_request.workspace.root = junction.clone();
    if let PolicyAction::ReadFile(target) = &mut root_request.action {
        target.root = junction.clone();
    }
    assert!(fixture.engine.check(&root_request).is_err());
    fs::remove_dir(&junction).unwrap();
}

#[cfg(unix)]
#[test]
fn symlink_is_denied() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    symlink(
        fixture.workspace.path().join("src"),
        fixture.workspace.path().join("linked"),
    )
    .unwrap();
    let report = fixture
        .engine
        .check(&fixture.read("linked/Main.java"))
        .unwrap();
    assert_eq!(report.report.decision.rule_id(), "path.symlink_or_reparse");
}

#[test]
fn original_write_is_denied_in_read_only_and_without_apply() {
    let fixture = Fixture::new();
    let action = PolicyAction::WriteFile(PathTarget {
        root: fixture.workspace.path().to_path_buf(),
        path: PathBuf::from("src/Main.java"),
        range: None,
        expected_hash: None,
    });
    let read_only = fixture
        .engine
        .check(&fixture.request(action.clone(), PolicyMode::ReadOnly))
        .unwrap();
    assert_eq!(
        read_only.report.decision.rule_id(),
        "mode.read_only_write_denied"
    );
    let edit = fixture
        .engine
        .check(&fixture.request(action, PolicyMode::WorktreeEdit))
        .unwrap();
    assert_eq!(
        edit.report.decision.rule_id(),
        "write.original_requires_apply"
    );
}

#[test]
fn approved_apply_requires_clean_fresh_git_state_and_exact_metadata_boundaries() {
    let fixture = Fixture::new();
    let mut request = apply_request(&fixture);
    request.workspace.repository_clean = Some(false);
    assert_eq!(
        fixture
            .engine
            .explain(&request)
            .unwrap()
            .report
            .decision
            .rule_id(),
        "apply.repository_dirty"
    );

    request.workspace.repository_clean = Some(true);
    request.workspace.working_tree_digest = None;
    assert_eq!(
        fixture
            .engine
            .explain(&request)
            .unwrap()
            .report
            .decision
            .rule_id(),
        "git.working_tree_unobserved"
    );

    let git_dir = fixture.workspace.path().join(".git");
    fs::create_dir_all(git_dir.join("alternate")).unwrap();
    fs::write(git_dir.join("alternate/index"), b"").unwrap();
    request.workspace.working_tree_digest = Some(HASH_B.to_string());
    request.workspace.repository.as_mut().unwrap().index = git_dir.join("alternate/index");
    assert!(fixture.engine.explain(&request).is_err());

    let mut objects = apply_request(&fixture);
    fs::create_dir_all(git_dir.join("objects/alternate")).unwrap();
    objects.workspace.repository.as_mut().unwrap().object_dir = git_dir.join("objects/alternate");
    assert!(fixture.engine.explain(&objects).is_err());

    let git_target = fixture.engine.check(&fixture.read(".git/index")).unwrap();
    assert_eq!(git_target.report.decision.rule_id(), "path.sensitive");
}

#[test]
fn worktree_git_file_and_lease_authorize_scoped_write() {
    let fixture = Fixture::new();
    let worktree = ActiveFixture::new(fixture.workspace.path());
    fs::create_dir_all(worktree.root.join("src")).unwrap();
    fs::write(worktree.root.join("src/Main.java"), "class Main {}\n").unwrap();
    let mut request = fixture.request(
        PolicyAction::WriteFile(PathTarget {
            root: worktree.root.clone(),
            path: PathBuf::from("src/Main.java"),
            range: None,
            expected_hash: None,
        }),
        PolicyMode::WorktreeEdit,
    );
    worktree.attach(&mut request);
    let report = fixture.engine.check(&request).unwrap();
    assert!(report.report.allowed(), "{:?}", report.report.decision);
    assert_eq!(report.report.decision.rule_id(), "write.active_worktree");
}

#[test]
fn fake_worktree_without_lease_is_denied() {
    let fixture = Fixture::new();
    let worktree = ActiveFixture::new_without_lease(fixture.workspace.path());
    fs::create_dir_all(worktree.root.join("src")).unwrap();
    fs::write(worktree.root.join("src/Main.java"), "class Main {}\n").unwrap();
    let mut request = fixture.request(
        PolicyAction::WriteFile(PathTarget {
            root: worktree.root.clone(),
            path: PathBuf::from("src/Main.java"),
            range: None,
            expected_hash: None,
        }),
        PolicyMode::WorktreeEdit,
    );
    worktree.attach(&mut request);
    let report = fixture.engine.check(&request).unwrap();
    assert_eq!(report.report.decision.rule_id(), "worktree.lease_missing");
}

#[test]
fn worktree_owner_source_state_and_commondir_are_revalidated() {
    let fixture = Fixture::new();
    let worktree = ActiveFixture::new(fixture.workspace.path());
    fs::create_dir_all(worktree.root.join("src")).unwrap();
    fs::write(worktree.root.join("src/Main.java"), "class Main {}\n").unwrap();
    let mut request = fixture.request(
        PolicyAction::WriteFile(PathTarget {
            root: worktree.root.clone(),
            path: PathBuf::from("src/Main.java"),
            range: None,
            expected_hash: None,
        }),
        PolicyMode::WorktreeEdit,
    );
    worktree.attach(&mut request);

    let mut foreign = request.clone();
    foreign
        .workspace
        .active_worktree
        .as_mut()
        .unwrap()
        .owner_request_id = "another-request".to_string();
    assert_eq!(
        fixture
            .engine
            .check(&foreign)
            .unwrap()
            .report
            .decision
            .rule_id(),
        "worktree.owner_mismatch"
    );

    let preflight = fixture.engine.check(&request).unwrap();
    let mut drifted = request.clone();
    drifted.workspace.working_tree_digest = Some("c".repeat(64));
    assert!(preflight.revalidate_observed(&drifted).is_err());

    fs::write(worktree.git_dir.join("commondir"), "..\n").unwrap();
    assert_eq!(
        fixture
            .engine
            .check(&request)
            .unwrap()
            .report
            .decision
            .rule_id(),
        "git.commondir_invalid"
    );
}

#[test]
fn process_allowlist_network_shell_and_metacharacters_are_enforced() {
    let fixture = Fixture::new();
    let worktree = ActiveFixture::new(fixture.workspace.path());
    fs::write(fixture.workspace.path().join("mvnw.cmd"), "@echo off\r\n").unwrap();
    fs::write(worktree.root.join("mvnw.cmd"), "@echo off\r\n").unwrap();
    let mut request = fixture.request(
        PolicyAction::RunProcess(RunProcessAction {
            executable: PathBuf::from("mvnw.cmd"),
            arguments: vec!["--offline".to_string(), "test".to_string()],
            cwd: worktree.root.clone(),
            timeout_ms: 60_000,
            output_limit_bytes: 1024 * 1024,
            network: NetworkIntent::Denied,
            launch: ProcessLaunch::WindowsCommandScript,
            environment_allowlist: vec!["JAVA_HOME".to_string()],
        }),
        PolicyMode::WorktreeEdit,
    );
    worktree.attach(&mut request);
    let allowed = fixture.engine.check(&request).unwrap();
    assert!(allowed.report.allowed(), "{:?}", allowed.report.decision);

    fs::write(worktree.root.join("mvnw.cmd"), "@echo malicious\r\n").unwrap();
    let drifted_wrapper = fixture.engine.check(&request).unwrap();
    assert_eq!(
        drifted_wrapper.report.decision.rule_id(),
        "process.wrapper_drift"
    );
    fs::write(worktree.root.join("mvnw.cmd"), "@echo off\r\n").unwrap();

    if let PolicyAction::RunProcess(process) = &mut request.action {
        process.arguments = vec!["test".to_string(), "&&".to_string(), "whoami".to_string()];
    }
    let metachar = fixture.engine.check(&request).unwrap();
    assert_eq!(
        metachar.report.decision.rule_id(),
        "process.metacharacter_denied"
    );

    if let PolicyAction::RunProcess(process) = &mut request.action {
        process.arguments = vec!["Get-ChildItem".to_string()];
        process.executable = PathBuf::from("powershell.exe");
        process.launch = ProcessLaunch::Shell;
    }
    let shell = fixture.engine.check(&request).unwrap();
    assert_eq!(shell.report.decision.rule_id(), "process.shell_denied");

    if let PolicyAction::RunProcess(process) = &mut request.action {
        process.arguments = vec!["test".to_string()];
        process.executable = PathBuf::from("unknown.exe");
        process.launch = ProcessLaunch::Direct;
    }
    let unknown = fixture.engine.check(&request).unwrap();
    assert_eq!(
        unknown.report.decision.rule_id(),
        "process.executable_unknown"
    );

    if let PolicyAction::RunProcess(process) = &mut request.action {
        process.executable = PathBuf::from("mvnw.cmd");
        process.launch = ProcessLaunch::WindowsCommandScript;
        process.network = NetworkIntent::Undeclared;
    }
    let undeclared = fixture.engine.check(&request).unwrap();
    assert_eq!(undeclared.report.decision.rule_id(), "network.undeclared");

    if let PolicyAction::RunProcess(process) = &mut request.action {
        process.network = NetworkIntent::Denied;
    }
    let denied_without_offline = fixture.engine.check(&request).unwrap();
    assert_eq!(
        denied_without_offline.report.decision.rule_id(),
        "process.offline_required"
    );

    if let PolicyAction::RunProcess(process) = &mut request.action {
        process.network = NetworkIntent::Declared;
    }
    let network = fixture.engine.check(&request).unwrap();
    assert_eq!(network.report.decision.kind(), "require_approval");
}

#[test]
fn direct_arguments_distinguish_data_characters_from_shell_composition() {
    let fixture = Fixture::new();
    let worktree = ActiveFixture::new(fixture.workspace.path());
    fs::write(fixture.workspace.path().join("mvnw"), "#!/bin/sh\n").unwrap();
    fs::write(worktree.root.join("mvnw"), "#!/bin/sh\n").unwrap();
    let mut request = fixture.request(
        PolicyAction::RunProcess(RunProcessAction {
            executable: PathBuf::from("mvnw"),
            arguments: vec![
                "--offline".to_string(),
                "-Durl=https://example.invalid/?a=1&b=2".to_string(),
                "-Dregex=a|b".to_string(),
                "-Dxml=a>b".to_string(),
                "test".to_string(),
            ],
            cwd: worktree.root.clone(),
            timeout_ms: 60_000,
            output_limit_bytes: 1024 * 1024,
            network: NetworkIntent::Denied,
            launch: ProcessLaunch::Direct,
            environment_allowlist: vec!["CI".to_string(), "JAVA_HOME".to_string()],
        }),
        PolicyMode::WorktreeEdit,
    );
    worktree.attach(&mut request);
    assert!(fixture.engine.check(&request).unwrap().report.allowed());

    if let PolicyAction::RunProcess(process) = &mut request.action {
        process.arguments = vec!["--offline".to_string(), "test && whoami".to_string()];
    }
    assert_eq!(
        fixture
            .engine
            .check(&request)
            .unwrap()
            .report
            .decision
            .rule_id(),
        "process.metacharacter_denied"
    );

    if let PolicyAction::RunProcess(process) = &mut request.action {
        process.arguments = vec![
            "--offline".to_string(),
            "--settings".to_string(),
            "outside.xml".to_string(),
            "test".to_string(),
        ];
    }
    assert_eq!(
        fixture
            .engine
            .check(&request)
            .unwrap()
            .report
            .decision
            .rule_id(),
        "process.option_denied"
    );

    if let PolicyAction::RunProcess(process) = &mut request.action {
        process.arguments = vec!["--offline".to_string(), "test".to_string()];
        process.environment_allowlist = vec!["PATH".to_string()];
    }
    assert_eq!(
        fixture
            .engine
            .check(&request)
            .unwrap()
            .report
            .decision
            .rule_id(),
        "process.environment_denied"
    );

    if let PolicyAction::RunProcess(process) = &mut request.action {
        process.environment_allowlist.clear();
        process.executable = PathBuf::from("java.exe");
    }
    assert_eq!(
        fixture
            .engine
            .check(&request)
            .unwrap()
            .report
            .decision
            .rule_id(),
        "process.java_runtime_denied"
    );
}

#[test]
fn package_push_publish_and_unknown_are_denied() {
    let fixture = Fixture::new();
    let actions = vec![
        PolicyAction::PackageInstall {
            ecosystem: "winget".to_string(),
            package_set_hash: HASH_A.to_string(),
        },
        PolicyAction::GitPush(GitWriteAction {
            repository_root: fixture.workspace.path().to_path_buf(),
            operation: GitWriteOperation::Unknown,
            paths: Vec::new(),
        }),
        PolicyAction::Publish {
            target: "marketplace".to_string(),
            artifact_hash: HASH_A.to_string(),
        },
        PolicyAction::Unknown,
    ];
    let expected = [
        "package.install_denied",
        "git.push_denied",
        "publish.denied",
        "action.unknown",
    ];
    for (action, expected_rule) in actions.into_iter().zip(expected) {
        let report = fixture
            .engine
            .check(&fixture.request(action, PolicyMode::ReadOnly))
            .unwrap();
        assert_eq!(report.report.decision.rule_id(), expected_rule);
    }
}

#[test]
fn git_read_is_allowlisted_and_git_write_is_closed() {
    let fixture = Fixture::new();
    let read = fixture.request(
        PolicyAction::GitRead(GitReadAction {
            repository_root: fixture.workspace.path().to_path_buf(),
            operation: GitReadOperation::Status,
            paths: Vec::new(),
        }),
        PolicyMode::ReadOnly,
    );
    assert!(fixture.engine.check(&read).unwrap().report.allowed());
    let write = fixture.request(
        PolicyAction::GitWrite(GitWriteAction {
            repository_root: fixture.workspace.path().to_path_buf(),
            operation: GitWriteOperation::Checkout,
            paths: Vec::new(),
        }),
        PolicyMode::WorktreeEdit,
    );
    assert_eq!(
        fixture
            .engine
            .check(&write)
            .unwrap()
            .report
            .decision
            .rule_id(),
        "git.generic_write_denied"
    );
}

#[test]
fn create_worktree_requires_edit_mode_and_controlled_destination() {
    let fixture = Fixture::new();
    let run_id = unique_id("create");
    let storage = prepare_storage();
    let destination = storage.join("runs").join(&run_id);
    let action = PolicyAction::CreateWorktree(CreateWorktreeAction {
        repository_root: fixture.workspace.path().to_path_buf(),
        destination,
        base_head: HASH_A.to_string(),
        run_id,
        detached: true,
    });
    let mut denied_request = fixture.request(action.clone(), PolicyMode::ReadOnly);
    fixture.observe_clean_repository(&mut denied_request);
    let denied = fixture.engine.check(&denied_request).unwrap();
    assert_eq!(
        denied.report.decision.rule_id(),
        "mode.read_only_write_denied"
    );
    let mut allowed_request = fixture.request(action, PolicyMode::WorktreeEdit);
    fixture.observe_clean_repository(&mut allowed_request);
    let allowed = fixture.engine.check(&allowed_request).unwrap();
    assert!(allowed.report.allowed());

    let mut traversal = fixture.request(
        PolicyAction::CreateWorktree(CreateWorktreeAction {
            repository_root: fixture.workspace.path().to_path_buf(),
            destination: storage.join("runs").join("escape"),
            base_head: HASH_A.to_string(),
            run_id: "../escape".to_string(),
            detached: true,
        }),
        PolicyMode::WorktreeEdit,
    );
    fixture.observe_clean_repository(&mut traversal);
    assert_eq!(
        fixture
            .engine
            .check(&traversal)
            .unwrap()
            .report
            .decision
            .rule_id(),
        "worktree.run_id"
    );
}

#[test]
fn approval_is_one_shot_and_bound_to_every_observed_state() {
    let fixture = Fixture::new();
    let confirmation = NativeConfirmation::explicit("vscode", "modal-001").unwrap();

    let mut request = apply_request(&fixture);
    let grant = fixture
        .engine
        .issue_approval(&request, &confirmation, 60)
        .unwrap();
    request.approval_id = Some(grant.approval_id.clone());
    let accepted = fixture.engine.check(&request).unwrap();
    assert!(accepted.report.allowed());
    assert_eq!(accepted.report.decision.rule_id(), "approval.one_shot");
    let replay = fixture.engine.check(&request).unwrap();
    assert_eq!(replay.report.decision.rule_id(), "approval.reused");

    assert_drift(
        &fixture,
        &confirmation,
        |request| request.workspace.workspace_id = "workspace-other".to_string(),
        "approval.workspace_drift",
    );
    assert_drift(
        &fixture,
        &confirmation,
        |request| {
            request.workspace.repository.as_mut().unwrap().head = HASH_B.to_string();
            if let PolicyAction::ApplyPatch(action) = &mut request.action {
                action.base_head = HASH_B.to_string();
            }
        },
        "approval.head_drift",
    );
    assert_drift(
        &fixture,
        &confirmation,
        |request| {
            if let PolicyAction::ApplyPatch(action) = &mut request.action {
                action.diff_hash = HASH_B.to_string();
            }
        },
        "approval.diff_drift",
    );
    assert_drift(
        &fixture,
        &confirmation,
        |request| {
            fs::write(
                request.workspace.root.join("src/Main.java"),
                "class Main { int changed; }\n",
            )
            .unwrap();
        },
        "approval.files_drift",
    );
    assert_drift(
        &fixture,
        &confirmation,
        |request| {
            fs::write(
                request.workspace.root.join("src/Other.java"),
                "class Other {}\n",
            )
            .unwrap();
            if let PolicyAction::ApplyPatch(action) = &mut request.action {
                action.paths.push(PathBuf::from("src/Other.java"));
            }
        },
        "approval.files_drift",
    );
    assert_drift(
        &fixture,
        &confirmation,
        |request| {
            if let PolicyAction::ApplyPatch(action) = &mut request.action {
                action.transaction_id = "transaction-002".to_string();
            }
        },
        "approval.transaction_drift",
    );
    assert_drift(
        &fixture,
        &confirmation,
        |request| {
            if let PolicyAction::ApplyPatch(action) = &mut request.action {
                action.root = action.root.join(".");
            }
        },
        "approval.actions_drift",
    );
}

#[test]
fn approval_expires() {
    let fixture = Fixture::new();
    let confirmation = NativeConfirmation::explicit("vscode", "modal-expiry").unwrap();
    let mut request = apply_request(&fixture);
    let grant = fixture
        .engine
        .issue_approval(&request, &confirmation, 1)
        .unwrap();
    std::thread::sleep(Duration::from_millis(1_100));
    request.approval_id = Some(grant.approval_id);
    let report = fixture.engine.check(&request).unwrap();
    assert_eq!(report.report.decision.rule_id(), "approval.expired");
}

#[test]
fn approval_consumption_is_atomic_under_concurrency() {
    let state = tempfile::tempdir().unwrap();
    let store = ApprovalStore::open(state.path()).unwrap();
    let binding = standalone_approval_binding();
    let confirmation = NativeConfirmation::explicit("vscode", "concurrent-modal").unwrap();
    let grant = store.issue(binding.clone(), &confirmation, 60).unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let mut handles = Vec::new();
    for _ in 0..2 {
        let store = store.clone();
        let binding = binding.clone();
        let approval_id = grant.approval_id.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            store.consume(&approval_id, &binding)
        }));
    }
    barrier.wait();
    let outcomes = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, Err(ApprovalError::Reused)))
            .count(),
        1
    );
    assert!(outcomes.iter().flatten().all(|grant| {
        grant.state == ApprovalState::Consumed && grant.issued_unix_ms < grant.expires_unix_ms
    }));
}

#[test]
fn approval_binds_request_mode_worktree_order_actions_and_transaction_exactly() {
    assert_approval_binding_drift(
        |binding| binding.request_id = "request-other".to_string(),
        ApprovalError::WrongRequest,
    );
    assert_approval_binding_drift(
        |binding| binding.workspace_id = "workspace-other".to_string(),
        ApprovalError::WrongWorkspace,
    );
    assert_approval_binding_drift(
        |binding| binding.mode = PolicyMode::WorktreeEdit,
        ApprovalError::WrongMode,
    );
    assert_approval_binding_drift(
        |binding| binding.base_head = "b".repeat(40),
        ApprovalError::HeadChanged,
    );
    assert_approval_binding_drift(
        |binding| binding.working_tree_digest = HASH_B.to_string(),
        ApprovalError::WorkingTreeChanged,
    );
    assert_approval_binding_drift(
        |binding| binding.diff_hash = HASH_B.to_string(),
        ApprovalError::DiffChanged,
    );
    assert_approval_binding_drift(
        |binding| binding.files.swap(0, 1),
        ApprovalError::FilesChanged,
    );
    assert_approval_binding_drift(
        |binding| {
            binding.files.pop();
        },
        ApprovalError::FilesChanged,
    );
    assert_approval_binding_drift(
        |binding| {
            binding.files.push(ApprovalFileBinding {
                path_hash: "e".repeat(64),
                expected_hash: "f".repeat(64),
            });
        },
        ApprovalError::FilesChanged,
    );
    assert_approval_binding_drift(
        |binding| binding.action_hashes.swap(0, 1),
        ApprovalError::ActionsChanged,
    );
    assert_approval_binding_drift(
        |binding| binding.action_hashes[0] = "e".repeat(64),
        ApprovalError::ActionsChanged,
    );
    assert_approval_binding_drift(
        |binding| binding.transaction_id = "transaction-other".to_string(),
        ApprovalError::TransactionChanged,
    );
}

#[test]
fn unknown_and_corrupt_approvals_fail_closed_and_are_not_retry_oracles() {
    let state = tempfile::tempdir().unwrap();
    let store = ApprovalStore::open(state.path()).unwrap();
    let binding = standalone_approval_binding();
    assert_eq!(
        store.consume("approval-unknown", &binding),
        Err(ApprovalError::Missing)
    );

    let confirmation = NativeConfirmation::explicit("vscode", "corrupt-modal").unwrap();
    let grant = store.issue(binding.clone(), &confirmation, 60).unwrap();
    let active = state
        .path()
        .join("policy-v1/approvals/active")
        .join(format!("{}.json", grant.approval_id));
    fs::write(active, b"{\"truncated\":").unwrap();
    assert_eq!(
        store.consume(&grant.approval_id, &binding),
        Err(ApprovalError::InvalidRecord)
    );
    assert_eq!(
        store.consume(&grant.approval_id, &binding),
        Err(ApprovalError::Reused)
    );
}

#[test]
fn audit_contains_hashes_not_secret_content_and_skips_partial_records() {
    let fixture = Fixture::new();
    let secret_name = ".env-super-secret";
    fs::write(
        fixture.workspace.path().join(secret_name),
        "TOKEN=ultra-secret",
    )
    .unwrap();
    let _ = fixture.engine.check(&fixture.read(secret_name)).unwrap();
    let report = fixture
        .engine
        .audit_store()
        .list(&AuditQuery {
            limit: 20,
            ..AuditQuery::default()
        })
        .unwrap();
    assert_eq!(report.events.len(), 1);
    let encoded = serde_json::to_string(&report.events).unwrap();
    assert!(!encoded.contains(secret_name));
    assert!(!encoded.contains("ultra-secret"));

    let workspace_hash = report.events[0].workspace_hash.clone();
    let scoped = fixture
        .engine
        .audit_store()
        .list(&AuditQuery {
            limit: 20,
            workspace_hash: Some(workspace_hash.clone()),
            action_kind: None,
        })
        .unwrap();

    fs::write(
        scoped.storage.join("99999999999999999999-partial.json"),
        b"{",
    )
    .unwrap();
    fs::write(scoped.storage.join(".interrupted.tmp"), b"{").unwrap();
    let after = fixture
        .engine
        .audit_store()
        .list(&AuditQuery {
            limit: 20,
            workspace_hash: Some(workspace_hash),
            action_kind: None,
        })
        .unwrap();
    assert_eq!(after.events.len(), 1);
    assert_eq!(after.ignored_partial_records, 1);
}

#[test]
fn audit_order_is_deterministic() {
    let state = tempfile::tempdir().unwrap();
    let store = AuditStore::open(state.path()).unwrap();
    for (id, timestamp) in [("event-b", 2), ("event-a", 1), ("event-c", 2)] {
        store.record(audit_event(id, timestamp)).unwrap();
    }
    let report = store
        .list(&AuditQuery {
            limit: 10,
            ..AuditQuery::default()
        })
        .unwrap();
    let ids = report
        .events
        .iter()
        .map(|event| event.event_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["event-a", "event-b", "event-c"]);
}

#[test]
fn audit_is_concurrent_namespaced_and_refuses_cross_workspace_scope() {
    let state = tempfile::tempdir().unwrap();
    let store = Arc::new(AuditStore::open(state.path()).unwrap());
    let barrier = Arc::new(Barrier::new(9));
    let mut handles = Vec::new();
    for index in 0..8 {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            let mut event = audit_event(&format!("event-concurrent-{index}"), index);
            event.approval_state = "consumed".to_string();
            event.approval_hash = Some(HASH_B.to_string());
            barrier.wait();
            store.record(event)
        }));
    }
    barrier.wait();
    for handle in handles {
        handle.join().unwrap().unwrap();
    }
    let query = AuditQuery {
        limit: 20,
        workspace_hash: Some(HASH_A.to_string()),
        action_kind: None,
    };
    let report = store.list_scoped(HASH_A, &query).unwrap();
    assert_eq!(report.events.len(), 8);
    assert!(report.events.windows(2).all(|pair| {
        (pair[0].timestamp_unix_ms, pair[0].event_id.as_str())
            <= (pair[1].timestamp_unix_ms, pair[1].event_id.as_str())
    }));
    let encoded = serde_json::to_string(&report).unwrap();
    assert!(!encoded.contains("approval-"));

    let other_query = AuditQuery {
        limit: 20,
        workspace_hash: Some(HASH_B.to_string()),
        action_kind: None,
    };
    assert!(store.list_scoped(HASH_A, &other_query).is_err());
    assert!(store.list(&other_query).unwrap().events.is_empty());
}

fn apply_request(fixture: &Fixture) -> PolicyRequest {
    let mut request = fixture.request(
        PolicyAction::ApplyPatch(ApplyPatchAction {
            root: fixture.workspace.path().to_path_buf(),
            paths: vec![PathBuf::from("src/Main.java")],
            created_paths: Vec::new(),
            diff_hash: HASH_A.to_string(),
            files_hash: HASH_A.to_string(),
            transaction_id: "transaction-001".to_string(),
            base_head: HASH_A.to_string(),
        }),
        PolicyMode::ApprovedApply,
    );
    fixture.observe_clean_repository(&mut request);
    request
}

fn repository_boundary(root: &Path, head: &str) -> GitRepositoryBoundary {
    let git_dir = root.join(".git");
    fs::create_dir_all(git_dir.join("objects")).unwrap();
    fs::write(git_dir.join("index"), b"").unwrap();
    GitRepositoryBoundary {
        worktree_root: root.to_path_buf(),
        git_dir: git_dir.clone(),
        common_dir: git_dir.clone(),
        index: git_dir.join("index"),
        object_dir: git_dir.join("objects"),
        head: head.to_string(),
        main_worktree: true,
    }
}

fn assert_drift(
    fixture: &Fixture,
    confirmation: &NativeConfirmation,
    mutate: impl FnOnce(&mut PolicyRequest),
    expected_rule: &str,
) {
    fs::write(
        fixture.workspace.path().join("src/Main.java"),
        "class Main {}\n",
    )
    .unwrap();
    let mut request = apply_request(fixture);
    let grant = fixture
        .engine
        .issue_approval(&request, confirmation, 60)
        .unwrap();
    mutate(&mut request);
    request.approval_id = Some(grant.approval_id);
    let report = fixture.engine.check(&request).unwrap();
    assert_eq!(report.report.decision.rule_id(), expected_rule);
}

fn standalone_approval_binding() -> ApprovalBinding {
    ApprovalBinding {
        request_id: "request-approval".to_string(),
        workspace_id: "workspace-approval".to_string(),
        workspace_root_hash: HASH_A.to_string(),
        mode: PolicyMode::ApprovedApply,
        base_head: "a".repeat(40),
        working_tree_digest: HASH_A.to_string(),
        diff_hash: HASH_A.to_string(),
        files_hash: HASH_A.to_string(),
        files: vec![
            ApprovalFileBinding {
                path_hash: "a".repeat(64),
                expected_hash: "b".repeat(64),
            },
            ApprovalFileBinding {
                path_hash: "c".repeat(64),
                expected_hash: "d".repeat(64),
            },
        ],
        action_hashes: vec!["a".repeat(64), "b".repeat(64)],
        transaction_id: "transaction-approval".to_string(),
    }
}

fn assert_approval_binding_drift(
    mutate: impl FnOnce(&mut ApprovalBinding),
    expected: ApprovalError,
) {
    let state = tempfile::tempdir().unwrap();
    let store = ApprovalStore::open(state.path()).unwrap();
    let binding = standalone_approval_binding();
    let confirmation = NativeConfirmation::explicit("vscode", unique_id("modal")).unwrap();
    let grant = store.issue(binding.clone(), &confirmation, 60).unwrap();
    let mut observed = binding;
    mutate(&mut observed);
    assert_eq!(store.consume(&grant.approval_id, &observed), Err(expected));
}

fn audit_event(id: &str, timestamp: u64) -> AuditEvent {
    AuditEvent {
        schema_version: 1,
        event_id: id.to_string(),
        timestamp_unix_ms: timestamp,
        request_id: "request-001".to_string(),
        action_id_hash: HASH_A.to_string(),
        action_kind: "read_file".to_string(),
        action_hash: HASH_A.to_string(),
        rule_id: "read.safe_workspace_file".to_string(),
        decision: "allow".to_string(),
        risk: opticcode_policy::RiskLevel::Low,
        workspace_hash: HASH_A.to_string(),
        origin: ActionOrigin::Cli,
        approval_state: "none".to_string(),
        approval_hash: None,
        transaction_hash: None,
        result: "authorized".to_string(),
        duration_us: 1,
    }
}

struct ActiveFixture {
    run_id: String,
    root: PathBuf,
    source: PathBuf,
    git_dir: PathBuf,
    common_dir: PathBuf,
    lease: PathBuf,
}

impl ActiveFixture {
    fn new(source: &Path) -> Self {
        Self::create(source, true)
    }

    fn new_without_lease(source: &Path) -> Self {
        Self::create(source, false)
    }

    fn create(source: &Path, lease_enabled: bool) -> Self {
        let run_id = unique_id("active");
        let storage = prepare_storage();
        let root = storage.join("runs").join(&run_id);
        fs::create_dir(&root).unwrap();
        let common_dir = source.join(".git");
        fs::create_dir_all(common_dir.join("worktrees")).unwrap();
        let git_dir = common_dir.join("worktrees").join(&run_id);
        fs::create_dir(&git_dir).unwrap();
        fs::write(git_dir.join("commondir"), "../..\n").unwrap();
        fs::write(
            root.join(".git"),
            format!("gitdir: {}\n", git_dir.display()),
        )
        .unwrap();
        let lease = storage.join("leases").join(format!("{run_id}.json"));
        if lease_enabled {
            let value = serde_json::json!({
                "schema_version": 1,
                "run_id": run_id.clone(),
                "owner_workspace_id": "workspace-001",
                "owner_request_id": "request-001",
                "process_id": std::process::id(),
                "created_unix_ms": unix_ms(),
                "source_git_root": source,
                "source_project": source,
                "source_commit": HASH_A,
                "worktree_path": root.clone(),
            });
            fs::write(&lease, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        }
        Self {
            run_id,
            root,
            source: source.to_path_buf(),
            git_dir,
            common_dir,
            lease,
        }
    }

    fn descriptor(&self) -> ActiveWorktree {
        ActiveWorktree {
            run_id: self.run_id.clone(),
            owner_workspace_id: "workspace-001".to_string(),
            owner_request_id: "request-001".to_string(),
            root: self.root.clone(),
            source_root: self.source.clone(),
            base_head: HASH_A.to_string(),
            git_dir: self.git_dir.clone(),
            common_dir: self.common_dir.clone(),
        }
    }

    fn attach(&self, request: &mut PolicyRequest) {
        request.workspace.active_worktree = Some(self.descriptor());
        request.workspace.repository = Some(repository_boundary(&self.source, HASH_A));
        request.workspace.working_tree_digest = Some(HASH_B.to_string());
        request.workspace.repository_clean = Some(true);
    }
}

impl Drop for ActiveFixture {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lease);
        let _ = fs::remove_dir_all(&self.root);
        let _ = fs::remove_dir_all(&self.git_dir);
    }
}

fn prepare_storage() -> PathBuf {
    let storage = std::env::temp_dir().join("opticcode-worktrees");
    fs::create_dir_all(storage.join("runs")).unwrap();
    fs::create_dir_all(storage.join("leases")).unwrap();
    storage
}

fn unique_id(prefix: &str) -> String {
    format!(
        "{prefix}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn unix_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

#[cfg(windows)]
fn create_junction(link: &Path, target: &Path) {
    let status = Command::new("cmd")
        .args(["/D", "/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .status()
        .unwrap();
    assert!(status.success(), "failed to create test junction");
}
