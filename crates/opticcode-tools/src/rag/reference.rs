use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;

use super::policy::{
    denied_directory, denied_relative_directory, metadata_is_link_or_reparse, sensitive_file_name,
};
use super::secrets::detect_secret;

pub const MAX_SAFE_REFERENCE_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SafeWorkspaceFile {
    pub relative_path: String,
    pub bytes: u64,
    pub chars: usize,
    pub content_hash: String,
    #[serde(skip)]
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SafeReferenceError {
    pub rule_id: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SafeTextSecret {
    pub rule_id: &'static str,
    pub category: &'static str,
    pub line: u32,
    pub column: u32,
}

pub fn inspect_sensitive_text(content: &str) -> Option<SafeTextSecret> {
    detect_secret(content).map(|secret| SafeTextSecret {
        rule_id: secret.rule_id,
        category: secret.category,
        line: secret.line,
        column: secret.column,
    })
}

impl SafeReferenceError {
    fn new(rule_id: &'static str, message: impl Into<String>) -> Self {
        Self {
            rule_id,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SafeReferenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.rule_id, self.message)
    }
}

impl std::error::Error for SafeReferenceError {}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileFingerprint {
    len: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    attributes: u32,
}

pub fn read_safe_workspace_file(
    workspace: &Path,
    requested_path: &Path,
    max_bytes: u64,
) -> Result<SafeWorkspaceFile, SafeReferenceError> {
    if max_bytes == 0 || max_bytes > MAX_SAFE_REFERENCE_FILE_BYTES {
        return Err(SafeReferenceError::new(
            "size.invalid_limit",
            format!("reference byte limit must be between 1 and {MAX_SAFE_REFERENCE_FILE_BYTES}"),
        ));
    }

    let workspace_metadata = fs::symlink_metadata(workspace).map_err(|_| {
        SafeReferenceError::new("io.workspace_metadata", "workspace cannot be inspected")
    })?;
    if !workspace_metadata.is_dir() || metadata_is_link_or_reparse(&workspace_metadata) {
        return Err(SafeReferenceError::new(
            "path.workspace_reparse",
            "workspace root must be a real directory",
        ));
    }
    let root = fs::canonicalize(workspace).map_err(|_| {
        SafeReferenceError::new("io.workspace_resolve", "workspace cannot be resolved")
    })?;

    let relative = relative_request_path(&root, requested_path)?;
    reject_sensitive_path(&relative)?;
    let candidate = root.join(&relative);
    validate_components(&root, &relative)?;

    let before_metadata = fs::symlink_metadata(&candidate)
        .map_err(|_| SafeReferenceError::new("path.not_found", "referenced file does not exist"))?;
    if !before_metadata.is_file() || metadata_is_link_or_reparse(&before_metadata) {
        return Err(SafeReferenceError::new(
            "path.symlink_or_reparse",
            "reference is not a regular file",
        ));
    }
    if before_metadata.len() > max_bytes {
        return Err(SafeReferenceError::new(
            "size.too_large",
            format!("referenced file exceeds the {max_bytes}-byte limit"),
        ));
    }
    let before_canonical = fs::canonicalize(&candidate).map_err(|_| {
        SafeReferenceError::new("path.resolve_failed", "referenced file cannot be resolved")
    })?;
    if !before_canonical.starts_with(&root) {
        return Err(SafeReferenceError::new(
            "path.escape",
            "referenced file resolves outside the workspace",
        ));
    }
    let before_fingerprint = fingerprint(&before_metadata);

    let mut file = File::open(&candidate).map_err(|_| {
        SafeReferenceError::new("io.read_failed", "referenced file cannot be opened")
    })?;
    let handle_before = file.metadata().map_err(|_| {
        SafeReferenceError::new("io.metadata_failed", "open file cannot be inspected")
    })?;
    if fingerprint(&handle_before) != before_fingerprint {
        return Err(SafeReferenceError::new(
            "content.changed_during_read",
            "referenced file changed before it could be read",
        ));
    }

    let mut bytes = Vec::with_capacity(before_metadata.len() as usize);
    file.by_ref()
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| {
            SafeReferenceError::new("io.read_failed", "referenced file could not be read")
        })?;
    if bytes.len() as u64 > max_bytes {
        return Err(SafeReferenceError::new(
            "size.too_large",
            format!("referenced file exceeds the {max_bytes}-byte limit"),
        ));
    }

    validate_components(&root, &relative)?;
    let handle_after = file.metadata().map_err(|_| {
        SafeReferenceError::new("io.metadata_failed", "open file cannot be re-inspected")
    })?;
    let after_metadata = fs::symlink_metadata(&candidate).map_err(|_| {
        SafeReferenceError::new("content.changed_during_read", "referenced file disappeared")
    })?;
    let after_canonical = fs::canonicalize(&candidate).map_err(|_| {
        SafeReferenceError::new("content.changed_during_read", "referenced file moved")
    })?;
    if metadata_is_link_or_reparse(&after_metadata)
        || before_canonical != after_canonical
        || before_fingerprint != fingerprint(&handle_after)
        || before_fingerprint != fingerprint(&after_metadata)
    {
        return Err(SafeReferenceError::new(
            "content.changed_during_read",
            "referenced file changed while it was being read",
        ));
    }

    let content = String::from_utf8(bytes).map_err(|_| {
        SafeReferenceError::new("content.invalid_utf8", "referenced file is not UTF-8 text")
    })?;
    if let Some(secret) = detect_secret(&content) {
        return Err(SafeReferenceError::new(
            secret.rule_id,
            format!(
                "referenced file was excluded by secret scanning at line {}, column {}",
                secret.line, secret.column
            ),
        ));
    }

    Ok(SafeWorkspaceFile {
        relative_path: normalized_relative(&relative),
        bytes: content.len() as u64,
        chars: content.chars().count(),
        content_hash: blake3::hash(content.as_bytes()).to_hex().to_string(),
        content,
    })
}

fn relative_request_path(
    canonical_root: &Path,
    requested_path: &Path,
) -> Result<PathBuf, SafeReferenceError> {
    let relative = if requested_path.is_absolute() {
        requested_path.strip_prefix(canonical_root).map_err(|_| {
            SafeReferenceError::new("path.escape", "absolute reference is outside the workspace")
        })?
    } else {
        requested_path
    };
    if relative.as_os_str().is_empty() {
        return Err(SafeReferenceError::new(
            "path.not_file",
            "workspace root is not a file reference",
        ));
    }
    for component in relative.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(SafeReferenceError::new(
                "path.invalid_relative",
                "reference path must contain only normal relative components",
            ));
        }
    }
    Ok(relative.to_path_buf())
}

fn reject_sensitive_path(relative: &Path) -> Result<(), SafeReferenceError> {
    if let Some((rule_id, _)) = denied_relative_directory(relative) {
        return Err(SafeReferenceError::new(
            rule_id,
            "reference points into an excluded generated directory",
        ));
    }
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        if components.peek().is_some() {
            if let Some(name) = component.as_os_str().to_str() {
                if let Some((rule_id, _)) = denied_directory(name) {
                    return Err(SafeReferenceError::new(
                        rule_id,
                        "reference points into an excluded directory",
                    ));
                }
            }
        }
    }
    if let Some((rule_id, _)) = sensitive_file_name(relative) {
        return Err(SafeReferenceError::new(
            rule_id,
            "reference points to a sensitive file name",
        ));
    }
    Ok(())
}

fn validate_components(root: &Path, relative: &Path) -> Result<(), SafeReferenceError> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&current).map_err(|_| {
            SafeReferenceError::new("path.not_found", "reference path component does not exist")
        })?;
        if metadata_is_link_or_reparse(&metadata) {
            return Err(SafeReferenceError::new(
                "path.symlink_or_reparse",
                "reference path contains a symlink, junction, or reparse point",
            ));
        }
    }
    Ok(())
}

fn fingerprint(metadata: &fs::Metadata) -> FileFingerprint {
    FileFingerprint {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        created: metadata.created().ok(),
        attributes: metadata_attributes(metadata),
    }
}

#[cfg(windows)]
fn metadata_attributes(metadata: &fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes()
}

#[cfg(not(windows))]
fn metadata_attributes(_metadata: &fs::Metadata) -> u32 {
    0
}

fn normalized_relative(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::read_safe_workspace_file;

    fn fixture() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("src")).unwrap();
        fs::write(
            temp.path().join("src").join("Plugin.java"),
            "public class Plugin {}\n",
        )
        .unwrap();
        temp
    }

    #[test]
    fn reads_regular_utf8_files_inside_the_workspace() {
        let temp = fixture();
        let file =
            read_safe_workspace_file(temp.path(), Path::new("src/Plugin.java"), 1024).unwrap();
        assert_eq!(file.relative_path, "src/Plugin.java");
        assert!(file.content.contains("class Plugin"));
    }

    #[test]
    fn rejects_escape_sensitive_and_secret_references() {
        let temp = fixture();
        assert_eq!(
            read_safe_workspace_file(temp.path(), Path::new("../outside.txt"), 1024)
                .unwrap_err()
                .rule_id,
            "path.invalid_relative"
        );
        fs::write(temp.path().join(".env"), "TOKEN=super-secret-value\n").unwrap();
        assert_eq!(
            read_safe_workspace_file(temp.path(), Path::new(".env"), 1024)
                .unwrap_err()
                .rule_id,
            "path.file.environment"
        );
        fs::write(
            temp.path().join("src").join("Credentials.txt"),
            "api_key=abcdefghijklmnopqrstuvwxyz123456\n",
        )
        .unwrap();
        assert!(
            read_safe_workspace_file(temp.path(), Path::new("src/Credentials.txt"), 1024).is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_components() {
        use std::os::unix::fs::symlink;

        let temp = fixture();
        symlink(temp.path().join("src"), temp.path().join("linked")).unwrap();
        assert_eq!(
            read_safe_workspace_file(temp.path(), Path::new("linked/Plugin.java"), 1024)
                .unwrap_err()
                .rule_id,
            "path.symlink_or_reparse"
        );
    }

    #[cfg(windows)]
    #[test]
    fn rejects_windows_junction_components() {
        use std::process::Command;

        let temp = fixture();
        let link = temp.path().join("linked");
        let target = temp.path().join("src");
        let output = Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "failed to create test junction: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            read_safe_workspace_file(temp.path(), Path::new("linked/Plugin.java"), 1024)
                .unwrap_err()
                .rule_id,
            "path.symlink_or_reparse"
        );
        fs::remove_dir(&link).unwrap();
    }
}
