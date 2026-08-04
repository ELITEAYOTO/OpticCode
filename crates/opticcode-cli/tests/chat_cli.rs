use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture() -> PathBuf {
    workspace().join("benchmarks/java-index-mini")
}

fn request(request_id: &str, command: &str, references: Value) -> Value {
    json!({
        "schema_version": 1,
        "protocol": "opticcode.chat",
        "request_id": request_id,
        "workspace_id": "chat-cli-fixture",
        "workspace_root": fixture(),
        "command": command,
        "prompt": if matches!(command, "ask" | "plan" | "context") {
            "Locate Helpers#ping()."
        } else {
            ""
        },
        "profile": "none",
        "provider": "ollama",
        "model": "qwen2.5-coder:14b",
        "context_mode": "symbol",
        "references": references,
        "history": [],
        "budgets": {
            "max_history_turns": 12,
            "max_history_chars": 32768,
            "max_history_tokens": 8192,
            "max_references": 24,
            "max_reference_bytes": 1048576,
            "max_prompt_tokens": 32768,
            "rag_hits": 0
        },
        "generation": {
            "max_output_tokens": 64,
            "temperature": null,
            "seed": null,
            "brief": true,
            "compare_generate": false
        },
        "security_mode": "read_only",
        "client": {
            "name": "chat-cli-test",
            "version": "0.1.0",
            "vscode_version": "1.125.0",
            "session_id": "chat-cli-session",
            "locale": "en",
            "recent_run_ids": [],
            "previous_repository_state": null
        },
        "expected_protocols": {
            "chat": 1,
            "assistant": 1,
            "discovery": 1,
            "llm": 1
        }
    })
}

fn run_chat(value: &Value) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_opticcode"))
        .current_dir(workspace())
        .args([
            "chat",
            "--protocol-jsonl",
            "--rag-index",
            "missing-chat-cli-index",
            "--http-timeout-ms",
            "1000",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn opticcode chat");
    let mut stdin = child.stdin.take().expect("chat stdin");
    serde_json::to_writer(&mut stdin, value).expect("write request");
    stdin.write_all(b"\n").expect("terminate request");
    drop(stdin);
    child.wait_with_output().expect("wait for chat")
}

fn events(output: &Output) -> Vec<Value> {
    assert!(
        output.stderr.is_empty(),
        "stderr must remain protocol-clean"
    );
    String::from_utf8(output.stdout.clone())
        .expect("chat output UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("one JSON event per line"))
        .collect()
}

#[test]
fn help_chat_is_versioned_sequenced_read_only_and_terminal_once() {
    let output = run_chat(&request("chat-cli-help", "help", json!([])));
    assert!(output.status.success());
    let values = events(&output);
    assert!(!values.is_empty());
    for (sequence, event) in values.iter().enumerate() {
        assert_eq!(event["schema_version"], 1);
        assert_eq!(event["protocol"], "opticcode.chat");
        assert_eq!(event["request_id"], "chat-cli-help");
        assert_eq!(event["sequence"], sequence as u64);
    }
    assert_eq!(values[0]["type"], "request_accepted");
    assert_eq!(values[0]["requested_security_mode"], "read_only");
    assert_eq!(values[0]["security_mode"], "read_only");
    assert_eq!(values[0]["effective_security_mode"], "read_only");
    assert_eq!(values[0]["policy_decision"], "allow");
    assert_eq!(values[0]["policy_rule_id"], "analysis.context_read_only");
    assert_eq!(values[0]["policy_version"], "opticcode.default.v1");
    assert_eq!(values.last().unwrap()["type"], "completed");
    assert_eq!(terminal_count(&values), 1);
    let rendered = values
        .iter()
        .filter(|event| event["type"] == "token_delta")
        .filter_map(|event| event["text"].as_str())
        .collect::<String>();
    assert!(rendered.contains("/apply"));
    assert!(rendered.contains("unavailable until CHAT-EDIT-001"));
}

#[test]
fn chat_cli_forces_read_only_when_client_requests_edit_mode() {
    let mut value = request("chat-cli-policy-mode", "fix", json!([]));
    value["security_mode"] = json!("approved_apply");
    value["prompt"] = json!("Fix the selected code");
    let output = run_chat(&value);
    assert_eq!(output.status.code(), Some(2));
    let values = events(&output);
    assert_eq!(values[0]["type"], "request_accepted");
    assert_eq!(values[0]["requested_security_mode"], "approved_apply");
    assert_eq!(values[0]["effective_security_mode"], "read_only");
    assert_eq!(values[0]["policy_decision"], "allow");
    assert_eq!(values.last().unwrap()["type"], "failed");
    assert_eq!(
        values.last().unwrap()["error"]["code"],
        "security_mode_unavailable"
    );
    assert!(!values
        .iter()
        .any(|event| event["type"] == "references_resolving"));
    assert_eq!(terminal_count(&values), 1);
}

#[test]
fn chat_resolves_safe_ranges_and_rejects_missing_or_outside_references() {
    let references = json!([
        {
            "reference_id": "safe-range",
            "inclusion_reason": "selected by user",
            "kind": "range",
            "path": "src/main/java/dev/opticcode/util/Helpers.java",
            "range": {
                "start": { "line": 0, "character": 0 },
                "end": { "line": 2, "character": 1 }
            }
        },
        {
            "reference_id": "missing",
            "inclusion_reason": "attached by user",
            "kind": "file",
            "path": "src/Missing.java"
        },
        {
            "reference_id": "outside",
            "inclusion_reason": "attached by user",
            "kind": "file",
            "path": "../Cargo.toml"
        }
    ]);
    let output = run_chat(&request("chat-cli-references", "help", references));
    assert!(
        output.status.success(),
        "chat reference request failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let values = events(&output);
    let resolved = values
        .iter()
        .find(|event| event["type"] == "references_resolved")
        .expect("references_resolved event");
    assert_eq!(resolved["accepted"].as_array().unwrap().len(), 1);
    assert_eq!(resolved["accepted"][0]["reference_id"], "safe-range");
    assert_eq!(resolved["rejected"].as_array().unwrap().len(), 2);
    assert_eq!(terminal_count(&values), 1);
}

#[test]
fn malformed_initial_request_is_one_pure_failed_event() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_opticcode"))
        .current_dir(workspace())
        .args(["chat", "--protocol-jsonl"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"request_id\":\"malformed-chat\"}\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let values = events(&output);
    assert_eq!(values.len(), 1);
    assert_eq!(values[0]["type"], "failed");
    assert_eq!(values[0]["sequence"], 0);
    assert_eq!(values[0]["request_id"], "malformed-chat");
}

#[test]
fn isolated_chat_help_does_not_overflow_the_windows_stack() {
    for arguments in [["chat", "--help"], ["help", "chat"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_opticcode"))
            .current_dir(workspace())
            .args(arguments)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("Usage:"));
    }
}

fn terminal_count(values: &[Value]) -> usize {
    values
        .iter()
        .filter(|event| {
            matches!(
                event["type"].as_str(),
                Some("completed" | "failed" | "cancelled")
            )
        })
        .count()
}
