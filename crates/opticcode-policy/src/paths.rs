use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::model::PathTarget;

const MAX_POLICY_HASH_FILE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathExpectation {
    File,
    Directory,
    ExistingEntry,
    NewFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathFingerprint {
    pub metadata_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    pub bytes: u64,
    pub modified_unix_nanos: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathSafetyReport {
    pub root: PathBuf,
    pub target: PathBuf,
    pub relative_path: PathBuf,
    pub exists: bool,
    pub fingerprint: PathFingerprint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PathSafetyError {
    pub rule_id: &'static str,
    pub message: String,
}

impl PathSafetyError {
    fn new(rule_id: &'static str, message: impl Into<String>) -> Self {
        Self {
            rule_id,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for PathSafetyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.rule_id, self.message)
    }
}

impl std::error::Error for PathSafetyError {}

pub(crate) fn inspect_path(
    target: &PathTarget,
    expectation: PathExpectation,
) -> Result<PathSafetyReport, PathSafetyError> {
    let root_metadata = fs::symlink_metadata(&target.root).map_err(|_| {
        PathSafetyError::new("path.root_unavailable", "action root cannot be inspected")
    })?;
    if !root_metadata.is_dir() || metadata_is_link_or_reparse(&root_metadata) {
        return Err(PathSafetyError::new(
            "path.root_reparse",
            "action root must be a real directory",
        ));
    }
    let root = fs::canonicalize(&target.root).map_err(|_| {
        PathSafetyError::new("path.root_unavailable", "action root cannot be resolved")
    })?;
    let relative = relative_target(&root, &target.path)?;
    reject_sensitive_path(&relative)?;
    reject_nested_repository(&root, &relative)?;

    let candidate = root.join(&relative);
    let (exists, metadata) = inspect_components(&root, &relative, expectation)?;
    let resolved_target = if exists {
        let resolved = fs::canonicalize(&candidate).map_err(|_| {
            PathSafetyError::new("path.resolve_failed", "action target cannot be resolved")
        })?;
        if !resolved.starts_with(&root) {
            return Err(PathSafetyError::new(
                "path.outside_root",
                "action target resolves outside its declared root",
            ));
        }
        resolved
    } else {
        candidate
    };

    if let Some(metadata) = metadata.as_ref() {
        match expectation {
            PathExpectation::File if !metadata.is_file() => {
                return Err(PathSafetyError::new(
                    "path.not_file",
                    "action target is not a regular file",
                ));
            }
            PathExpectation::Directory if !metadata.is_dir() => {
                return Err(PathSafetyError::new(
                    "path.not_directory",
                    "action target is not a directory",
                ));
            }
            PathExpectation::NewFile => {
                return Err(PathSafetyError::new(
                    "path.already_exists",
                    "new-file target already exists",
                ));
            }
            PathExpectation::ExistingEntry | PathExpectation::File | PathExpectation::Directory => {
            }
        }
    } else if expectation != PathExpectation::NewFile {
        return Err(PathSafetyError::new(
            "path.not_found",
            "action target does not exist",
        ));
    }

    let fingerprint = fingerprint(
        metadata.as_ref(),
        &resolved_target,
        target.expected_hash.as_deref(),
    )?;
    Ok(PathSafetyReport {
        root,
        target: resolved_target,
        relative_path: relative,
        exists,
        fingerprint,
    })
}

pub(crate) fn inspect_root(path: &Path) -> Result<PathBuf, PathSafetyError> {
    let target = PathTarget {
        root: path.to_path_buf(),
        path: PathBuf::from("."),
        range: None,
        expected_hash: None,
    };
    // Root actions are represented separately because a normal relative path cannot be empty.
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        PathSafetyError::new(
            "path.root_unavailable",
            "workspace root cannot be inspected",
        )
    })?;
    if !metadata.is_dir() || metadata_is_link_or_reparse(&metadata) {
        return Err(PathSafetyError::new(
            "path.root_reparse",
            "workspace root must be a real directory",
        ));
    }
    let canonical = fs::canonicalize(&target.root).map_err(|_| {
        PathSafetyError::new("path.root_unavailable", "workspace root cannot be resolved")
    })?;
    let canonical_metadata = fs::symlink_metadata(&canonical).map_err(|_| {
        PathSafetyError::new(
            "path.root_unavailable",
            "canonical workspace root cannot be inspected",
        )
    })?;
    if !canonical_metadata.is_dir() || metadata_is_link_or_reparse(&canonical_metadata) {
        return Err(PathSafetyError::new(
            "path.root_reparse",
            "canonical workspace root must be a real directory",
        ));
    }
    Ok(canonical)
}

pub(crate) fn revalidate(report: &PathSafetyReport) -> Result<PathSafetyReport, PathSafetyError> {
    let expectation = if report.exists {
        PathExpectation::ExistingEntry
    } else {
        PathExpectation::NewFile
    };
    inspect_path(
        &PathTarget {
            root: report.root.clone(),
            path: report.relative_path.clone(),
            range: None,
            expected_hash: report.fingerprint.content_hash.clone(),
        },
        expectation,
    )
}

fn relative_target(root: &Path, requested: &Path) -> Result<PathBuf, PathSafetyError> {
    let relative = if requested.is_absolute() {
        requested.strip_prefix(root).map_err(|_| {
            PathSafetyError::new(
                "path.outside_root",
                "absolute action target is outside its declared root",
            )
        })?
    } else {
        requested
    };
    if relative.as_os_str().is_empty() {
        return Err(PathSafetyError::new(
            "path.root_target",
            "the root itself is not a valid file action target",
        ));
    }
    for component in relative.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(PathSafetyError::new(
                "path.invalid_relative",
                "action target must contain only normal relative components",
            ));
        }
    }
    Ok(relative.to_path_buf())
}

fn inspect_components(
    root: &Path,
    relative: &Path,
    expectation: PathExpectation,
) -> Result<(bool, Option<fs::Metadata>), PathSafetyError> {
    let mut current = root.to_path_buf();
    let count = relative.components().count();
    for (index, component) in relative.components().enumerate() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata_is_link_or_reparse(&metadata) {
                    return Err(PathSafetyError::new(
                        "path.symlink_or_reparse",
                        "action target contains a symlink, junction, or reparse point",
                    ));
                }
                if index + 1 < count && !metadata.is_dir() {
                    return Err(PathSafetyError::new(
                        "path.invalid_component",
                        "an intermediate action path component is not a directory",
                    ));
                }
                if index + 1 == count {
                    return Ok((true, Some(metadata)));
                }
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && expectation == PathExpectation::NewFile
                    && index + 1 == count =>
            {
                return Ok((false, None));
            }
            Err(_) => {
                return Err(PathSafetyError::new(
                    "path.not_found",
                    "action path component does not exist or cannot be inspected",
                ));
            }
        }
    }
    Err(PathSafetyError::new(
        "path.invalid_relative",
        "action target has no usable path component",
    ))
}

fn reject_sensitive_path(relative: &Path) -> Result<(), PathSafetyError> {
    for component in relative.components() {
        let name = component.as_os_str().to_string_lossy().to_ascii_lowercase();
        if is_sensitive_name(&name) {
            return Err(PathSafetyError::new(
                "path.sensitive",
                "action target is covered by the sensitive-path policy",
            ));
        }
    }
    Ok(())
}

fn is_sensitive_name(name: &str) -> bool {
    if name == ".env" || name.starts_with(".env.") || name.starts_with(".env-") {
        return true;
    }
    if matches!(
        name,
        ".git"
            | ".npmrc"
            | ".pypirc"
            | ".netrc"
            | "_netrc"
            | "auth.json"
            | ".ssh"
            | ".gnupg"
            | ".aws"
            | ".azure"
            | ".kube"
            | "credentials"
            | "credentials.json"
            | "id_rsa"
            | "id_rsa.pub"
            | "id_dsa"
            | "id_dsa.pub"
            | "id_ecdsa"
            | "id_ecdsa.pub"
            | "id_ed25519"
            | "id_ed25519.pub"
    ) {
        return true;
    }
    if name.ends_with(".pem")
        || name.ends_with(".key")
        || name.ends_with(".p12")
        || name.ends_with(".pfx")
        || name.ends_with(".jks")
        || name.ends_with(".keystore")
        || name.ends_with(".kdbx")
    {
        return true;
    }
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    [
        "credential",
        "credentials",
        "secret",
        "secrets",
        "token",
        "tokens",
    ]
    .iter()
    .any(|prefix| {
        stem == *prefix
            || stem.starts_with(&format!("{prefix}-"))
            || stem.starts_with(&format!("{prefix}_"))
    }) || stem.starts_with("service-account")
        || stem.starts_with("service_account")
}

fn reject_nested_repository(root: &Path, relative: &Path) -> Result<(), PathSafetyError> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        let marker = current.join(".git");
        if let Ok(metadata) = fs::symlink_metadata(marker) {
            if metadata.is_dir() || metadata.is_file() || metadata_is_link_or_reparse(&metadata) {
                return Err(PathSafetyError::new(
                    "git.nested_repository",
                    "action target crosses a nested repository or submodule boundary",
                ));
            }
        }
    }
    Ok(())
}

fn fingerprint(
    metadata: Option<&fs::Metadata>,
    path: &Path,
    expected_hash: Option<&str>,
) -> Result<PathFingerprint, PathSafetyError> {
    let Some(metadata) = metadata else {
        return Ok(PathFingerprint {
            metadata_hash: blake3::hash(b"missing").to_hex().to_string(),
            content_hash: None,
            bytes: 0,
            modified_unix_nanos: 0,
        });
    };
    let modified_unix_nanos = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_nanos());
    let metadata_value = format!(
        "{}:{}:{}:{}",
        metadata.len(),
        modified_unix_nanos,
        metadata.is_file(),
        metadata_attributes(metadata)
    );
    let content_hash = if metadata.is_file()
        && (expected_hash.is_some() || metadata.len() <= MAX_POLICY_HASH_FILE_BYTES)
    {
        if metadata.len() > MAX_POLICY_HASH_FILE_BYTES {
            return Err(PathSafetyError::new(
                "path.hash_limit",
                "file is too large for a policy content-hash precondition",
            ));
        }
        let bytes = fs::read(path).map_err(|_| {
            PathSafetyError::new(
                "path.read_failed",
                "file cannot be hashed for policy validation",
            )
        })?;
        let actual = blake3::hash(&bytes).to_hex().to_string();
        if expected_hash.is_some_and(|expected| expected != actual) {
            return Err(PathSafetyError::new(
                "path.hash_mismatch",
                "file content no longer matches the expected hash",
            ));
        }
        Some(actual)
    } else {
        None
    };
    Ok(PathFingerprint {
        metadata_hash: blake3::hash(metadata_value.as_bytes()).to_hex().to_string(),
        content_hash,
        bytes: metadata.len(),
        modified_unix_nanos,
    })
}

#[cfg(windows)]
fn metadata_is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
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
