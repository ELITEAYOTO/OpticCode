use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

fn request(workspace: &Path, action: Value) -> Value {
    json!({
        "schema_version": 1,
        "protocol": "opticcode.policy",
        "request_id": "policy-cli-request",
        "action_id": "policy-cli-action",
        "origin": "cli",
        "profile": "minecraft-java-1.8",
        "client": {
            "name": "policy-cli-test",
            "version": "1.0.0"
        },
        "mode": "read_only",
        "workspace": {
            "workspace_id": "policy-cli-workspace",
            "root": workspace,
            "repository": null,
            "active_worktree": null
        },
        "action": action,
        "approval_id": null
    })
}

fn run_policy(state: &Path, args: &[&str], input: Option<&Value>) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_opticcode"))
        .args(args)
        .env("OPTICCODE_POLICY_STATE_DIR", state)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(input) = input {
        let mut stdin = child.stdin.take().unwrap();
        serde_json::to_writer(&mut stdin, input).unwrap();
        stdin.write_all(b"\n").unwrap();
    }
    child.wait_with_output().unwrap()
}

fn run_policy_bytes(state: &Path, args: &[&str], input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_opticcode"))
        .args(args)
        .env("OPTICCODE_POLICY_STATE_DIR", state)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn policy_check_is_pure_json_and_audited() {
    let workspace = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join("src")).unwrap();
    std::fs::write(workspace.path().join("src/Main.java"), "class Main {}\n").unwrap();
    let value = request(
        workspace.path(),
        json!({
            "type": "read_file",
            "data": {
                "root": workspace.path(),
                "path": "src/Main.java"
            }
        }),
    );
    let output = run_policy(state.path(), &["policy", "check", "--json"], Some(&value));
    assert!(
        output.status.success(),
        "stderr={} stdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(output.stderr.is_empty());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["protocol"], "opticcode.policy");
    assert_eq!(report["decision"]["decision"], "allow");
    assert_eq!(report["decision"]["rule_id"], "read.safe_workspace_file");
    assert!(report["audit_event_id"].is_string());

    let audit = run_policy(
        state.path(),
        &["policy", "audit", "--json", "--limit", "10"],
        None,
    );
    assert!(audit.status.success());
    let audit: Value = serde_json::from_slice(&audit.stdout).unwrap();
    assert_eq!(audit["events"].as_array().unwrap().len(), 1);
    assert_eq!(audit["events"][0]["action_kind"], "read_file");
}

#[test]
fn policy_explain_does_not_write_audit_and_unknown_action_is_denied() {
    let workspace = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let unknown = request(workspace.path(), json!({ "type": "future_action" }));
    let output = run_policy(
        state.path(),
        &["policy", "explain", "--json"],
        Some(&unknown),
    );
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["decision"]["decision"], "deny");
    assert_eq!(report["decision"]["rule_id"], "action.unknown");
    assert!(report["audit_event_id"].is_null());

    let audit = run_policy(state.path(), &["policy", "audit", "--json"], None);
    assert!(audit.status.success());
    let audit: Value = serde_json::from_slice(&audit.stdout).unwrap();
    assert!(audit["events"].as_array().unwrap().is_empty());
}

#[test]
fn policy_rejects_trailing_json_and_help_is_available() {
    let state = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_opticcode"))
        .args(["policy", "check", "--json"])
        .env("OPTICCODE_POLICY_STATE_DIR", state.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"{} {}\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("one valid JSON object"));

    let help = Command::new(env!("CARGO_BIN_EXE_opticcode"))
        .args(["policy", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    let text = String::from_utf8_lossy(&help.stdout);
    assert!(text.contains("check"));
    assert!(text.contains("explain"));
    assert!(text.contains("audit"));

    for command in ["check", "explain", "audit"] {
        let help = Command::new(env!("CARGO_BIN_EXE_opticcode"))
            .args(["policy", command, "--help"])
            .output()
            .unwrap();
        assert!(help.status.success(), "missing help for policy {command}");
        assert!(help.stderr.is_empty());
    }
}

#[test]
fn policy_check_uses_stable_decision_exit_codes() {
    let workspace = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join(".env"), "VALUE=test-only\n").unwrap();
    let denied = request(
        workspace.path(),
        json!({
            "type": "read_file",
            "data": {"root": workspace.path(), "path": ".env"}
        }),
    );
    let output = run_policy(state.path(), &["policy", "check", "--json"], Some(&denied));
    assert_eq!(output.status.code(), Some(11));
    assert!(output.stderr.is_empty());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["decision"]["decision"], "deny");

    let mut approval = request(
        workspace.path(),
        json!({
            "type": "recover_transaction",
            "data": {
                "workspace_root": workspace.path(),
                "transaction_id": "transaction-cli",
                "expected_state_hash": "a".repeat(64)
            }
        }),
    );
    approval["mode"] = json!("worktree_edit");
    let output = run_policy(
        state.path(),
        &["policy", "check", "--json"],
        Some(&approval),
    );
    assert_eq!(output.status.code(), Some(10));
    assert!(output.stderr.is_empty());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["decision"]["decision"], "require_approval");
}

#[test]
fn policy_cli_rejects_invalid_schema_unknown_fields_and_oversized_input() {
    let workspace = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let action = json!({
        "type": "read_directory",
        "data": {"root": workspace.path(), "path": "."}
    });
    let mut invalid_schema = request(workspace.path(), action.clone());
    invalid_schema["schema_version"] = json!(999);
    let output = run_policy(
        state.path(),
        &["policy", "check", "--json"],
        Some(&invalid_schema),
    );
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());

    let mut unknown_field = request(workspace.path(), action);
    unknown_field["unsafe_override"] = json!(true);
    let output = run_policy(
        state.path(),
        &["policy", "check", "--json"],
        Some(&unknown_field),
    );
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());

    let oversized = vec![b' '; 1024 * 1024 + 1];
    let output = run_policy_bytes(state.path(), &["policy", "check", "--json"], &oversized);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("exceeds"));
}

#[test]
fn policy_cli_handles_spaces_unicode_and_never_modifies_the_workspace() {
    let parent = tempfile::tempdir().unwrap();
    let workspace = parent.path().join("Projet \u{00c9}te avec espaces");
    let state = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.join("src")).unwrap();
    let source = workspace.join("src/Classe.java");
    let original = b"class Classe {}\r\n";
    std::fs::write(&source, original).unwrap();
    let value = request(
        &workspace,
        json!({
            "type": "read_file",
            "data": {"root": &workspace, "path": "src/Classe.java"}
        }),
    );
    let output = run_policy(state.path(), &["policy", "check", "--json"], Some(&value));
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    serde_json::from_slice::<Value>(&output.stdout).unwrap();
    assert_eq!(std::fs::read(source).unwrap(), original);
    assert!(!workspace.join(".opticcode").exists());
    assert!(!workspace.join(".git").exists());
}
