use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

struct TemporaryIndexFixture {
    root: PathBuf,
}

impl TemporaryIndexFixture {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after Unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "opticcode cli java index spaces {} {stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("temporary Java index fixture should be created");
        Self { root }
    }

    fn write_sources(&self) -> (PathBuf, Vec<u8>) {
        let source_root = self.root.join("src main/java/dev/test");
        fs::create_dir_all(&source_root).expect("source directory should be created");
        fs::write(
            source_root.join("Peer.java"),
            b"package dev.test; class Peer {}\r\n",
        )
        .expect("Peer.java should be written");
        let valid_path = source_root.join("UnicodeIndex.java");
        let valid_source = concat!(
            "package dev.test;\r\n",
            "class UnicodeIndex { Object caf\u{00e9} = null; Peer peer; }\r\n"
        )
        .as_bytes()
        .to_vec();
        fs::write(&valid_path, &valid_source).expect("Unicode source should be written");
        fs::write(
            source_root.join("Broken.java"),
            "package dev.test; class Broken { MissingType value; void run( { }\r\n",
        )
        .expect("broken source should be written");
        fs::write(source_root.join("NonUtf8.java"), [0xff, 0xfe, 0xfd])
            .expect("non-UTF-8 source should be written");
        (valid_path, valid_source)
    }
}

impl Drop for TemporaryIndexFixture {
    fn drop(&mut self) {
        if self.root.starts_with(std::env::temp_dir()) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[test]
fn java_index_cli_is_read_only_deterministic_and_bounded() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/java-index-mini");
    let first = parse_json(&run_java_index(&corpus, 100_000, 200_000, 16));
    let second = parse_json(&run_java_index(&corpus, 100_000, 200_000, 16));

    assert_eq!(first["schema_version"], 1);
    assert_eq!(first["source"]["syntax_schema_version"], 2);
    assert_eq!(first["operation"], "java_index");
    assert_eq!(first["analysis_complete"], true);
    assert_eq!(first["source"]["parsed_files"], 10);
    assert!(first["counts"]["exact"]
        .as_u64()
        .is_some_and(|count| count > 0));
    assert!(first["counts"]["ambiguous"]
        .as_u64()
        .is_some_and(|count| count > 0));
    assert!(first["counts"]["unresolved"]
        .as_u64()
        .is_some_and(|count| count > 0));
    assert!(first["timings"]["serialization_us"].is_u64());

    let mut first_stable = first.clone();
    let mut second_stable = second;
    remove_timings(&mut first_stable);
    remove_timings(&mut second_stable);
    assert_eq!(first_stable, second_stable);

    let bounded = parse_json(&run_java_index(&corpus, 3, 4, 1));
    assert_eq!(bounded["truncated"], true);
    assert_eq!(bounded["analysis_complete"], false);
    assert_eq!(bounded["symbols"].as_array().map(Vec::len), Some(3));
    assert_eq!(bounded["references"].as_array().map(Vec::len), Some(4));
    assert_eq!(bounded["truncation"]["symbols"], true);
    assert_eq!(bounded["truncation"]["references"], true);

    let fixture = TemporaryIndexFixture::new();
    let (valid_path, valid_source) = fixture.write_sources();
    let partial = parse_json(&run_java_index(&fixture.root, 100, 100, 8));
    assert_eq!(partial["source"]["discovered_files"], 4);
    assert_eq!(partial["source"]["parsed_files"], 3);
    assert_eq!(partial["source"]["skipped_non_utf8_files"], 1);
    assert_eq!(partial["source"]["syntax_error_files"], 1);
    assert_eq!(partial["analysis_complete"], false);
    assert!(partial["counts"]["invalid_syntax_context"]
        .as_u64()
        .is_some_and(|count| count > 0));
    assert_eq!(
        fs::read(valid_path).expect("source should remain readable"),
        valid_source
    );
}

#[cfg(windows)]
#[test]
fn java_index_cli_rejects_a_junction_root() {
    let fixture = TemporaryIndexFixture::new();
    let target = fixture.root.join("target sources");
    let junction = fixture.root.join("linked sources");
    fs::create_dir_all(&target).expect("junction target should be created");
    fs::write(target.join("Target.java"), "class Target {}\n")
        .expect("junction target source should be written");
    let create = Command::new("cmd")
        .args(["/D", "/C", "mklink", "/J"])
        .arg(&junction)
        .arg(&target)
        .output()
        .expect("junction command should start");
    assert!(
        create.status.success(),
        "junction should be created: {}",
        String::from_utf8_lossy(&create.stderr)
    );

    let output = Command::new(env!("CARGO_BIN_EXE_opticcode"))
        .args(["java-index", "--path"])
        .arg(&junction)
        .arg("--json")
        .output()
        .expect("OpticCode CLI should start");
    fs::remove_dir(&junction).expect("junction should be removed without deleting its target");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("symlink or reparse point"));
}

fn run_java_index(
    path: &Path,
    symbol_limit: usize,
    reference_limit: usize,
    candidate_limit: usize,
) -> Output {
    Command::new(env!("CARGO_BIN_EXE_opticcode"))
        .args(["java-index", "--path"])
        .arg(path)
        .args([
            "--limit",
            "100",
            "--max-file-bytes",
            "1048576",
            "--item-limit",
            "1000",
            "--symbol-limit",
            &symbol_limit.to_string(),
            "--reference-limit",
            &reference_limit.to_string(),
            "--candidate-limit",
            &candidate_limit.to_string(),
            "--json",
        ])
        .output()
        .expect("OpticCode CLI should start")
}

fn parse_json(output: &Output) -> Value {
    assert_eq!(
        output.status.code(),
        Some(0),
        "java-index should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "JSON mode must not emit stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "CLI stdout should be JSON: {error}; stdout={}; stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn remove_timings(value: &mut Value) {
    if let Some(object) = value.as_object_mut() {
        object.remove("timings");
    }
}
