use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

struct TemporaryBuildFixture {
    root: PathBuf,
}

impl TemporaryBuildFixture {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after Unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "opticcode-cli-build-timeout-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("temporary build fixture should be created");
        Self { root }
    }
}

impl Drop for TemporaryBuildFixture {
    fn drop(&mut self) {
        let temp_root = std::env::temp_dir();
        if self.root.starts_with(&temp_root) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[test]
fn blocked_build_returns_bounded_timeout_json() {
    let fixture = TemporaryBuildFixture::new();
    let project = fixture.root.join("project");
    let fake_bin = fixture.root.join("fake-bin");
    fs::create_dir_all(&project).expect("project directory should be created");
    fs::create_dir_all(&fake_bin).expect("fake bin directory should be created");
    fs::write(
        project.join("pom.xml"),
        "<project><modelVersion>4.0.0</modelVersion><groupId>dev.test</groupId><artifactId>blocked</artifactId><version>1</version></project>\n",
    )
    .expect("pom should be written");
    write_blocked_maven(&fake_bin);

    let started_at = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_opticcode"))
        .args(["build", "--path"])
        .arg(&project)
        .args([
            "--timeout-seconds",
            "1",
            "--output-limit-bytes",
            "1024",
            "--json",
        ])
        .env("PATH", path_with_prefix(&fake_bin))
        .output()
        .expect("OpticCode CLI should start");

    assert_eq!(output.status.code(), Some(1));
    assert!(started_at.elapsed() < Duration::from_secs(8));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "CLI stdout should be timeout JSON: {error}; stdout={}; stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });

    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["build_success"], false);
    assert_eq!(report["overall_success"], false);
    assert_eq!(report["process"]["status"], "timed_out");
    assert_eq!(report["process"]["timed_out"], true);
    assert_eq!(report["process"]["cancelled"], false);
    assert_eq!(report["process"]["timeout_ms"], 1000);
    assert_eq!(report["process"]["termination"]["attempted"], true);
    assert_eq!(report["process"]["termination"]["succeeded"], true);
    assert_eq!(report["process"]["output"]["limit_bytes_per_stream"], 1024);
    assert!(report["stdout_tail"]
        .as_str()
        .is_some_and(|value| value.contains("build-started")));

    #[cfg(windows)]
    assert_eq!(
        report["process"]["termination"]["strategy"],
        "windows_job_object"
    );
}

fn write_blocked_maven(fake_bin: &Path) {
    #[cfg(windows)]
    fs::write(
        fake_bin.join("mvn.cmd"),
        "@echo off\r\necho build-started\r\nping -n 30 127.0.0.1 >nul\r\necho build-finished\r\nexit /b 0\r\n",
    )
    .expect("fake Maven command should be written");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let path = fake_bin.join("mvn");
        fs::write(
            &path,
            "#!/bin/sh\nprintf 'build-started\\n'\nsleep 30\nprintf 'build-finished\\n'\n",
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
