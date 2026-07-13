use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

struct TemporaryJavaSyntaxFixture {
    root: PathBuf,
}

impl TemporaryJavaSyntaxFixture {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after Unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "opticcode-cli-java-syntax-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("temporary syntax fixture should be created");
        Self { root }
    }

    fn write_sources(&self) -> (PathBuf, Vec<u8>) {
        let source_root = self.root.join("src/main/java/dev/test");
        fs::create_dir_all(&source_root).expect("source directories should be created");
        let valid_path = source_root.join("AValid.java");
        let valid_source = br#"package dev.test;

import org.bukkit.Material;

public class AValid {
    // Material.GUNPOWDER is documentation only.
    private String text = "Material.GUNPOWDER";
    private Object value = Material.GUNPOWDER;

    public void run() {
        getServer().broadcastMessage(text);
    }
}
"#
        .to_vec();
        fs::write(&valid_path, &valid_source).expect("valid Java should be written");
        fs::write(
            source_root.join("ZBroken.java"),
            "package dev.test; class ZBroken { void run( { }\n",
        )
        .expect("broken Java should be written");
        fs::write(source_root.join("ZNonUtf8.java"), [0xff, 0xfe, 0xfd])
            .expect("non-UTF-8 Java should be written");
        (valid_path, valid_source)
    }
}

impl Drop for TemporaryJavaSyntaxFixture {
    fn drop(&mut self) {
        if self.root.starts_with(std::env::temp_dir()) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[test]
fn java_syntax_cli_is_read_only_bounded_and_structured() {
    let fixture = TemporaryJavaSyntaxFixture::new();
    let (valid_path, valid_source) = fixture.write_sources();

    let full = run_java_syntax(&fixture.root, 10);
    assert_eq!(full.status.code(), Some(0));
    let report = parse_json(&full);
    assert_eq!(report["schema_version"], 2);
    assert_eq!(report["operation"], "java_syntax");
    assert_eq!(report["limits"]["max_files"], 10);
    assert_eq!(report["limits"]["max_file_bytes"], 1_048_576);
    assert_eq!(report["limits"]["max_items_per_kind"], 1000);
    assert_eq!(report["limits"]["max_warnings"], 100);
    assert_eq!(report["discovered_files"], 3);
    assert_eq!(report["parsed_files"], 2);
    assert_eq!(report["syntax_error_files"], 1);
    assert_eq!(report["skipped_non_utf8_files"], 1);
    assert_eq!(report["file_selection_truncated"], false);
    assert_eq!(report["retained_items_truncated"], false);
    assert_eq!(report["warnings_truncated"], false);
    assert_eq!(report["truncated"], false);
    assert_eq!(report["analysis_complete"], false);

    let mut first_stable_report = report.clone();
    let mut second_stable_report = parse_json(&run_java_syntax(&fixture.root, 10));
    remove_timing_fields(&mut first_stable_report);
    remove_timing_fields(&mut second_stable_report);
    assert_eq!(first_stable_report, second_stable_report);

    let valid = report["files"]
        .as_array()
        .and_then(|files| {
            files.iter().find(|file| {
                file["path"]
                    .as_str()
                    .is_some_and(|path| path.replace('\\', "/").ends_with("AValid.java"))
            })
        })
        .expect("valid Java report should exist");
    assert_eq!(valid["syntax_valid"], true);
    assert!(valid["symbols"].as_array().is_some_and(|symbols| {
        symbols
            .iter()
            .any(|symbol| symbol["kind"] == "class" && symbol["name"] == "AValid")
    }));
    let legacy_references = valid["references"]
        .as_array()
        .expect("references should be an array")
        .iter()
        .filter(|reference| {
            reference["kind"] == "field_access"
                && reference["qualifier"] == "Material"
                && reference["name"] == "GUNPOWDER"
        })
        .count();
    assert_eq!(legacy_references, 1);
    assert!(valid["excluded_regions"]
        .as_array()
        .is_some_and(|regions| regions.len() >= 2));
    assert_eq!(
        fs::read(&valid_path).expect("valid source should remain readable"),
        valid_source
    );

    let limited = run_java_syntax(&fixture.root, 1);
    assert_eq!(limited.status.code(), Some(0));
    let limited = parse_json(&limited);
    assert_eq!(limited["discovered_files"], 3);
    assert_eq!(limited["selected_files"], 1);
    assert_eq!(limited["parsed_files"], 1);
    assert_eq!(limited["file_selection_truncated"], true);
    assert_eq!(limited["truncated"], true);
    assert!(limited["files"][0]["path"]
        .as_str()
        .is_some_and(|path| path.replace('\\', "/").ends_with("AValid.java")));

    let item_limited = parse_json(&run_java_syntax_with_item_limit(&fixture.root, 10, 1));
    let valid = item_limited["files"]
        .as_array()
        .and_then(|files| {
            files.iter().find(|file| {
                file["path"]
                    .as_str()
                    .is_some_and(|path| path.replace('\\', "/").ends_with("AValid.java"))
            })
        })
        .expect("item-limited valid report should exist");
    assert_eq!(valid["retained_items_truncated"], true);
    assert_eq!(item_limited["retained_items_truncated"], true);
    assert_eq!(item_limited["truncated"], true);
    assert!(valid["counts"]["symbols"]
        .as_u64()
        .is_some_and(|count| count > 1));
    assert_eq!(valid["symbols"].as_array().map(Vec::len), Some(1));
}

fn run_java_syntax(path: &Path, limit: usize) -> Output {
    run_java_syntax_with_item_limit(path, limit, 1000)
}

fn run_java_syntax_with_item_limit(path: &Path, limit: usize, item_limit: usize) -> Output {
    Command::new(env!("CARGO_BIN_EXE_opticcode"))
        .args(["java-syntax", "--path"])
        .arg(path)
        .args([
            "--limit",
            &limit.to_string(),
            "--max-file-bytes",
            "1048576",
            "--item-limit",
            &item_limit.to_string(),
            "--json",
        ])
        .output()
        .expect("OpticCode CLI should start")
}

fn parse_json(output: &Output) -> Value {
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

fn remove_timing_fields(value: &mut Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                remove_timing_fields(value);
            }
        }
        Value::Object(values) => {
            values.remove("duration_us");
            values.remove("parse_duration_us");
            values.remove("analysis_duration_us");
            for value in values.values_mut() {
                remove_timing_fields(value);
            }
        }
        _ => {}
    }
}
