use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
const SECRET_SENTINEL: &str = "RAG_SAFE_INVALID_SECRET_SENTINEL_12345";

struct TemporaryCliFixture {
    root: PathBuf,
}

impl TemporaryCliFixture {
    fn new(label: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after Unix epoch")
            .as_nanos();
        let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "opticcode-rag-cli-{label}-{}-{stamp}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("CLI fixture should be created");
        Self { root }
    }

    fn directory(&self, relative: &str) -> PathBuf {
        let path = self.root.join(relative);
        fs::create_dir_all(&path).expect("CLI fixture directory should be created");
        path
    }
}

impl Drop for TemporaryCliFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn rag_commands_are_machine_readable_and_never_echo_rejected_values() {
    let fixture = TemporaryCliFixture::new("commands");
    let source = fixture.directory("source");
    let index = fixture.root.join("index");
    fs::write(
        source.join("Spawner.java"),
        "class Spawner { String material = \"MOB_SPAWNER\"; }\n",
    )
    .unwrap();
    fs::write(source.join("Guide.md"), "Legacy spawner guide.\n").unwrap();
    fs::write(source.join(".env"), format!("PASSWORD={SECRET_SENTINEL}\n")).unwrap();
    fs::write(
        source.join("config.yml"),
        format!("password: {SECRET_SENTINEL}\n"),
    )
    .unwrap();

    let scan = run(&[
        "rag-scan",
        "--path",
        path_arg(&source),
        "--limit",
        "20",
        "--json",
    ]);
    assert_success(&scan);
    assert_no_secret(&scan);
    let scan_json = parse_json(&scan.stdout);
    assert_eq!(scan_json["schema_version"], 1);
    assert_eq!(scan_json["sources"][0]["indexable_files"], 2);
    assert_eq!(scan_json["sources"][0]["excluded_entries"], 2);

    let build = run(&[
        "rag-index",
        "--path",
        path_arg(&source),
        "--output",
        path_arg(&index),
        "--chunk-chars",
        "512",
        "--json",
    ]);
    assert_success(&build);
    assert_no_secret(&build);
    let build_json = parse_json(&build.stdout);
    assert_eq!(build_json["schema_version"], 2);
    assert_eq!(build_json["documents"], 2);
    assert_eq!(build_json["chunks"], 2);
    assert_eq!(build_json["excluded_entries"], 2);
    assert!(index.join("CURRENT").is_file());
    let generation = fs::read_to_string(index.join("CURRENT")).unwrap();
    let generation_dir = index.join("generations").join(generation.trim());
    for name in ["manifest.json", "documents.jsonl", "chunks.jsonl"] {
        let bytes = fs::read(generation_dir.join(name)).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains(SECRET_SENTINEL));
    }

    let search = run(&[
        "rag-search",
        "spawner",
        "--index",
        path_arg(&index),
        "--limit",
        "5",
        "--json",
    ]);
    assert_success(&search);
    assert_no_secret(&search);
    let search_json = parse_json(&search.stdout);
    assert_eq!(search_json["schema_version"], 1);
    assert_eq!(search_json["hits"].as_array().unwrap().len(), 2);

    let debug = run(&[
        "rag-debug",
        "spawner",
        "--index",
        path_arg(&index),
        "--limit",
        "2",
        "--json",
    ]);
    assert_success(&debug);
    assert_no_secret(&debug);
    let debug_json = parse_json(&debug.stdout);
    assert_eq!(debug_json["schema_version"], 1);
    assert!(!debug_json["context"]["hits"].as_array().unwrap().is_empty());
}

#[test]
fn rag_search_refuses_a_legacy_index_instead_of_misreading_it() {
    let fixture = TemporaryCliFixture::new("legacy");
    let index = fixture.directory("index");
    fs::write(index.join("documents.jsonl"), "{}\n").unwrap();
    fs::write(index.join("chunks.jsonl"), "{}\n").unwrap();

    let output = run(&["rag-search", "test", "--index", path_arg(&index), "--json"]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("legacy RAG index"));
    assert!(stderr.contains("rebuild"));
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_opticcode"))
        .args(args)
        .output()
        .expect("opticcode CLI should start")
}

fn path_arg(path: &Path) -> &str {
    path.to_str().expect("test path should be UTF-8")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "CLI failed with stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_no_secret(output: &Output) {
    assert!(!String::from_utf8_lossy(&output.stdout).contains(SECRET_SENTINEL));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(SECRET_SENTINEL));
}

fn parse_json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("CLI output should be pure JSON")
}
