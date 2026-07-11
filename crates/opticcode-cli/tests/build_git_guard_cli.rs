use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

struct TemporaryCliFixture {
    root: PathBuf,
}

impl TemporaryCliFixture {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after Unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "opticcode-cli-git-guard-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("temporary CLI fixture should be created");
        Self { root }
    }
}

impl Drop for TemporaryCliFixture {
    fn drop(&mut self) {
        let temp_root = std::env::temp_dir();
        if self.root.starts_with(&temp_root) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[test]
fn strict_build_cli_returns_json_and_nonzero_exit() {
    let fixture = TemporaryCliFixture::new();
    let project = fixture.root.join("project");
    let fake_bin = fixture.root.join("fake-bin");
    fs::create_dir_all(project.join("src")).expect("source directory should be created");
    fs::create_dir_all(&fake_bin).expect("fake bin directory should be created");

    fs::write(
        project.join("pom.xml"),
        "<project><modelVersion>4.0.0</modelVersion><groupId>dev.test</groupId><artifactId>fixture</artifactId><version>1</version></project>\n",
    )
    .expect("pom should be written");
    fs::write(project.join("README.md"), "clean\n").expect("README should be written");
    fs::write(project.join("src/Main.java"), "class Main {}\n")
        .expect("Java source should be written");
    fs::write(project.join("dependency-reduced-pom.xml"), "<project/>\n")
        .expect("generated POM fixture should be written");
    write_fake_maven(&fake_bin);

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
            "CLI fixture",
        ],
    );
    fs::write(project.join("README.md"), "pre-existing\n")
        .expect("pre-existing change should be written");

    let snapshot_output = Command::new(env!("CARGO_BIN_EXE_opticcode"))
        .args(["git-state", "--path"])
        .arg(&project)
        .arg("--json")
        .output()
        .expect("git-state CLI should start");
    assert!(snapshot_output.status.success());
    let snapshot: Value =
        serde_json::from_slice(&snapshot_output.stdout).expect("git-state should emit JSON");
    assert_eq!(snapshot["schema_version"], 1);
    assert_eq!(snapshot["metrics"]["status_entries"], 1);
    assert_eq!(snapshot["metrics"]["fingerprinted_files"], 1);
    assert!(snapshot["changes"][0]["content_fingerprint"]
        .as_str()
        .is_some_and(|value| value.starts_with("blake3:")));

    let output = Command::new(env!("CARGO_BIN_EXE_opticcode"))
        .args(["build", "--path"])
        .arg(&project)
        .args(["--fail-on-worktree-change", "--json"])
        .env("PATH", path_with_prefix(&fake_bin))
        .output()
        .expect("OpticCode CLI should start");

    assert_eq!(output.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "CLI stdout should be JSON: {error}; stdout={}",
            String::from_utf8_lossy(&output.stdout)
        )
    });

    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["build_success"], true);
    assert_eq!(report["overall_success"], false);
    assert_eq!(report["exit_code"], 0);
    assert_eq!(report["process"]["status"], "success");
    assert_eq!(report["process"]["timed_out"], false);
    assert_eq!(report["process"]["cancelled"], false);
    assert_eq!(report["process"]["timeout_ms"], 600_000);
    assert_eq!(
        report["process"]["output"]["limit_bytes_per_stream"],
        1_048_576
    );
    assert_eq!(report["git_guard"]["status"], "captured");
    assert_eq!(report["git_guard"]["strict_policy"]["enabled"], true);
    assert_eq!(report["git_guard"]["strict_policy"]["passed"], false);

    let reasons = report["git_guard"]["strict_policy"]["reasons"]
        .as_array()
        .expect("strict reasons should be an array");
    assert_eq!(reasons.len(), 2);
    assert!(reasons.iter().any(|reason| {
        reason
            .as_str()
            .is_some_and(|value| value.contains("dependency-reduced-pom.xml"))
    }));

    let changes = report["git_guard"]["diff"]["changes_after"]
        .as_array()
        .expect("classified changes should be an array");
    assert_origin(changes, "README.md", "pre_existing");
    assert_origin(changes, "dependency-reduced-pom.xml", "build_generated");
    assert_origin(changes, "src/Main.java", "tracked_changed");
    assert_origin(changes, "target/generated.txt", "untracked_generated");
}

fn assert_origin(changes: &[Value], path: &str, expected: &str) {
    let change = changes
        .iter()
        .find(|change| change["change"]["path"] == path)
        .unwrap_or_else(|| panic!("missing CLI classified path: {path}"));
    assert_eq!(change["origin"], expected);
}

fn write_fake_maven(fake_bin: &Path) {
    #[cfg(windows)]
    fs::write(
        fake_bin.join("mvn.cmd"),
        "@echo off\r\n> \"src\\Main.java\" echo class Main { int generated; }\r\n> \"dependency-reduced-pom.xml\" echo ^<project^>^<generated /^>^</project^>\r\nif not exist \"target\" mkdir \"target\"\r\n> \"target\\generated.txt\" echo generated\r\nexit /b 0\r\n",
    )
    .expect("fake Maven command should be written");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let path = fake_bin.join("mvn");
        fs::write(
            &path,
            "#!/bin/sh\nprintf 'class Main { int generated; }\\n' > src/Main.java\nprintf '<project><generated /></project>\\n' > dependency-reduced-pom.xml\nmkdir -p target\nprintf 'generated\\n' > target/generated.txt\nexit 0\n",
        )
        .expect("fake Maven command should be written");
        let mut permissions = fs::metadata(&path)
            .expect("fake Maven metadata should be available")
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
