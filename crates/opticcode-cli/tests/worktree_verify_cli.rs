use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

struct TemporaryWorktreeFixture {
    root: PathBuf,
}

impl TemporaryWorktreeFixture {
    fn new(label: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after Unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "opticcode-cli-worktree-{label}-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("temporary fixture should be created");
        Self { root }
    }

    fn create_project(&self) -> WorktreeProject {
        let project = self.root.join("project with spaces");
        let java_path = project.join("src/main/java/dev/test/LegacyTest.java");
        fs::create_dir_all(java_path.parent().expect("Java path should have a parent"))
            .expect("source directories should be created");
        fs::write(
            project.join("pom.xml"),
            "<project><modelVersion>4.0.0</modelVersion><groupId>dev.test</groupId><artifactId>worktree-test</artifactId><version>1</version><properties><maven.compiler.source>1.8</maven.compiler.source><maven.compiler.target>1.8</maven.compiler.target></properties><dependencies><dependency><groupId>org.spigotmc</groupId><artifactId>spigot-api</artifactId><version>1.8.8-R0.1-SNAPSHOT</version></dependency></dependencies></project>\n",
        )
        .expect("pom should be written");
        let original_java = b"package dev.test;\nclass LegacyTest { Object powder = Material.GUNPOWDER; Object tool = Material.WOODEN_SHOVEL; }\n".to_vec();
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
                "worktree fixture",
            ],
        );

        WorktreeProject {
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

impl Drop for TemporaryWorktreeFixture {
    fn drop(&mut self) {
        let temp = std::env::temp_dir();
        if self.root.starts_with(&temp) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

struct WorktreeProject {
    root: PathBuf,
    java_path: PathBuf,
    original_java: Vec<u8>,
}

#[derive(Clone, Copy)]
enum MavenMode {
    Success,
    Failure,
    Blocked,
}

impl MavenMode {
    fn label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Blocked => "blocked",
        }
    }
}

#[test]
fn verifies_patch_and_build_without_mutating_source() {
    let fixture = TemporaryWorktreeFixture::new("success");
    let project = fixture.create_project();
    let output = run_verify(&project.root, &fixture.fake_bin(MavenMode::Success), 10);
    assert_cli_exit(&output, 0);
    let report = parse_json(&output);

    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["operation"], "worktree_verify");
    assert_eq!(report["operation_success"], true);
    assert_eq!(report["verification_success"], true);
    assert_eq!(report["cleanup_success"], true);
    assert_eq!(report["lease_recovery_required"], false);
    assert_eq!(report["status"], "passed");
    assert_eq!(report["source"]["unchanged"], true);
    assert_eq!(report["source"]["refs_unchanged"], true);
    assert_eq!(
        report["source"]["git_guard"]["strict_policy"]["passed"],
        true
    );
    assert_eq!(report["worktree_commit"], report["source"]["commit_before"]);
    assert_eq!(report["worktree_detached"], true);
    assert_eq!(report["apply"]["success"], true);
    assert_eq!(report["apply"]["change_count"], 1);
    assert_eq!(report["apply"]["transaction"]["final_state"], "committed");
    assert_eq!(report["build"]["success"], true);
    assert_eq!(report["build"]["process_status"], "success");
    assert_eq!(report["cleanup"]["success"], true);
    assert_eq!(report["cleanup"]["schema_version"], 1);
    assert_eq!(report["cleanup"]["operation_success"], true);
    assert_eq!(report["cleanup"]["descriptor_removed"], true);
    assert_eq!(report["diff"]["complete"], true);
    assert!(report["diff"]["content"].as_str().is_some_and(|diff| diff
        .contains("Material.SULPHUR")
        && diff.contains("Material.WOOD_SPADE")));

    assert_eq!(
        fs::read(&project.java_path).expect("source Java should remain readable"),
        project.original_java
    );
    assert_git_clean(&project.root);
    assert_worktree_cleaned(&project.root, &report);
}

#[test]
fn failed_build_is_reported_and_worktree_is_still_cleaned() {
    let fixture = TemporaryWorktreeFixture::new("failure");
    let project = fixture.create_project();
    let output = run_verify(&project.root, &fixture.fake_bin(MavenMode::Failure), 10);
    assert_cli_exit(&output, 6);
    let report = parse_json(&output);

    assert_eq!(report["operation_success"], false);
    assert_eq!(report["verification_success"], false);
    assert_eq!(report["cleanup_success"], true);
    assert_eq!(report["lease_recovery_required"], false);
    assert_eq!(report["status"], "build_failed");
    assert_eq!(report["source"]["unchanged"], true);
    assert_eq!(report["apply"]["success"], true);
    assert_eq!(report["build"]["success"], false);
    assert_eq!(report["build"]["process_status"], "failed");
    assert_eq!(report["cleanup"]["success"], true);
    assert_git_clean(&project.root);
    assert_worktree_cleaned(&project.root, &report);
}

#[test]
fn timed_out_build_is_bounded_and_worktree_is_cleaned() {
    let fixture = TemporaryWorktreeFixture::new("timeout");
    let project = fixture.create_project();
    let started_at = Instant::now();
    let output = run_verify(&project.root, &fixture.fake_bin(MavenMode::Blocked), 1);
    assert_cli_exit(&output, 6);
    assert!(started_at.elapsed() < Duration::from_secs(8));
    let report = parse_json(&output);

    assert_eq!(report["status"], "build_failed");
    assert_eq!(report["build"]["process_status"], "timed_out");
    assert_eq!(report["build"]["timed_out"], true);
    assert_eq!(report["build"]["termination"]["attempted"], true);
    assert_eq!(report["build"]["termination"]["succeeded"], true);
    assert_eq!(report["source"]["unchanged"], true);
    assert_eq!(report["cleanup"]["success"], true);
    assert_git_clean(&project.root);
    assert_worktree_cleaned(&project.root, &report);
}

#[test]
fn dirty_source_is_rejected_before_worktree_creation() {
    let fixture = TemporaryWorktreeFixture::new("dirty");
    let project = fixture.create_project();
    fs::write(project.root.join("local.txt"), "not committed\n")
        .expect("dirty fixture should be written");

    let output = run_verify(&project.root, &fixture.fake_bin(MavenMode::Success), 10);
    assert_cli_exit(&output, 8);
    let error = parse_json(&output);
    assert_eq!(error["operation"], "worktree_verify");
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
fn cleanup_rejects_path_traversal_run_id() {
    let output = Command::new(env!("CARGO_BIN_EXE_opticcode"))
        .args(["worktrees", "--cleanup", "../escape", "--yes", "--json"])
        .output()
        .expect("OpticCode CLI should start");
    assert_cli_exit(&output, 9);
    let error = parse_json(&output);
    assert_eq!(error["operation"], "worktree_cleanup");
    assert_eq!(error["error_kind"], "invalid_run_id");
}

fn run_verify(project: &Path, fake_bin: &Path, timeout_seconds: u64) -> Output {
    Command::new(env!("CARGO_BIN_EXE_opticcode"))
        .args(["worktree-verify", "--path"])
        .arg(project)
        .args([
            "--timeout-seconds",
            &timeout_seconds.to_string(),
            "--git-timeout-seconds",
            "30",
            "--output-limit-bytes",
            "262144",
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

fn assert_cli_exit(output: &Output, expected: i32) {
    assert_eq!(
        output.status.code(),
        Some(expected),
        "unexpected CLI exit; stdout={}; stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_git_clean(project: &Path) {
    let output = Command::new("git")
        .args(["status", "--porcelain=v1", "--untracked-files=all"])
        .current_dir(project)
        .output()
        .expect("Git status should start");
    assert!(output.status.success());
    assert!(
        output.stdout.is_empty(),
        "source should remain clean: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

fn assert_worktree_cleaned(project: &Path, report: &Value) {
    let worktree = PathBuf::from(
        report["worktree_root"]
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
