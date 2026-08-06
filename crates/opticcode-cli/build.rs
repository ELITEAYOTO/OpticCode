use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

const GIT_COMMIT_ENV: &str = "OPTICCODE_GIT_COMMIT";
const GIT_DIRTY_ENV: &str = "OPTICCODE_GIT_DIRTY";

fn main() {
    println!("cargo:rerun-if-env-changed={GIT_COMMIT_ENV}");
    println!("cargo:rerun-if-env-changed={GIT_DIRTY_ENV}");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=src");

    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR")
            .expect("Cargo must provide CARGO_MANIFEST_DIR to build.rs"),
    );

    let workspace_root = manifest_dir.join("../..");

    track_git_metadata(&workspace_root);

    let commit = env::var(GIT_COMMIT_ENV)
        .ok()
        .and_then(|value| normalize_commit(&value))
        .or_else(|| {
            git_output(&workspace_root, &["rev-parse", "--verify", "HEAD"])
                .and_then(|value| normalize_commit(&value))
        });

    if let Some(commit) = commit {
        let short_commit: String = commit.chars().take(8).collect();

        emit_rustc_env(GIT_COMMIT_ENV, &commit);
        emit_rustc_env("OPTICCODE_GIT_COMMIT_SHORT", &short_commit);
    }

    let dirty = env::var(GIT_DIRTY_ENV)
        .ok()
        .and_then(|value| parse_bool(&value))
        .or_else(|| {
            git_output(
                &workspace_root,
                &["status", "--porcelain=v1", "--untracked-files=normal"],
            )
            .map(|output| !output.trim().is_empty())
        });

    if let Some(dirty) = dirty {
        emit_rustc_env(GIT_DIRTY_ENV, if dirty { "true" } else { "false" });
    }

    let target = env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());
    let profile = env::var("PROFILE").unwrap_or_else(|_| "unknown".to_string());

    emit_rustc_env("OPTICCODE_BUILD_TARGET", &target);
    emit_rustc_env("OPTICCODE_BUILD_PROFILE", &profile);
}

fn git_output(workspace_root: &Path, arguments: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .args(arguments)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_string())
}

fn normalize_commit(value: &str) -> Option<String> {
    let value = value.trim();

    if !(40..=64).contains(&value.len()) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }

    Some(value.to_ascii_lowercase())
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

fn emit_rustc_env(key: &str, value: &str) {
    if value
        .bytes()
        .any(|byte| matches!(byte, b'\r' | b'\n' | b'\0'))
    {
        return;
    }

    println!("cargo:rustc-env={key}={value}");
}

fn track_git_metadata(workspace_root: &Path) {
    for arguments in [
        &["rev-parse", "--git-path", "HEAD"][..],
        &["rev-parse", "--git-path", "index"][..],
        &["rev-parse", "--git-path", "packed-refs"][..],
    ] {
        if let Some(path) = git_output(workspace_root, arguments) {
            track_path(workspace_root, &path);
        }
    }

    if let Some(reference) = git_output(workspace_root, &["symbolic-ref", "-q", "HEAD"]) {
        if let Some(path) = git_output(workspace_root, &["rev-parse", "--git-path", &reference]) {
            track_path(workspace_root, &path);
        }
    }
}

fn track_path(workspace_root: &Path, value: &str) {
    let path = PathBuf::from(value);

    let absolute = if path.is_absolute() {
        path
    } else {
        workspace_root.join(path)
    };

    println!("cargo:rerun-if-changed={}", absolute.display());
}
