use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

#[test]
fn java_edits_cli_is_read_only_deterministic_explainable_and_bounded() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/java-edits-legacy");
    let before = snapshot_tree(&corpus);
    let first = parse_json(&run_java_edits(&corpus, 10_000, true));
    let second = parse_json(&run_java_edits(&corpus, 10_000, true));

    assert_eq!(first["schema_version"], 1);
    assert_eq!(first["operation"], "java_edit_proposals");
    assert_eq!(first["rule_set"], "minecraft_java_1_8_v1");
    assert_eq!(first["index_schema_version"], 1);
    assert_eq!(first["index_source"]["discovered_files"], 13);
    assert_eq!(first["index_source"]["parsed_files"], 13);
    assert_eq!(first["index_truncation"]["source"], false);
    assert_eq!(first["analysis_complete"], true);
    assert_eq!(first["safe_to_apply"], true);
    assert_eq!(first["truncated"], false);
    assert_eq!(first["counts"]["references_examined"], 85);
    assert_eq!(first["counts"]["legacy_candidates"], 26);
    assert_eq!(first["counts"]["exact_target_matches"], 18);
    assert_eq!(first["counts"]["proposals"], 16);
    assert_eq!(first["counts"]["files_with_proposals"], 3);
    assert_eq!(first["counts"]["rejections"], 10);
    assert_eq!(first["proposals"].as_array().map(Vec::len), Some(16));
    assert_eq!(first["file_validations"].as_array().map(Vec::len), Some(3));
    assert_eq!(first["rejections"].as_array().map(Vec::len), Some(10));
    assert!(first["timings"]["serialization_us"].is_u64());
    assert!(first["proposals"].as_array().is_some_and(|proposals| {
        proposals.iter().all(|proposal| {
            proposal["source_hash"]
                .as_str()
                .is_some_and(|hash| hash.starts_with("blake3:"))
                && proposal["expected_content"].is_string()
                && proposal["expected_node_content"].is_string()
                && proposal["replacement"].is_string()
                && proposal["confidence"] == "syntax_exact"
        })
    }));

    let mut first_stable = first.clone();
    let mut second_stable = second;
    first_stable
        .as_object_mut()
        .expect("report object")
        .remove("timings");
    second_stable
        .as_object_mut()
        .expect("report object")
        .remove("timings");
    assert_eq!(first_stable, second_stable);

    let bounded = parse_json(&run_java_edits(&corpus, 3, true));
    assert_eq!(bounded["proposals"].as_array().map(Vec::len), Some(3));
    assert_eq!(bounded["proposals_truncated"], true);
    assert_eq!(bounded["analysis_complete"], false);
    assert_eq!(bounded["safe_to_apply"], false);
    assert_eq!(bounded["truncated"], true);

    let human = run_java_edits(&corpus, 10_000, false);
    assert!(human.status.success());
    let stdout = String::from_utf8_lossy(&human.stdout);
    assert!(stdout.contains("Java edit proposals (read-only)"));
    assert!(stdout.contains("GUNPOWDER -> SULPHUR [MC18-MATERIAL-001]"));
    assert!(stdout.contains("safe to apply downstream: true"));

    let after = snapshot_tree(&corpus);
    assert_eq!(before, after, "java-edits CLI modified its input corpus");
}

#[test]
fn java_edits_cli_rejects_invalid_limits() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/java-edits-legacy");
    let output = run_java_edits(&corpus, 0, true);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("Java edit proposal limit must be between 1"));
}

#[cfg(windows)]
#[test]
fn java_edits_cli_rejects_a_junction_root() {
    let fixture = TemporaryFixture::new();
    let target = fixture.root.join("target sources");
    let junction = fixture.root.join("linked sources");
    fs::create_dir_all(&target).expect("create junction target");
    fs::write(
        target.join("Plugin.java"),
        concat!(
            "import org.bukkit.Material;\n",
            "class Plugin { Object value = Material.GUNPOWDER; }\n",
        ),
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

    let output = run_java_edits(&junction, 10, true);
    fs::remove_dir(&junction).expect("remove junction without deleting target");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("symlink or reparse point"));
}

fn run_java_edits(path: &Path, proposal_limit: usize, json: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_opticcode"));
    command.args(["java-edits", "--path"]).arg(path).args([
        "--limit",
        "100",
        "--max-file-bytes",
        "1048576",
        "--item-limit",
        "2000",
        "--symbol-limit",
        "10000",
        "--reference-limit",
        "10000",
        "--candidate-limit",
        "16",
        "--proposal-limit",
        &proposal_limit.to_string(),
    ]);
    if json {
        command.arg("--json");
    }
    command.output().expect("OpticCode CLI should start")
}

fn parse_json(output: &Output) -> Value {
    assert_eq!(
        output.status.code(),
        Some(0),
        "java-edits should succeed: {}",
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
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "opticcode-cli-java-edits-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create temporary fixture");
        Self { root }
    }
}

impl Drop for TemporaryFixture {
    fn drop(&mut self) {
        if self.root.starts_with(std::env::temp_dir()) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
