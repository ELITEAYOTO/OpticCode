use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[test]
fn eval_cli_emits_pure_json_and_keeps_unicode_fixture_unchanged() {
    let temp = TempDirectory::new("eval cli unicode");
    let fixture = temp.path.join("fixture espace é");
    fs::create_dir_all(fixture.join("src")).unwrap();
    let source = fixture.join("src/Éclair.java");
    fs::write(&source, "public final class Éclair {}\n").unwrap();
    let original = fs::read(&source).unwrap();
    let suite = json!({
        "schema_version": 1,
        "id": "cli-suite",
        "version": "1",
        "description": "CLI JSON contract",
        "fixtures": {
            "fixture": { "kind": "versioned", "path": "fixture espace é" }
        },
        "cases": [{
            "id": "unicode-symbol",
            "category": "exact_symbol",
            "prompt": "Find Éclair",
            "fixture": "fixture",
            "exact_query": "Éclair",
            "expected": { "relevant_files": ["src/Éclair.java"] },
            "reason": "Exercises spaces and Unicode"
        }]
    });
    let suite_path = temp.path.join("suite.json");
    fs::write(&suite_path, serde_json::to_vec_pretty(&suite).unwrap()).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_opticcode"))
        .args(["eval", "--suite"])
        .arg(&suite_path)
        .args([
            "--strategy",
            "exact",
            "--no-rag",
            "--no-write-reports",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["summary"]["failed"], 0);
    assert_eq!(report["results"][0]["status"], "completed");
    assert!(!stdout.contains("Evaluation "));
    assert_eq!(fs::read(&source).unwrap(), original);
}

#[test]
fn eval_cli_returns_nonzero_for_invalid_corpus_without_mixed_stdout() {
    let temp = TempDirectory::new("eval cli invalid");
    let suite_path = temp.path.join("invalid.json");
    fs::write(
        &suite_path,
        br#"{"schema_version":999,"id":"bad","version":"1","description":"bad","fixtures":{},"cases":[]}"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_opticcode"))
        .args(["eval", "--suite"])
        .arg(&suite_path)
        .args([
            "--strategy",
            "exact",
            "--no-rag",
            "--no-write-reports",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unsupported evaluation suite schema"));
}

struct TempDirectory {
    path: PathBuf,
}

impl TempDirectory {
    fn new(label: &str) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "opticcode-{label}-{}-{timestamp}-{counter}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        assert!(self.path.starts_with(std::env::temp_dir()));
        let _ = fs::remove_dir_all(&self.path);
    }
}
