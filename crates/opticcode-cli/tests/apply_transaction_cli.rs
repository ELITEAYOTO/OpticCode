use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

struct TemporaryApplyFixture {
    root: PathBuf,
}

impl TemporaryApplyFixture {
    fn new(label: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after Unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "opticcode-cli-apply-{label}-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("temporary apply fixture should be created");
        Self { root }
    }

    fn create_git_project(&self) -> ApplyProject {
        let project = self.root.join("project with spaces");
        let java_path = project.join("src/main/java/dev/test/LegacyTest.java");
        fs::create_dir_all(java_path.parent().expect("Java file should have a parent"))
            .expect("source directories should be created");
        fs::write(
            project.join("pom.xml"),
            "<project><properties><maven.compiler.source>1.8</maven.compiler.source><maven.compiler.target>1.8</maven.compiler.target></properties><dependencies><dependency><groupId>org.spigotmc</groupId><artifactId>spigot-api</artifactId><version>1.8.8-R0.1-SNAPSHOT</version></dependency></dependencies></project>\n",
        )
        .expect("pom should be written");
        let original_java = b"package dev.test;\nclass LegacyTest { Object item = Material.GUNPOWDER; Object tool = Material.WOODEN_SHOVEL; }\n".to_vec();
        fs::write(&java_path, &original_java).expect("Java fixture should be written");
        fs::write(project.join("README.md"), "clean\n").expect("README should be written");

        run_git(&project, &["init", "--quiet"]);
        run_git(&project, &["add", "--all"]);
        run_git(
            &project,
            &[
                "-c",
                "user.name=OpticCode Test",
                "-c",
                "user.email=opticcode-test@example.invalid",
                "commit",
                "--quiet",
                "--no-verify",
                "-m",
                "CLI apply fixture",
            ],
        );

        ApplyProject {
            root: project,
            java_path,
            original_java,
        }
    }
}

impl Drop for TemporaryApplyFixture {
    fn drop(&mut self) {
        let temp_root = std::env::temp_dir();
        if self.root.starts_with(&temp_root) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

struct ApplyProject {
    root: PathBuf,
    java_path: PathBuf,
    original_java: Vec<u8>,
}

#[test]
fn apply_inspect_list_and_undo_are_transactional() {
    let fixture = TemporaryApplyFixture::new("lifecycle");
    let project = fixture.create_git_project();

    let apply = run_opticcode(&[
        "apply",
        "--path",
        project.root.to_str().expect("project path should be UTF-8"),
        "--allow-external",
        "--yes",
        "--json",
    ]);
    assert_cli_exit(&apply, 0);
    let apply_json = parse_json(&apply);
    assert_eq!(apply_json["schema_version"], 1);
    assert_eq!(apply_json["operation"], "apply");
    assert_eq!(apply_json["operation_success"], true);
    assert_eq!(apply_json["mode"], "in_place");
    assert_eq!(apply_json["transaction"]["final_state"], "committed");
    assert_eq!(apply_json["transaction"]["rollback_attempted"], false);
    assert_eq!(
        apply_json["transaction"]["planned_files"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    let transaction_id = apply_json["transaction"]["transaction_id"]
        .as_str()
        .expect("transaction id should be present")
        .to_string();
    let run_dir = project.root.join(".opticcode/runs").join(&transaction_id);
    assert!(run_dir.join("manifest.json").is_file());
    assert!(run_dir.join("patch.diff").is_file());
    assert!(run_dir.join("backups/00000000.bin").is_file());
    assert!(run_dir.join("events/00000004-committed.json").is_file());

    let applied_java = fs::read(&project.java_path).expect("applied Java should be readable");
    let applied_text = String::from_utf8(applied_java).expect("Java should remain UTF-8");
    assert!(applied_text.contains("Material.SULPHUR"));
    assert!(applied_text.contains("Material.WOOD_SPADE"));

    let inspect = run_opticcode(&[
        "transactions",
        "--path",
        project.root.to_str().expect("project path should be UTF-8"),
        "--inspect",
        &transaction_id,
        "--json",
    ]);
    assert_cli_exit(&inspect, 0);
    let inspect_json = parse_json(&inspect);
    assert_eq!(inspect_json["valid"], true);
    assert_eq!(inspect_json["legacy"], false);
    assert_eq!(inspect_json["final_state"], "committed");
    assert_eq!(inspect_json["events"].as_array().map(Vec::len), Some(5));

    let list = run_opticcode(&[
        "transactions",
        "--path",
        project.root.to_str().expect("project path should be UTF-8"),
        "--json",
    ]);
    assert_cli_exit(&list, 0);
    let list_json = parse_json(&list);
    assert_eq!(list_json["schema_version"], 1);
    assert!(list_json["transactions"]
        .as_array()
        .is_some_and(|transactions| transactions.iter().any(|transaction| {
            transaction["transaction_id"] == transaction_id
                && transaction["final_state"] == "committed"
        })));

    let invalid = run_opticcode(&[
        "transactions",
        "--path",
        project.root.to_str().expect("project path should be UTF-8"),
        "--inspect",
        "../escape",
        "--json",
    ]);
    assert_cli_exit(&invalid, 5);
    let invalid_json = parse_json(&invalid);
    assert_eq!(invalid_json["operation_success"], false);
    assert_eq!(invalid_json["error_kind"], "invalid_transaction");

    let undo = run_opticcode(&[
        "apply",
        "--path",
        project.root.to_str().expect("project path should be UTF-8"),
        "--undo",
        &transaction_id,
        "--allow-external",
        "--yes",
        "--json",
    ]);
    assert_cli_exit(&undo, 0);
    let undo_json = parse_json(&undo);
    assert_eq!(undo_json["operation"], "undo");
    assert_eq!(undo_json["operation_success"], true);
    assert_eq!(undo_json["transaction"]["final_state"], "rolled_back");
    assert_eq!(undo_json["transaction"]["rollback_success"], true);
    assert_eq!(undo_json["transaction"]["git_restored"], true);
    assert_eq!(
        fs::read(&project.java_path).expect("restored Java should be readable"),
        project.original_java
    );

    let second_undo = run_opticcode(&[
        "apply",
        "--path",
        project.root.to_str().expect("project path should be UTF-8"),
        "--undo",
        &transaction_id,
        "--allow-external",
        "--yes",
        "--json",
    ]);
    assert_cli_exit(&second_undo, 0);
    assert_eq!(
        parse_json(&second_undo)["transaction"]["final_state"],
        "rolled_back"
    );
}

#[test]
fn dirty_repository_requires_explicit_policy_and_is_restored_exactly() {
    let fixture = TemporaryApplyFixture::new("dirty-policy");
    let project = fixture.create_git_project();
    let readme_path = project.root.join("README.md");
    let pre_existing_readme = b"pre-existing local work\r\n";
    fs::write(&readme_path, pre_existing_readme).expect("dirty README should be written");

    let rejected = run_opticcode(&[
        "apply",
        "--path",
        project.root.to_str().expect("project path should be UTF-8"),
        "--allow-external",
        "--yes",
        "--json",
    ]);
    assert_cli_exit(&rejected, 4);
    let rejected_json = parse_json(&rejected);
    assert_eq!(rejected_json["operation_success"], false);
    assert_eq!(rejected_json["error_kind"], "precondition");
    assert!(rejected_json["error"]
        .as_str()
        .is_some_and(|error| error.contains("clean Git worktree")));
    assert_eq!(
        fs::read(&project.java_path).expect("rejected Java should be readable"),
        project.original_java
    );
    assert!(!project.root.join(".opticcode").exists());

    let allowed = run_opticcode(&[
        "apply",
        "--path",
        project.root.to_str().expect("project path should be UTF-8"),
        "--allow-external",
        "--allow-dirty",
        "--yes",
        "--json",
    ]);
    assert_cli_exit(&allowed, 0);
    let allowed_json = parse_json(&allowed);
    assert_eq!(allowed_json["transaction"]["final_state"], "committed");
    assert_eq!(
        allowed_json["transaction"]["planned_files"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert!(allowed_json["transaction"]["transaction_id"].is_string());
    assert_eq!(
        fs::read(&readme_path).expect("README should remain readable"),
        pre_existing_readme
    );

    let transaction_id = allowed_json["transaction"]["transaction_id"]
        .as_str()
        .expect("transaction id should be present");
    let manifest: Value = serde_json::from_slice(
        &fs::read(
            project
                .root
                .join(".opticcode/runs")
                .join(transaction_id)
                .join("manifest.json"),
        )
        .expect("manifest should be readable"),
    )
    .expect("manifest should be valid JSON");
    assert_eq!(manifest["validation"]["git_policy"], "allow_dirty");
    assert_eq!(manifest["validation"]["git_clean"], false);
    assert_eq!(manifest["validation"]["pre_existing_changes"], 1);
    assert!(manifest["files"][0]["before_hash"]
        .as_str()
        .is_some_and(|hash| hash.starts_with("blake3:")));

    let undo = run_opticcode(&[
        "apply",
        "--path",
        project.root.to_str().expect("project path should be UTF-8"),
        "--undo",
        transaction_id,
        "--allow-external",
        "--yes",
        "--json",
    ]);
    assert_cli_exit(&undo, 0);
    let undo_json = parse_json(&undo);
    assert_eq!(undo_json["transaction"]["git_restored"], true);
    assert_eq!(
        fs::read(&project.java_path).expect("restored Java should be readable"),
        project.original_java
    );
    assert_eq!(
        fs::read(&readme_path).expect("README should be readable"),
        pre_existing_readme
    );
}

#[test]
fn rollback_failure_has_a_distinct_exit_and_can_be_recovered() {
    let fixture = TemporaryApplyFixture::new("rollback-recovery");
    let project = fixture.create_git_project();

    let apply = run_opticcode(&[
        "apply",
        "--path",
        project.root.to_str().expect("project path should be UTF-8"),
        "--allow-external",
        "--yes",
        "--json",
    ]);
    assert_cli_exit(&apply, 0);
    let apply_json = parse_json(&apply);
    let transaction_id = apply_json["transaction"]["transaction_id"]
        .as_str()
        .expect("transaction id should be present")
        .to_string();
    let applied_java = fs::read(&project.java_path).expect("applied Java should be readable");

    fs::remove_file(&project.java_path).expect("applied Java should be removable");
    fs::create_dir(&project.java_path).expect("blocking directory should be created");
    fs::write(project.java_path.join("foreign.txt"), b"do not overwrite\n")
        .expect("blocking content should be written");

    let failed_undo = run_opticcode(&[
        "apply",
        "--path",
        project.root.to_str().expect("project path should be UTF-8"),
        "--undo",
        &transaction_id,
        "--allow-external",
        "--yes",
        "--json",
    ]);
    assert_cli_exit(&failed_undo, 3);
    let failed_json = parse_json(&failed_undo);
    assert_eq!(failed_json["operation_success"], false);
    assert_eq!(failed_json["transaction"]["final_state"], "rollback_failed");
    assert_eq!(failed_json["transaction"]["rollback_success"], false);
    assert!(failed_json["transaction"]["errors"]
        .as_array()
        .is_some_and(|errors| !errors.is_empty()));

    fs::remove_dir_all(&project.java_path).expect("blocking directory should be removed");
    fs::write(&project.java_path, applied_java).expect("applied state should be restored safely");

    let recovery = run_opticcode(&[
        "transactions",
        "--path",
        project.root.to_str().expect("project path should be UTF-8"),
        "--recover",
        &transaction_id,
        "--allow-external",
        "--yes",
        "--json",
    ]);
    assert_cli_exit(&recovery, 0);
    let recovery_json = parse_json(&recovery);
    assert_eq!(recovery_json["operation_success"], true);
    assert_eq!(recovery_json["final_state"], "rolled_back");
    assert_eq!(recovery_json["rollback_success"], true);
    assert_eq!(recovery_json["git_restored"], true);
    assert_eq!(
        fs::read(&project.java_path).expect("recovered Java should be readable"),
        project.original_java
    );
}

#[cfg(windows)]
#[test]
fn locked_target_returns_exit_two_with_successful_rollback_json() {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_SHARE_READ: u32 = 0x0000_0001;

    let fixture = TemporaryApplyFixture::new("locked-target");
    let project = fixture.create_git_project();
    let locked = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&project.java_path)
        .expect("Java target should be opened without delete sharing");

    let apply = run_opticcode(&[
        "apply",
        "--path",
        project.root.to_str().expect("project path should be UTF-8"),
        "--allow-external",
        "--yes",
        "--json",
    ]);

    assert_cli_exit(&apply, 2);
    let json = parse_json(&apply);
    assert_eq!(json["operation_success"], false);
    assert_eq!(json["transaction"]["final_state"], "rolled_back");
    assert_eq!(json["transaction"]["rollback_attempted"], true);
    assert_eq!(json["transaction"]["rollback_success"], true);
    assert_eq!(json["transaction"]["git_restored"], true);
    assert_eq!(
        fs::read(&project.java_path).expect("locked Java should remain readable"),
        project.original_java
    );
    drop(locked);
}

fn run_opticcode(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_opticcode"))
        .args(args)
        .output()
        .expect("OpticCode CLI should start")
}

fn parse_json(output: &Output) -> Value {
    assert!(
        output.stderr.is_empty(),
        "JSON mode must not emit human stderr output: {}",
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

fn assert_cli_exit(output: &Output, expected: i32) {
    assert_eq!(
        output.status.code(),
        Some(expected),
        "unexpected CLI exit; stdout={}; stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(["-c", "core.autocrlf=false", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(root)
        .output()
        .expect("Git command should start");
    assert!(
        output.status.success(),
        "Git command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
