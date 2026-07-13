use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[test]
fn debug_cli_help_and_java_context_json_do_not_overflow_the_windows_stack() {
    let help = Command::new(env!("CARGO_BIN_EXE_opticcode"))
        .arg("--help")
        .output()
        .expect("OpticCode help should start");
    assert!(
        help.status.success(),
        "debug CLI help failed: {}",
        String::from_utf8_lossy(&help.stderr)
    );
    assert!(String::from_utf8_lossy(&help.stdout).contains("java-context"));
    let subcommand_help = Command::new(env!("CARGO_BIN_EXE_opticcode"))
        .args(["help", "java-context"])
        .output()
        .expect("java-context help should start");
    assert!(subcommand_help.status.success());
    assert!(String::from_utf8_lossy(&subcommand_help.stdout).contains("--context-tokens"));

    let corpus = corpus_root();
    let inspect = Command::new(env!("CARGO_BIN_EXE_opticcode"))
        .args(["inspect", "--path"])
        .arg(&corpus)
        .output()
        .expect("debug inspect should start");
    assert!(
        inspect.status.success(),
        "debug command dispatch failed: {}",
        String::from_utf8_lossy(&inspect.stderr)
    );
    let before = snapshot_tree(&corpus);
    let first = parse_json(&run_java_context(
        &corpus,
        "dev.opticcode.util.Helpers#create(String)",
        &[],
    ));
    let second = parse_json(&run_java_context(
        &corpus,
        "dev.opticcode.util.Helpers#create(String)",
        &[],
    ));

    assert_eq!(first["schema_version"], 1);
    assert_eq!(first["operation"], "java_task_context");
    assert_eq!(
        first["primary_symbol"],
        "dev.opticcode.util.Helpers#create(String)"
    );
    assert!(first["timings"]["serialization_us"].is_u64());
    assert!(first["budget"]["rendered_bytes"]
        .as_u64()
        .is_some_and(|used| {
            used <= first["limits"]["max_context_bytes"]
                .as_u64()
                .expect("byte limit")
        }));
    assert!(first["budget"]["rendered_chars"]
        .as_u64()
        .is_some_and(|used| {
            used <= first["limits"]["max_context_chars"]
                .as_u64()
                .expect("character limit")
        }));
    assert!(first["budget"]["estimated_tokens"]
        .as_u64()
        .is_some_and(|used| {
            used <= first["limits"]["max_estimated_tokens"]
                .as_u64()
                .expect("token limit")
        }));

    let mut first_stable = first;
    let mut second_stable = second;
    first_stable
        .as_object_mut()
        .expect("first report object")
        .remove("timings");
    second_stable
        .as_object_mut()
        .expect("second report object")
        .remove("timings");
    assert_eq!(first_stable, second_stable);
    assert_eq!(before, snapshot_tree(&corpus));
}

#[test]
fn java_context_cli_is_bounded_on_a_large_corpus_and_rejects_invalid_limits() {
    let fixture = TemporaryFixture::new("large-context");
    for index in 0..400 {
        fixture.write(
            format!("src/main/java/load/C{index:03}.java"),
            format!(
                "package load; public final class C{index:03} {{ public void run{index:03}() {{ }} }}\n"
            )
            .as_bytes(),
        );
    }

    let report = parse_json(&run_java_context(
        &fixture.root,
        "load.C399#run399()",
        &["--snippet-limit", "3", "--context-tokens", "256"],
    ));
    assert_eq!(report["index_source"]["parsed_files"], 400);
    assert_eq!(report["primary_symbol"], "load.C399#run399()");
    assert!(report["counts"]["snippets"]
        .as_u64()
        .is_some_and(|count| count <= 3));
    assert!(report["budget"]["estimated_tokens"]
        .as_u64()
        .is_some_and(|tokens| tokens <= 256));

    let invalid = run_java_context(
        &fixture.root,
        "load.C399#run399()",
        &["--context-tokens", "1"],
    );
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("token limit must be between"));
}

#[cfg(windows)]
#[test]
fn java_context_cli_rejects_a_junction_root() {
    let fixture = TemporaryFixture::new("context-junction");
    let target = fixture.root.join("target sources");
    let junction = fixture.root.join("linked sources");
    fs::create_dir_all(&target).expect("create junction target");
    fs::write(
        target.join("Plugin.java"),
        b"class Plugin { void run() { } }\n",
    )
    .expect("write junction fixture");
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

    let output = run_java_context(&junction, "Plugin#run()", &[]);
    fs::remove_dir(&junction).expect("remove junction without deleting target");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("symlink or reparse point"));
}

fn run_java_context(path: &Path, task: &str, extra: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_opticcode"));
    command
        .arg("java-context")
        .arg(task)
        .arg("--path")
        .arg(path)
        .args(extra)
        .arg("--json");
    command.output().expect("OpticCode CLI should start")
}

fn parse_json(output: &Output) -> Value {
    assert_eq!(
        output.status.code(),
        Some(0),
        "java-context should succeed: {}",
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

fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/java-index-mini")
}

fn snapshot_tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn visit(root: &Path, current: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(current).expect("read fixture directory") {
            let entry = entry.expect("read fixture entry");
            let path = entry.path();
            if path.is_dir() {
                visit(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root)
                        .expect("relative fixture path")
                        .to_string_lossy()
                        .replace('\\', "/"),
                    fs::read(path).expect("read fixture file"),
                );
            }
        }
    }

    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

struct TemporaryFixture {
    root: PathBuf,
}

impl TemporaryFixture {
    fn new(label: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_nanos();
        let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "opticcode-{label}-{}-{stamp}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create temporary fixture");
        Self { root }
    }

    fn write(&self, relative: impl AsRef<Path>, bytes: &[u8]) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create fixture parent");
        }
        fs::write(path, bytes).expect("write fixture file");
    }
}

impl Drop for TemporaryFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
