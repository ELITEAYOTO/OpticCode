use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use opticcode_tools::git_state::{
    capture_git_state, BuildGitReport, GitChangeOrigin, GitGuardStatus,
};

struct TemporaryRepository {
    root: PathBuf,
}

impl TemporaryRepository {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after Unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "opticcode-git-state-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("temporary repository should be created");
        Self { root }
    }
}

impl Drop for TemporaryRepository {
    fn drop(&mut self) {
        let temp_root = std::env::temp_dir();
        if self.root.starts_with(&temp_root) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[test]
fn captures_and_classifies_build_changes_in_temporary_repository() {
    let repository = TemporaryRepository::new();
    let root = &repository.root;

    run_git(root, &["init", "--quiet"]);
    fs::create_dir_all(root.join("src")).expect("source directory should be created");
    fs::write(root.join("README.md"), "clean\n").expect("README should be written");
    fs::write(root.join("src/Main.java"), "class Main {}\n")
        .expect("Java source should be written");
    fs::write(root.join("dependency-reduced-pom.xml"), "<project/>\n")
        .expect("generated Maven file fixture should be written");
    run_git(root, &["add", "--all"]);
    run_git(
        root,
        &[
            "-c",
            "user.name=OpticCode Test",
            "-c",
            "user.email=opticcode-test@example.invalid",
            "commit",
            "--quiet",
            "--no-verify",
            "-m",
            "initial fixture",
        ],
    );

    let initial = capture_git_state(root).expect("initial Git state should be captured");
    assert!(initial.changes.is_empty(), "fixture should start clean");

    fs::write(root.join("README.md"), "pre-existing user change\n")
        .expect("pre-existing change should be written");
    let before = capture_git_state(root).expect("state before build should be captured");
    assert_eq!(before.changes.len(), 1);
    assert_eq!(before.changes[0].path, "README.md");

    fs::write(root.join("src/Main.java"), "class Main { int built; }\n")
        .expect("simulated build should modify tracked source");
    fs::write(
        root.join("dependency-reduced-pom.xml"),
        "<project><built/></project>\n",
    )
    .expect("simulated Maven build should rewrite generated POM");
    fs::create_dir_all(root.join("target")).expect("target directory should be created");
    fs::write(root.join("target/generated.txt"), "generated\n")
        .expect("simulated build output should be written");

    let after = capture_git_state(root).expect("state after build should be captured");
    let report = BuildGitReport::from_snapshots(before, after, true)
        .expect("before and after snapshots should compare");
    let diff = report
        .diff
        .as_ref()
        .expect("captured report should have diff");

    assert_eq!(report.status, GitGuardStatus::Captured);
    assert!(!report.strict_policy.passed);
    assert!(report.strict_violation());
    assert_eq!(diff.counts.pre_existing, 1);
    assert_eq!(diff.counts.build_generated, 1);
    assert_eq!(diff.counts.tracked_changed, 1);
    assert_eq!(diff.counts.untracked_generated, 1);
    assert_eq!(diff.counts.strict_candidates, 2);

    assert_origin(diff, "README.md", GitChangeOrigin::PreExisting);
    assert_origin(
        diff,
        "dependency-reduced-pom.xml",
        GitChangeOrigin::BuildGenerated,
    );
    assert_origin(diff, "src/Main.java", GitChangeOrigin::TrackedChanged);
    assert_origin(
        diff,
        "target/generated.txt",
        GitChangeOrigin::UntrackedGenerated,
    );
    let generated_tracked = diff
        .changes_after
        .iter()
        .find(|change| change.change.path == "dependency-reduced-pom.xml")
        .expect("generated tracked change should exist");
    assert!(generated_tracked.tracked_was_clean_before);
    assert!(report
        .strict_policy
        .reasons
        .iter()
        .any(|reason| reason.contains("dependency-reduced-pom.xml")));

    let json = serde_json::to_string(&report).expect("report should serialize to JSON");
    assert!(json.contains("\"build_generated\""));
    assert!(json.contains("\"tracked_changed\""));
    assert!(json.contains("\"untracked_generated\""));
}

#[test]
fn captures_real_nul_terminated_rename_with_spaces_and_unicode() {
    let repository = TemporaryRepository::new();
    let root = &repository.root;
    let original = "ancien été.txt";
    let renamed = "nouveau fichier été.txt";

    run_git(root, &["init", "--quiet"]);
    fs::write(root.join(original), "legacy\n").expect("original file should be written");
    run_git(root, &["add", "--all"]);
    run_git(
        root,
        &[
            "-c",
            "user.name=OpticCode Test",
            "-c",
            "user.email=opticcode-test@example.invalid",
            "commit",
            "--quiet",
            "--no-verify",
            "-m",
            "rename fixture",
        ],
    );

    fs::rename(root.join(original), root.join(renamed)).expect("file should be renamed");
    run_git(root, &["add", "--all"]);
    let snapshot = capture_git_state(root).expect("rename should be captured");

    assert_eq!(snapshot.changes.len(), 1);
    assert_eq!(snapshot.changes[0].path, renamed);
    assert_eq!(snapshot.changes[0].original_path.as_deref(), Some(original));
}

#[test]
fn ignored_build_outputs_are_not_reported_by_porcelain_capture() {
    let repository = TemporaryRepository::new();
    let root = &repository.root;

    run_git(root, &["init", "--quiet"]);
    fs::write(root.join(".gitignore"), "target/\n").expect("gitignore should be written");
    fs::write(root.join("README.md"), "clean\n").expect("README should be written");
    run_git(root, &["add", "--all"]);
    run_git(
        root,
        &[
            "-c",
            "user.name=OpticCode Test",
            "-c",
            "user.email=opticcode-test@example.invalid",
            "commit",
            "--quiet",
            "--no-verify",
            "-m",
            "ignored output fixture",
        ],
    );

    fs::create_dir_all(root.join("target")).expect("target directory should be created");
    fs::write(root.join("target/ignored.jar"), b"ignored build output")
        .expect("ignored artifact should be written");
    let snapshot = capture_git_state(root).expect("Git state should be captured");

    assert!(snapshot.changes.is_empty());
    assert_eq!(snapshot.metrics.fingerprinted_files, 0);
    assert_eq!(snapshot.metrics.fingerprinted_bytes, 0);
}

fn assert_origin(
    diff: &opticcode_tools::git_state::GitStateDiff,
    path: &str,
    expected: GitChangeOrigin,
) {
    let classified = diff
        .changes_after
        .iter()
        .find(|change| change.change.path == path)
        .unwrap_or_else(|| panic!("missing classified path: {path}"));
    assert_eq!(classified.origin, expected);
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
