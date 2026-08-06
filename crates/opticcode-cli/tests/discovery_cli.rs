use std::process::Command;

use serde_json::Value;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_opticcode"))
        .args(args)
        .output()
        .expect("run opticcode discovery command")
}

fn run_json(args: &[&str]) -> Value {
    let output = run(args);

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

fn run_text(args: &[&str]) -> String {
    let output = run(args);

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        output.stderr.is_empty(),
        "discovery command mixed stderr output"
    );

    String::from_utf8(output.stdout).expect("discovery stdout must be valid UTF-8")
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

    assert_eq!(version["platform"]["os"], std::env::consts::OS);
    assert_eq!(version["platform"]["architecture"], std::env::consts::ARCH);

    let target = version["platform"]["target"]
        .as_str()
        .expect("build target must be a string");
    assert!(!target.trim().is_empty());

    let kind = version["build"]["kind"]
        .as_str()
        .expect("build kind must be a string");
    assert!(
        matches!(kind, "debug" | "release"),
        "unexpected build kind: {kind}"
    );

    let profile = version["build"]["profile"]
        .as_str()
        .expect("build profile must be a string");
    assert!(!profile.trim().is_empty());

    assert!(
        version["build"]["dirty"].is_boolean() || version["build"]["dirty"].is_null(),
        "build dirty state must be a boolean or null"
    );

    if let Some(commit) = version["build"]["commit"].as_str() {
        assert!(
            (40..=64).contains(&commit.len()),
            "unexpected commit length: {}",
            commit.len()
        );

        assert!(
            commit.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "commit must contain only hexadecimal characters"
        );

        let short = version["build"]["commit_short"]
            .as_str()
            .expect("short commit must exist when full commit exists");

        assert_eq!(short, &commit[..8]);
    } else {
        assert!(
            version["build"]["commit_short"].is_null(),
            "short commit must be null when the full commit is unavailable"
        );
    }

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
    assert_eq!(capabilities["policy_runtime"]["chat_write"], true);
}

#[test]
fn version_human_output_includes_build_provenance() {
    let output = run_text(&["version"]);

    assert!(output.contains("platform:"));
    assert!(output.contains("target="));
    assert!(output.contains("build:"));
    assert!(output.contains("profile="));
    assert!(output.contains("commit="));
    assert!(output.contains("state="));
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
        .any(|check| { check["id"] == "policy_engine" && check["status"] == "ok" }));
}
