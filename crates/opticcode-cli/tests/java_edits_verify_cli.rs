use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

struct TemporaryFixture {
    root: PathBuf,
}

impl TemporaryFixture {
    fn new(label: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after Unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "opticcode-cli-java-edits-verify-{label}-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("temporary fixture should be created");
        Self { root }
    }

    fn create_project(&self, eligible: bool) -> JavaProject {
        let project = self.root.join("project with spaces");
        let java_path = project.join("src/main/java/dev/test/LegacyTest.java");
        fs::create_dir_all(java_path.parent().expect("Java path should have a parent"))
            .expect("source directories should be created");
        fs::write(
            project.join("pom.xml"),
            concat!(
                "<project><modelVersion>4.0.0</modelVersion>",
                "<groupId>dev.test</groupId><artifactId>java-edits-verify-test</artifactId>",
                "<version>1</version><properties><maven.compiler.source>1.8</maven.compiler.source>",
                "<maven.compiler.target>1.8</maven.compiler.target></properties></project>\n",
            ),
        )
        .expect("pom should be written");
        let modern_or_legacy = if eligible {
            "Object powder = Material.GUNPOWDER; Object tool = Material.WOODEN_SHOVEL;"
        } else {
            "Object powder = Material.SULPHUR; Object tool = Material.WOOD_SPADE;"
        };
        let original_java = format!(
            concat!(
                "package dev.test;\n",
                "import org.bukkit.Material;\n",
                "import org.bukkit.event.entity.CreatureSpawnEvent;\n",
                "class LegacyTest {{ {} ",
                "Object reason = CreatureSpawnEvent.SpawnReason.SPAWNER; }}\n",
            ),
            modern_or_legacy,
        )
        .into_bytes();
        fs::write(&java_path, &original_java).expect("Java fixture should be written");
        fs::write(project.join(".gitignore"), ".opticcode/\ntarget/\n")
            .expect("gitignore should be written");

        run_git(&project, &["init", "--quiet"]);
        run_git(&project, &["config", "core.autocrlf", "false"]);
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
                "Java edit verification fixture",
            ],
        );

        JavaProject {
            root: project,
            java_path,
            original_java,
        }
    }

    fn fake_bin(&self, mode: MavenMode) -> PathBuf {
        let fake_bin = self.root.join(format!("fake-bin-{}", mode.label()));
        fs::create_dir(&fake_bin).expect("fake bin should be created");
        write_fake_maven(&fake_bin, mode);
        fake_bin
    }
}

impl Drop for TemporaryFixture {
    fn drop(&mut self) {
        let temp = std::env::temp_dir();
        if self.root.starts_with(&temp) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

struct JavaProject {
    root: PathBuf,
    java_path: PathBuf,
    original_java: Vec<u8>,
}

#[derive(Clone, Copy)]
enum MavenMode {
    Success,
    Failure,
    Blocked,
    MutatesJava,
}

impl MavenMode {
    fn label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Blocked => "blocked",
            Self::MutatesJava => "mutates-java",
        }
    }
}

#[test]
fn verifies_exact_java_edits_end_to_end_without_mutating_source() {
    let fixture = TemporaryFixture::new("success");
    let project = fixture.create_project(true);
    let output = run_verify(&project.root, &fixture.fake_bin(MavenMode::Success), 10, 10);
    assert_exit(&output, 0);
    let report = parse_json(&output);

    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["operation"], "java_edits_verify");
    assert_eq!(report["operation_success"], true);
    assert_eq!(report["verification_success"], true);
    assert_eq!(report["cleanup_success"], true);
    assert_eq!(report["status"], "passed");
    assert_eq!(report["source_analysis"]["proposals"], 2);
    assert_eq!(report["source_analysis"]["files_with_proposals"], 1);
    assert_eq!(report["source_analysis"]["truncated"], false);
    assert_eq!(report["revalidation"]["attempted"], true);
    assert_eq!(report["revalidation"]["success"], true);
    assert_eq!(report["revalidation"]["received"], 2);
    assert_eq!(report["revalidation"]["valid"], 2);
    assert_eq!(report["revalidation"]["refused"], 0);
    assert_eq!(
        report["revalidation"]["source_contract_fingerprint"],
        report["revalidation"]["worktree_contract_fingerprint"]
    );
    assert_eq!(report["materialization"]["success"], true);
    assert_eq!(report["materialization"]["valid"], 2);
    assert_eq!(
        report["materialization"]["files"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(report["post_write_validation"]["success"], true);
    assert_eq!(report["post_write_validation"]["files_checked"], 1);
    assert_eq!(
        report["post_write_validation"]["files"][0]["bytes_match"],
        true
    );
    assert_eq!(
        report["post_write_validation"]["files"][0]["syntax_valid"],
        true
    );
    assert_eq!(report["final_git_validation"]["attempted"], true);
    assert_eq!(report["final_git_validation"]["success"], true);
    assert_eq!(report["final_git_validation"]["expected_changes"], 1);
    assert_eq!(report["final_git_validation"]["matched_changes"], 1);
    assert_eq!(
        report["final_git_validation"]["unexpected_changes"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );

    let worktree = &report["worktree"];
    assert_eq!(worktree["source"]["unchanged"], true);
    assert_eq!(worktree["source"]["refs_unchanged"], true);
    assert_eq!(worktree["worktree_detached"], true);
    assert_eq!(worktree["apply"]["success"], true);
    assert_eq!(worktree["apply"]["change_count"], 1);
    assert_eq!(worktree["apply"]["patch_complete"], true);
    assert!(worktree["apply"]["patch_hash"]
        .as_str()
        .is_some_and(|hash| hash.starts_with("blake3:")));
    assert_eq!(worktree["apply"]["transaction"]["final_state"], "committed");
    assert_eq!(worktree["build"]["success"], true);
    assert_eq!(worktree["cleanup"]["success"], true);
    let diff = worktree["diff"]["content"]
        .as_str()
        .expect("final Git diff should be present");
    assert!(diff.contains("Material.SULPHUR"));
    assert!(diff.contains("Material.WOOD_SPADE"));
    assert!(diff.contains("CreatureSpawnEvent.SpawnReason.SPAWNER"));

    assert_source_unchanged(&project);
    assert_worktree_cleaned(&project.root, worktree);
}

#[test]
fn failed_build_is_distinct_from_successful_apply_and_still_cleans_up() {
    let fixture = TemporaryFixture::new("build-failure");
    let project = fixture.create_project(true);
    let output = run_verify(&project.root, &fixture.fake_bin(MavenMode::Failure), 10, 10);
    assert_exit(&output, 6);
    let report = parse_json(&output);

    assert_eq!(report["status"], "build_failed");
    assert_eq!(report["operation_success"], false);
    assert_eq!(report["revalidation"]["success"], true);
    assert_eq!(report["materialization"]["success"], true);
    assert_eq!(report["post_write_validation"]["success"], true);
    assert_eq!(report["worktree"]["apply"]["success"], true);
    assert_eq!(report["worktree"]["build"]["success"], false);
    assert_eq!(report["worktree"]["build"]["process_status"], "failed");
    assert_eq!(report["cleanup_success"], true);
    assert_source_unchanged(&project);
    assert_worktree_cleaned(&project.root, &report["worktree"]);
}

#[test]
fn timed_out_build_is_bounded_and_source_safe() {
    let fixture = TemporaryFixture::new("timeout");
    let project = fixture.create_project(true);
    let started_at = Instant::now();
    let output = run_verify(&project.root, &fixture.fake_bin(MavenMode::Blocked), 1, 10);
    assert_exit(&output, 6);
    assert!(started_at.elapsed() < Duration::from_secs(8));
    let report = parse_json(&output);

    assert_eq!(report["status"], "build_failed");
    assert_eq!(report["worktree"]["build"]["process_status"], "timed_out");
    assert_eq!(report["worktree"]["build"]["timed_out"], true);
    assert_eq!(
        report["worktree"]["build"]["termination"]["succeeded"],
        true
    );
    assert_source_unchanged(&project);
    assert_worktree_cleaned(&project.root, &report["worktree"]);
}

#[test]
fn build_mutation_is_rejected_by_git_guard_and_final_hash_validation() {
    let fixture = TemporaryFixture::new("build-mutation");
    let project = fixture.create_project(true);
    let output = run_verify(
        &project.root,
        &fixture.fake_bin(MavenMode::MutatesJava),
        10,
        10,
    );
    assert_exit(&output, 6);
    let report = parse_json(&output);

    assert_eq!(report["status"], "final_git_validation_failed");
    assert_eq!(report["post_write_validation"]["success"], true);
    assert_eq!(report["final_git_validation"]["success"], false);
    assert_eq!(report["final_git_validation"]["matched_changes"], 0);
    assert!(report["final_git_validation"]["errors"]
        .as_array()
        .is_some_and(|errors| !errors.is_empty()));
    assert_eq!(report["worktree"]["build"]["success"], false);
    assert_eq!(
        report["worktree"]["build"]["git_guard"]["strict_policy"]["passed"],
        false
    );
    assert_source_unchanged(&project);
    assert_worktree_cleaned(&project.root, &report["worktree"]);
}

#[test]
fn dirty_source_is_rejected_before_worktree_creation() {
    let fixture = TemporaryFixture::new("dirty");
    let project = fixture.create_project(true);
    fs::write(project.root.join("local.txt"), "not committed\n")
        .expect("dirty fixture should be written");

    let output = run_verify(&project.root, &fixture.fake_bin(MavenMode::Success), 10, 10);
    assert_exit(&output, 8);
    let error = parse_json(&output);
    assert_eq!(error["operation"], "java_edits_verify");
    assert_eq!(error["operation_success"], false);
    assert_eq!(error["error_kind"], "precondition");
    assert!(error["error"]
        .as_str()
        .is_some_and(|message| message.contains("must be clean")));
    assert_eq!(
        fs::read(&project.java_path).expect("source Java should remain readable"),
        project.original_java
    );
}

#[test]
fn proposal_truncation_fails_closed_without_creating_a_worktree() {
    let fixture = TemporaryFixture::new("truncated");
    let project = fixture.create_project(true);
    let output = run_verify(&project.root, &fixture.fake_bin(MavenMode::Success), 10, 1);
    assert_exit(&output, 6);
    let report = parse_json(&output);

    assert_eq!(report["status"], "source_analysis_failed");
    assert_eq!(report["source_analysis"]["truncated"], true);
    assert_eq!(report["source_analysis"]["safe_to_apply"], false);
    assert_eq!(report["revalidation"]["attempted"], false);
    assert!(report["worktree"].is_null());
    assert_source_unchanged(&project);
}

#[test]
fn no_eligible_edits_is_a_successful_zero_cost_noop() {
    let fixture = TemporaryFixture::new("no-changes");
    let project = fixture.create_project(false);
    let output = run_verify(&project.root, &fixture.fake_bin(MavenMode::Failure), 10, 10);
    assert_exit(&output, 0);
    let report = parse_json(&output);

    assert_eq!(report["status"], "no_changes");
    assert_eq!(report["operation_success"], true);
    assert_eq!(report["source_analysis"]["proposals"], 0);
    assert_eq!(report["revalidation"]["attempted"], false);
    assert_eq!(report["post_write_validation"]["attempted"], false);
    assert!(report["worktree"].is_null());
    assert_source_unchanged(&project);
}

#[test]
fn no_change_result_still_requires_a_clean_git_source() {
    let fixture = TemporaryFixture::new("no-changes-dirty");
    let project = fixture.create_project(false);
    fs::write(project.root.join("local.txt"), "not committed\n")
        .expect("dirty fixture should be written");

    let output = run_verify(&project.root, &fixture.fake_bin(MavenMode::Failure), 10, 10);
    assert_exit(&output, 8);
    let error = parse_json(&output);
    assert_eq!(error["operation"], "java_edits_verify");
    assert_eq!(error["error_kind"], "precondition");
    assert!(error["error"]
        .as_str()
        .is_some_and(|message| message.contains("must be clean")));
    assert_eq!(
        fs::read(&project.java_path).expect("source Java should remain readable"),
        project.original_java
    );
}

#[test]
fn oversized_patch_content_is_omitted_but_hashes_remain_authoritative() {
    let fixture = TemporaryFixture::new("bounded-patch");
    let project = fixture.create_project(true);
    let output = run_verify_with_output_limit(
        &project.root,
        &fixture.fake_bin(MavenMode::Success),
        10,
        10,
        128,
    );
    assert_exit(&output, 0);
    let report = parse_json(&output);

    assert_eq!(report["status"], "passed");
    assert_eq!(report["worktree"]["apply"]["patch_complete"], false);
    assert_eq!(report["worktree"]["apply"]["patch"], "");
    assert!(report["worktree"]["apply"]["patch_bytes"]
        .as_u64()
        .is_some_and(|bytes| bytes > 128));
    assert!(report["worktree"]["apply"]["patch_hash"]
        .as_str()
        .is_some_and(|hash| hash.starts_with("blake3:")));
    assert_eq!(report["worktree"]["diff"]["complete"], false);
    assert_eq!(report["final_git_validation"]["success"], true);
    assert_source_unchanged(&project);
    assert_worktree_cleaned(&project.root, &report["worktree"]);
}

fn run_verify(
    project: &Path,
    fake_bin: &Path,
    timeout_seconds: u64,
    proposal_limit: usize,
) -> Output {
    run_verify_with_output_limit(project, fake_bin, timeout_seconds, proposal_limit, 262_144)
}

fn run_verify_with_output_limit(
    project: &Path,
    fake_bin: &Path,
    timeout_seconds: u64,
    proposal_limit: usize,
    output_limit_bytes: usize,
) -> Output {
    Command::new(env!("CARGO_BIN_EXE_opticcode"))
        .args(["java-edits-verify", "--path"])
        .arg(project)
        .args([
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
            "--timeout-seconds",
            &timeout_seconds.to_string(),
            "--git-timeout-seconds",
            "30",
            "--output-limit-bytes",
            &output_limit_bytes.to_string(),
            "--json",
        ])
        .env("PATH", path_with_prefix(fake_bin))
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

fn assert_exit(output: &Output, expected: i32) {
    assert_eq!(
        output.status.code(),
        Some(expected),
        "unexpected CLI exit; stdout={}; stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_source_unchanged(project: &JavaProject) {
    assert_eq!(
        fs::read(&project.java_path).expect("source Java should remain readable"),
        project.original_java
    );
    let status = Command::new("git")
        .args(["status", "--porcelain=v1", "--untracked-files=all"])
        .current_dir(&project.root)
        .output()
        .expect("Git status should start");
    assert!(status.status.success());
    assert!(
        status.stdout.is_empty(),
        "source should remain clean: {}",
        String::from_utf8_lossy(&status.stdout)
    );
}

fn assert_worktree_cleaned(project: &Path, worktree_report: &Value) {
    let worktree = PathBuf::from(
        worktree_report["worktree_root"]
            .as_str()
            .expect("worktree path should be present"),
    );
    assert!(!worktree.exists());
    let list = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(project)
        .output()
        .expect("Git worktree list should start");
    assert!(list.status.success());
    assert!(!String::from_utf8_lossy(&list.stdout).contains(worktree.to_string_lossy().as_ref()));
}

fn run_git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("Git should start");
    assert!(
        output.status.success(),
        "Git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_fake_maven(fake_bin: &Path, mode: MavenMode) {
    #[cfg(windows)]
    {
        let body = match mode {
            MavenMode::Success => "@echo off\r\necho BUILD SUCCESS\r\nexit /b 0\r\n",
            MavenMode::Failure => "@echo off\r\necho BUILD FAILURE 1>&2\r\nexit /b 7\r\n",
            MavenMode::Blocked => {
                "@echo off\r\necho build-started\r\nping -n 30 127.0.0.1 >nul\r\nexit /b 0\r\n"
            }
            MavenMode::MutatesJava => concat!(
                "@echo off\r\n",
                "echo // build mutation>>src\\main\\java\\dev\\test\\LegacyTest.java\r\n",
                "echo BUILD SUCCESS\r\n",
                "exit /b 0\r\n",
            ),
        };
        fs::write(fake_bin.join("mvn.cmd"), body).expect("fake Maven should be written");
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let body = match mode {
            MavenMode::Success => "#!/bin/sh\nprintf 'BUILD SUCCESS\\n'\nexit 0\n",
            MavenMode::Failure => "#!/bin/sh\nprintf 'BUILD FAILURE\\n' >&2\nexit 7\n",
            MavenMode::Blocked => "#!/bin/sh\nprintf 'build-started\\n'\nsleep 30\n",
            MavenMode::MutatesJava => concat!(
                "#!/bin/sh\n",
                "printf '// build mutation\\n' >> src/main/java/dev/test/LegacyTest.java\n",
                "printf 'BUILD SUCCESS\\n'\n",
                "exit 0\n",
            ),
        };
        let path = fake_bin.join("mvn");
        fs::write(&path, body).expect("fake Maven should be written");
        let mut permissions = fs::metadata(&path)
            .expect("fake Maven metadata should exist")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("fake Maven should be executable");
    }
}

fn path_with_prefix(prefix: &Path) -> OsString {
    let existing = std::env::var_os("PATH").unwrap_or_default();
    std::env::join_paths(
        std::iter::once(prefix.to_path_buf()).chain(std::env::split_paths(&existing)),
    )
    .expect("test PATH should be valid")
}
