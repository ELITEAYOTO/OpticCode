use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};

use super::schema::{
    RagExclusionRecord, RagPosition, RagRuleDescriptor, RagSourceManifest, RAG_POLICY_VERSION,
};
use super::secrets::{detect_secret, secret_rule_descriptors};

pub(crate) const DEFAULT_RAG_MAX_FILE_BYTES: u64 = 512 * 1024;
pub(crate) const MAX_RAG_ENTRIES: usize = 500_000;
pub(crate) const MAX_RAG_EXCLUSIONS: usize = 100_000;

pub(crate) const ALLOWED_EXTENSIONS: &[&str] = &[
    "gradle",
    "groovy",
    "java",
    "json",
    "kt",
    "kts",
    "lang",
    "mcmeta",
    "md",
    "patch",
    "properties",
    "rs",
    "toml",
    "txt",
    "xml",
    "yaml",
    "yml",
];

const DENIED_DIRECTORIES: &[(&str, &str, &str)] = &[
    (".git", "path.directory.git", "repository_metadata"),
    (".opticcode", "path.directory.opticcode", "agent_metadata"),
    (".gradle", "path.directory.gradle", "build_cache"),
    (".idea", "path.directory.idea", "editor_metadata"),
    (".settings", "path.directory.settings", "editor_metadata"),
    (".vscode", "path.directory.vscode", "editor_metadata"),
    (".cache", "path.directory.cache", "cache"),
    (".cargo", "path.directory.cargo", "credential_or_cache"),
    (".m2", "path.directory.maven", "credential_or_cache"),
    (".npm", "path.directory.npm", "credential_or_cache"),
    (".ssh", "path.directory.ssh", "private_key"),
    (".next", "path.directory.next", "build_output"),
    (".pytest_cache", "path.directory.pytest", "cache"),
    (".venv", "path.directory.venv", "dependency_cache"),
    ("__pycache__", "path.directory.pycache", "cache"),
    ("target", "path.directory.target", "build_output"),
    ("build", "path.directory.build", "build_output"),
    ("bin", "path.directory.bin", "build_output"),
    ("classes", "path.directory.classes", "build_output"),
    ("out", "path.directory.out", "build_output"),
    ("dist", "path.directory.dist", "build_output"),
    ("coverage", "path.directory.coverage", "generated_output"),
    ("lib", "path.directory.lib", "dependency_cache"),
    ("libs", "path.directory.libs", "dependency_cache"),
    (
        "node_modules",
        "path.directory.node_modules",
        "dependency_cache",
    ),
    ("models", "path.directory.models", "model_artifact"),
    ("data", "path.directory.data", "generated_or_private_data"),
    (
        "idées-vrac",
        "path.directory.private_notes",
        "private_notes",
    ),
    (
        "idees-vrac",
        "path.directory.private_notes",
        "private_notes",
    ),
];

#[derive(Debug, Clone)]
pub(crate) struct RagSourceDescriptor {
    pub root: PathBuf,
    pub manifest: RagSourceManifest,
}

#[derive(Debug, Clone)]
pub(crate) struct RagCandidate {
    pub absolute_path: PathBuf,
    pub relative_path: String,
    pub content_type: String,
    pub allow_rule: String,
}

#[derive(Debug)]
pub(crate) struct RagInventory {
    pub total_files: usize,
    pub skipped_large_files: usize,
    pub extensions: BTreeMap<String, usize>,
    pub important_files: Vec<PathBuf>,
    pub candidates: Vec<RagCandidate>,
    pub exclusions: Vec<RagExclusionRecord>,
    pub scan_us: u64,
}

#[derive(Debug)]
pub(crate) struct StableContent {
    pub text: String,
    pub bytes: u64,
    pub chars: usize,
    pub blake3: String,
    pub source_modified_unix_ms: Option<u64>,
}

#[derive(Debug)]
pub(crate) struct RagReadOutcome {
    pub content: Option<StableContent>,
    pub exclusion: Option<RagExclusionRecord>,
    pub stable_read_us: u64,
    pub secret_scan_us: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileFingerprint {
    len: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    attributes: u32,
}

pub(crate) fn prepare_sources(roots: &[PathBuf]) -> Result<Vec<RagSourceDescriptor>> {
    if roots.is_empty() {
        bail!("at least one explicitly authorized RAG source root is required");
    }

    let mut sources = Vec::with_capacity(roots.len());
    for root in roots {
        let original_metadata = fs::symlink_metadata(root)
            .with_context(|| format!("failed to inspect RAG source root: {}", root.display()))?;
        if metadata_is_link_or_reparse(&original_metadata) || !original_metadata.is_dir() {
            bail!(
                "RAG source root must be a real directory, not a symlink or reparse point: {}",
                root.display()
            );
        }

        let canonical = fs::canonicalize(root)
            .with_context(|| format!("failed to resolve RAG source root: {}", root.display()))?;
        let canonical_metadata = fs::symlink_metadata(&canonical).with_context(|| {
            format!(
                "failed to inspect canonical RAG source root: {}",
                canonical.display()
            )
        })?;
        if metadata_is_link_or_reparse(&canonical_metadata) || !canonical_metadata.is_dir() {
            bail!(
                "canonical RAG source root is unsafe: {}",
                canonical.display()
            );
        }

        let root_fingerprint = blake3::hash(normalized_absolute(&canonical).as_bytes())
            .to_hex()
            .to_string();
        let source = format!("source-{}", &root_fingerprint[..16]);
        let source_kind = detect_source_kind(&canonical);
        let collection = format!("{}-{}", source_kind, &root_fingerprint[..12]);
        let source_label = canonical
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("root")
            .to_string();
        sources.push(RagSourceDescriptor {
            root: canonical,
            manifest: RagSourceManifest {
                collection,
                profile: RAG_POLICY_VERSION.to_string(),
                source,
                source_label,
                source_kind,
                root_fingerprint,
                source_version: None,
            },
        });
    }

    sources.sort_by(|left, right| left.manifest.source.cmp(&right.manifest.source));
    for left_index in 0..sources.len() {
        for right_index in left_index + 1..sources.len() {
            let left = &sources[left_index].root;
            let right = &sources[right_index].root;
            if left == right || left.starts_with(right) || right.starts_with(left) {
                bail!(
                    "RAG source roots must be distinct and non-overlapping: {} and {}",
                    left.display(),
                    right.display()
                );
            }
        }
    }
    Ok(sources)
}

pub(crate) fn collect_inventory(source: &RagSourceDescriptor) -> Result<RagInventory> {
    let started = Instant::now();
    let mut directories = vec![source.root.clone()];
    let mut total_entries = 0usize;
    let mut total_files = 0usize;
    let mut skipped_large_files = 0usize;
    let mut extensions = BTreeMap::new();
    let mut important_files = Vec::new();
    let mut candidates = Vec::new();
    let mut exclusions = Vec::new();

    while let Some(directory) = directories.pop() {
        let mut entries = fs::read_dir(&directory)
            .with_context(|| format!("failed to enumerate RAG directory: {}", directory.display()))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_ascii_lowercase());

        for entry in entries {
            total_entries = total_entries.saturating_add(1);
            if total_entries > MAX_RAG_ENTRIES {
                bail!("RAG source exceeds the bounded entry limit of {MAX_RAG_ENTRIES}");
            }

            let path = entry.path();
            let relative = match path.strip_prefix(&source.root) {
                Ok(value) => value,
                Err(_) => {
                    push_exclusion(
                        &mut exclusions,
                        exclusion(
                            source,
                            Path::new("<outside-root>"),
                            "unknown",
                            "path.escape",
                            "path_safety",
                            None,
                        ),
                    )?;
                    continue;
                }
            };
            let metadata = match fs::symlink_metadata(&path) {
                Ok(value) => value,
                Err(_) => {
                    push_exclusion(
                        &mut exclusions,
                        exclusion(
                            source,
                            relative,
                            "unknown",
                            "io.metadata_failed",
                            "io",
                            None,
                        ),
                    )?;
                    continue;
                }
            };

            if metadata_is_link_or_reparse(&metadata) {
                push_exclusion(
                    &mut exclusions,
                    exclusion(
                        source,
                        relative,
                        if metadata.is_dir() {
                            "directory"
                        } else {
                            "file"
                        },
                        "path.symlink_or_reparse",
                        "path_safety",
                        None,
                    ),
                )?;
                continue;
            }

            if metadata.is_dir() {
                if let Some((rule_id, category)) = denied_relative_directory(relative)
                    .or_else(|| denied_directory(entry.file_name().to_string_lossy().as_ref()))
                {
                    push_exclusion(
                        &mut exclusions,
                        exclusion(source, relative, "directory", rule_id, category, None),
                    )?;
                } else {
                    directories.push(path);
                }
                continue;
            }
            if !metadata.is_file() {
                push_exclusion(
                    &mut exclusions,
                    exclusion(
                        source,
                        relative,
                        "other",
                        "path.unsupported_entry",
                        "path_safety",
                        None,
                    ),
                )?;
                continue;
            }

            total_files += 1;
            let extension = extension_label(&path);
            *extensions.entry(extension.clone()).or_insert(0) += 1;

            if let Some((rule_id, category)) = sensitive_file_name(relative) {
                push_exclusion(
                    &mut exclusions,
                    exclusion(source, relative, "file", rule_id, category, None),
                )?;
                continue;
            }

            let Some((content_type, allow_rule)) = allowlisted_content_type(&extension) else {
                push_exclusion(
                    &mut exclusions,
                    exclusion(
                        source,
                        relative,
                        "file",
                        if extension == "<none>" {
                            "type.extensionless"
                        } else {
                            "type.not_allowlisted"
                        },
                        "content_type",
                        None,
                    ),
                )?;
                continue;
            };

            if metadata.len() > DEFAULT_RAG_MAX_FILE_BYTES {
                skipped_large_files += 1;
                push_exclusion(
                    &mut exclusions,
                    exclusion(
                        source,
                        relative,
                        "file",
                        "size.too_large",
                        "resource_limit",
                        None,
                    ),
                )?;
                continue;
            }

            let Some(relative_path) = relative.to_str().map(|value| value.replace('\\', "/"))
            else {
                push_exclusion(
                    &mut exclusions,
                    exclusion(
                        source,
                        relative,
                        "file",
                        "path.non_utf8",
                        "path_safety",
                        None,
                    ),
                )?;
                continue;
            };
            if is_important_file(relative) {
                important_files.push(relative.to_path_buf());
            }
            candidates.push(RagCandidate {
                absolute_path: path,
                relative_path,
                content_type: content_type.to_string(),
                allow_rule: allow_rule.to_string(),
            });
        }
    }

    candidates.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    important_files.sort();
    exclusions.sort_by(|left, right| {
        left.relative_path
            .cmp(&right.relative_path)
            .then_with(|| left.rule_id.cmp(&right.rule_id))
    });
    Ok(RagInventory {
        total_files,
        skipped_large_files,
        extensions,
        important_files,
        candidates,
        exclusions,
        scan_us: duration_us(started.elapsed()),
    })
}

pub(crate) fn read_stable_candidate(
    source: &RagSourceDescriptor,
    candidate: &RagCandidate,
) -> RagReadOutcome {
    read_stable_candidate_with_hook(source, candidate, |_| {})
}

pub(crate) fn read_stable_candidate_with_hook<F>(
    source: &RagSourceDescriptor,
    candidate: &RagCandidate,
    after_read: F,
) -> RagReadOutcome
where
    F: FnOnce(&Path),
{
    let read_started = Instant::now();
    let excluded = |rule_id: &str, category: &str| RagReadOutcome {
        content: None,
        exclusion: Some(exclusion(
            source,
            Path::new(&candidate.relative_path),
            "file",
            rule_id,
            category,
            None,
        )),
        stable_read_us: duration_us(read_started.elapsed()),
        secret_scan_us: 0,
    };

    let before_metadata = match fs::symlink_metadata(&candidate.absolute_path) {
        Ok(value) if value.is_file() && !metadata_is_link_or_reparse(&value) => value,
        _ => return excluded("path.symlink_or_reparse", "path_safety"),
    };
    if before_metadata.len() > DEFAULT_RAG_MAX_FILE_BYTES {
        return excluded("size.too_large", "resource_limit");
    }
    let before_fingerprint = fingerprint(&before_metadata);

    let before_canonical = match fs::canonicalize(&candidate.absolute_path) {
        Ok(value) if value.strip_prefix(&source.root).is_ok() => value,
        _ => return excluded("path.escape", "path_safety"),
    };
    if path_has_link_or_reparse(&source.root, &before_canonical).unwrap_or(true) {
        return excluded("path.symlink_or_reparse", "path_safety");
    }

    let mut file = match File::open(&candidate.absolute_path) {
        Ok(value) => value,
        Err(_) => return excluded("io.read_failed", "io"),
    };
    let handle_before = match file.metadata() {
        Ok(value) => fingerprint(&value),
        Err(_) => return excluded("io.metadata_failed", "io"),
    };
    if handle_before != before_fingerprint {
        return excluded("content.changed_during_read", "consistency");
    }

    let mut bytes = Vec::with_capacity(before_metadata.len() as usize);
    if file
        .by_ref()
        .take(DEFAULT_RAG_MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return excluded("io.read_failed", "io");
    }
    after_read(&candidate.absolute_path);

    if bytes.len() as u64 > DEFAULT_RAG_MAX_FILE_BYTES {
        return excluded("size.too_large", "resource_limit");
    }
    let handle_after = match file.metadata() {
        Ok(value) => fingerprint(&value),
        Err(_) => return excluded("io.metadata_failed", "io"),
    };
    let path_after = match fs::symlink_metadata(&candidate.absolute_path) {
        Ok(value) if value.is_file() && !metadata_is_link_or_reparse(&value) => value,
        _ => return excluded("path.symlink_or_reparse", "path_safety"),
    };
    let after_canonical = match fs::canonicalize(&candidate.absolute_path) {
        Ok(value) if value.strip_prefix(&source.root).is_ok() => value,
        _ => return excluded("path.escape", "path_safety"),
    };
    if before_canonical != after_canonical
        || before_fingerprint != handle_after
        || before_fingerprint != fingerprint(&path_after)
    {
        return excluded("content.changed_during_read", "consistency");
    }

    let text = match String::from_utf8(bytes) {
        Ok(value) => value,
        Err(_) => return excluded("content.invalid_utf8", "content_type"),
    };
    let stable_read_us = duration_us(read_started.elapsed());
    let secret_started = Instant::now();
    let detection = detect_secret(&text);
    let secret_scan_us = duration_us(secret_started.elapsed());
    if let Some(detection) = detection {
        return RagReadOutcome {
            content: None,
            exclusion: Some(exclusion(
                source,
                Path::new(&candidate.relative_path),
                "file",
                detection.rule_id,
                detection.category,
                Some(RagPosition {
                    line: detection.line,
                    column: detection.column,
                }),
            )),
            stable_read_us,
            secret_scan_us,
        };
    }

    let bytes = text.len() as u64;
    let chars = text.chars().count();
    let blake3 = blake3::hash(text.as_bytes()).to_hex().to_string();
    RagReadOutcome {
        content: Some(StableContent {
            text,
            bytes,
            chars,
            blake3,
            source_modified_unix_ms: modified_unix_ms(before_metadata.modified().ok()),
        }),
        exclusion: None,
        stable_read_us,
        secret_scan_us,
    }
}

pub(crate) fn exclusion_rules() -> Vec<RagRuleDescriptor> {
    let mut rules = vec![
        rule("path.symlink_or_reparse", "path_safety"),
        rule("path.escape", "path_safety"),
        rule("path.non_utf8", "path_safety"),
        rule("path.unsupported_entry", "path_safety"),
        rule("path.directory.rag_staging", "generated_output"),
        rule("path.directory.benchmark_runs", "generated_output"),
        rule("type.extensionless", "content_type"),
        rule("type.not_allowlisted", "content_type"),
        rule("size.too_large", "resource_limit"),
        rule("content.invalid_utf8", "content_type"),
        rule("content.changed_during_read", "consistency"),
        rule("io.metadata_failed", "io"),
        rule("io.read_failed", "io"),
        rule("path.file.environment", "credential_file"),
        rule("path.file.credential", "credential_file"),
        rule("path.file.private_key", "private_key"),
        rule("path.file.credential_store", "credential_store"),
    ];
    for (_, rule_id, category) in DENIED_DIRECTORIES {
        rules.push(rule(rule_id, category));
    }
    rules.extend(secret_rule_descriptors());
    rules.sort_by(|left, right| left.rule_id.cmp(&right.rule_id));
    rules.dedup_by(|left, right| left.rule_id == right.rule_id);
    rules
}

pub(crate) fn configuration_allowed_extensions() -> Vec<String> {
    ALLOWED_EXTENSIONS
        .iter()
        .map(|value| (*value).to_string())
        .collect()
}

pub(crate) fn record_matches_allow_policy(
    relative_path: &str,
    content_type: &str,
    allow_rule: &str,
) -> bool {
    let path = Path::new(relative_path);
    if sensitive_file_name(path).is_some() {
        return false;
    }
    let mut components = path.components().peekable();
    while let Some(component) = components.next() {
        if components.peek().is_some()
            && component
                .as_os_str()
                .to_str()
                .is_none_or(|name| denied_directory(name).is_some())
        {
            return false;
        }
    }
    let extension = extension_label(path);
    allowlisted_content_type(&extension).is_some_and(|(expected_type, expected_rule)| {
        content_type == expected_type && allow_rule == expected_rule
    })
}

pub(crate) fn denied_directory(name: &str) -> Option<(&'static str, &'static str)> {
    let lower = name.to_lowercase();
    if lower.starts_with(".staging-") || lower.starts_with(".opticcode-rag-") {
        return Some(("path.directory.rag_staging", "generated_output"));
    }
    DENIED_DIRECTORIES
        .iter()
        .find(|(candidate, _, _)| lower == *candidate)
        .map(|(_, rule_id, category)| (*rule_id, *category))
}

pub(crate) fn denied_relative_directory(path: &Path) -> Option<(&'static str, &'static str)> {
    let normalized = normalized_relative(path).to_lowercase();
    if normalized == "benchmarks/runs" || normalized.ends_with("/benchmarks/runs") {
        return Some(("path.directory.benchmark_runs", "generated_output"));
    }
    None
}

pub(crate) fn sensitive_file_name(path: &Path) -> Option<(&'static str, &'static str)> {
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    if name == ".env" || name.starts_with(".env.") {
        return Some(("path.file.environment", "credential_file"));
    }
    if matches!(
        name.as_str(),
        ".npmrc" | ".pypirc" | ".netrc" | "_netrc" | "auth.json"
    ) {
        return Some(("path.file.credential_store", "credential_store"));
    }
    if matches!(
        name.as_str(),
        "id_rsa"
            | "id_rsa.pub"
            | "id_dsa"
            | "id_dsa.pub"
            | "id_ecdsa"
            | "id_ecdsa.pub"
            | "id_ed25519"
            | "id_ed25519.pub"
    ) {
        return Some(("path.file.private_key", "private_key"));
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());
    if extension.as_deref().is_some_and(|value| {
        matches!(
            value,
            "pem" | "key" | "p12" | "pfx" | "jks" | "keystore" | "kdbx"
        )
    }) {
        return Some(("path.file.private_key", "private_key"));
    }
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if [
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
    {
        return Some(("path.file.credential", "credential_file"));
    }
    None
}

fn allowlisted_content_type(extension: &str) -> Option<(&'static str, &'static str)> {
    let content_type = match extension {
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "groovy" | "gradle" => "gradle",
        "xml" => "xml",
        "yml" | "yaml" => "yaml",
        "properties" => "properties",
        "json" | "mcmeta" => "json",
        "lang" => "minecraft_lang",
        "md" => "markdown",
        "txt" => "text",
        "patch" => "patch",
        "toml" => "toml",
        "rs" => "rust",
        _ => return None,
    };
    Some((content_type, allow_rule_for_extension(extension)))
}

fn allow_rule_for_extension(extension: &str) -> &'static str {
    match extension {
        "java" => "allow.extension.java",
        "kt" => "allow.extension.kt",
        "kts" => "allow.extension.kts",
        "groovy" => "allow.extension.groovy",
        "gradle" => "allow.extension.gradle",
        "xml" => "allow.extension.xml",
        "yml" => "allow.extension.yml",
        "yaml" => "allow.extension.yaml",
        "properties" => "allow.extension.properties",
        "json" => "allow.extension.json",
        "mcmeta" => "allow.extension.mcmeta",
        "lang" => "allow.extension.lang",
        "md" => "allow.extension.md",
        "txt" => "allow.extension.txt",
        "patch" => "allow.extension.patch",
        "toml" => "allow.extension.toml",
        "rs" => "allow.extension.rs",
        _ => "allow.extension.unknown",
    }
}

fn extension_label(path: &Path) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_else(|| "<none>".to_string())
}

fn exclusion(
    source: &RagSourceDescriptor,
    relative: &Path,
    entry_kind: &str,
    rule_id: &str,
    category: &str,
    position: Option<RagPosition>,
) -> RagExclusionRecord {
    RagExclusionRecord {
        collection: source.manifest.collection.clone(),
        source: source.manifest.source.clone(),
        relative_path: normalized_relative(relative),
        entry_kind: entry_kind.to_string(),
        rule_id: rule_id.to_string(),
        category: category.to_string(),
        decision: "excluded".to_string(),
        position,
    }
}

fn push_exclusion(
    exclusions: &mut Vec<RagExclusionRecord>,
    exclusion: RagExclusionRecord,
) -> Result<()> {
    if exclusions.len() >= MAX_RAG_EXCLUSIONS {
        bail!("RAG source exceeds the bounded exclusion limit of {MAX_RAG_EXCLUSIONS}");
    }
    exclusions.push(exclusion);
    Ok(())
}

fn path_has_link_or_reparse(root: &Path, absolute: &Path) -> Result<bool> {
    let relative = absolute
        .strip_prefix(root)
        .context("RAG candidate escaped its canonical source root")?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        let metadata = fs::symlink_metadata(&current).with_context(|| {
            format!(
                "failed to inspect RAG path component: {}",
                current.display()
            )
        })?;
        if metadata_is_link_or_reparse(&metadata) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn metadata_is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    metadata_has_reparse_attribute(metadata)
}

#[cfg(windows)]
fn metadata_has_reparse_attribute(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_has_reparse_attribute(_metadata: &fs::Metadata) -> bool {
    false
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

fn detect_source_kind(root: &Path) -> String {
    let normalized = normalized_absolute(root).to_ascii_lowercase();
    if root.join("pack.mcmeta").exists() {
        "resource-pack".to_string()
    } else if normalized.contains("pandaspigot") {
        "pandaspigot".to_string()
    } else if root.join("src/main/resources/plugin.yml").exists() {
        "plugin".to_string()
    } else if root.join("Cargo.toml").exists() && root.join("docs").exists() {
        "opticcode".to_string()
    } else {
        "external".to_string()
    }
}

fn is_important_file(path: &Path) -> bool {
    let normalized = normalized_relative(path).to_ascii_lowercase();
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        file_name.as_str(),
        "pom.xml"
            | "plugin.yml"
            | "paper-plugin.yml"
            | "bukkit.yml"
            | "build.gradle"
            | "build.gradle.kts"
            | "gradle.properties"
            | "settings.gradle"
            | "settings.gradle.kts"
            | "readme.md"
    ) || normalized.starts_with("patches/")
        || normalized.contains("/patches/")
        || normalized.starts_with("src/main/resources/")
}

fn normalized_relative(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn normalized_absolute(path: &Path) -> String {
    normalized_relative(path)
}

fn modified_unix_ms(value: Option<SystemTime>) -> Option<u64> {
    value
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
}

fn duration_us(value: std::time::Duration) -> u64 {
    value.as_micros().min(u128::from(u64::MAX)) as u64
}

fn rule(rule_id: &str, category: &str) -> RagRuleDescriptor {
    RagRuleDescriptor {
        rule_id: rule_id.to_string(),
        category: category.to_string(),
        decision: "exclude".to_string(),
    }
}

pub(crate) fn ensure_unique_paths(records: &[RagExclusionRecord]) -> bool {
    let mut seen = HashSet::new();
    records.iter().all(|record| {
        seen.insert((
            record.source.as_str(),
            record.relative_path.as_str(),
            record.rule_id.as_str(),
        ))
    })
}
