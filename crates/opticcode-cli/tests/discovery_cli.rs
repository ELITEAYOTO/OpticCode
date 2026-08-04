use std::process::Command;

use serde_json::Value;

fn run_json(args: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_opticcode"))
        .args(args)
        .output()
        .expect("run opticcode discovery command");
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "discovery command mixed stderr output"
    );
    serde_json::from_slice(&output.stdout).expect("discovery stdout must be pure JSON")
}

#[test]
fn version_and_capabilities_are_stable_pure_json_contracts() {
    let version = run_json(&["version", "--json"]);
    assert_eq!(version["schema_version"], 1);
    assert_eq!(version["protocol"], "opticcode.discovery");
    assert_eq!(
        version["protocols"]["assistant"]["id"],
        "opticcode.assistant"
    );
    assert_eq!(version["protocols"]["llm"]["id"], "opticcode.llm");

    let capabilities = run_json(&["capabilities", "--json"]);
    assert_eq!(capabilities["schema_version"], 1);
    assert_eq!(capabilities["protocol"], "opticcode.discovery");
    assert_eq!(capabilities["machine_output"]["ndjson"], true);
    assert_eq!(capabilities["features"]["worktrees"], true);
    assert_eq!(capabilities["features"]["policy"], true);
    assert_eq!(capabilities["policy_runtime"]["engine"], true);
    assert_eq!(capabilities["policy_runtime"]["audit"], true);
    assert_eq!(capabilities["policy_runtime"]["approvals"], true);
    assert_eq!(capabilities["policy_runtime"]["chat_read_only"], true);
    assert_eq!(capabilities["policy_runtime"]["chat_write"], false);
}

#[test]
fn doctor_returns_structured_json_when_ollama_is_unavailable() {
    let report = run_json(&[
        "doctor",
        "--json",
        "--path",
        "benchmarks/java-index-mini",
        "--profile",
        "none",
        "--rag-index",
        "missing-doctor-test-index",
        "--ollama-url",
        "http://127.0.0.1:9",
        "--timeout-ms",
        "100",
    ]);
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["protocol"], "opticcode.discovery");
    assert_eq!(report["success"], false);
    let checks = report["checks"].as_array().expect("doctor checks");
    assert!(checks
        .iter()
        .any(|check| { check["id"] == "ollama_provider" && check["status"] == "error" }));
    assert!(checks
        .iter()
        .any(|check| check["id"] == "worktrees_and_leases"));
    assert!(checks
        .iter()
        .any(|check| check["id"] == "policy_engine" && check["status"] == "ok"));
}
