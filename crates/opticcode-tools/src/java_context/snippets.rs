use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::java_index::{JavaIndexFile, JavaIndexProjectReport};
use crate::java_syntax::{SourcePoint, SourceRange};

use super::schema::{
    estimate_tokens, JavaContextCandidate, JavaContextMatchKind, JavaContextSnippet,
    JavaContextSnippetRole,
};
use super::JAVA_CONTEXT_SNIPPET_REASON_LIMIT;

pub(super) struct SnippetBuildResult {
    pub snippets: Vec<JavaContextSnippet>,
    pub snippets_truncated: bool,
    pub omitted_snippets: usize,
    pub source_reads: usize,
    pub source_read_errors: usize,
    pub source_hash_mismatches: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct ProjectFileSelection {
    pub build_manifest: bool,
    pub bukkit_descriptor: bool,
}

struct SourceFile {
    content: String,
    hash: String,
}

pub(super) fn build_snippets(
    index: &JavaIndexProjectReport,
    candidates: &[JavaContextCandidate],
    primary_symbols: &[String],
    max_snippets: usize,
    max_snippet_bytes: usize,
    project_files: ProjectFileSelection,
) -> SnippetBuildResult {
    let files = index
        .files
        .iter()
        .map(|file| (file.path.clone(), file))
        .collect::<BTreeMap<_, _>>();
    let mut cache = BTreeMap::<PathBuf, SourceFile>::new();
    let mut failed_files = BTreeSet::<PathBuf>::new();
    let mut snippets = Vec::new();
    let mut warnings = Vec::new();
    let mut source_reads = 0usize;
    let mut source_read_errors = 0usize;
    let mut source_hash_mismatches = 0usize;
    let mut selected_ranges = Vec::<(PathBuf, SourceRange)>::new();
    let potential_project_files = discover_project_files(&index.root, project_files);
    let max_project_file_bytes = usize::try_from(index.limits.max_file_bytes).unwrap_or(usize::MAX);
    let java_snippet_limit =
        max_snippets.saturating_sub(potential_project_files.len().min(max_snippets));
    let mut omitted_snippets = 0usize;
    let mut limit_reached = false;

    for (position, candidate) in candidates.iter().enumerate() {
        if snippets.len() >= java_snippet_limit {
            omitted_snippets = omitted_snippets.saturating_add(candidates.len() - position);
            limit_reached = true;
            break;
        }
        let role = candidate_role(primary_symbols, candidate);
        if role == JavaContextSnippetRole::SupportingDeclaration
            && selected_ranges.iter().any(|(path, range)| {
                path == &candidate.file
                    && (contains_range(candidate.range, *range)
                        || contains_range(*range, candidate.range))
            })
        {
            continue;
        }
        let Some(file) = files.get(&candidate.file).copied() else {
            source_read_errors += 1;
            warnings.push(format!(
                "context candidate {} has no Java index file record",
                candidate.symbol_id
            ));
            continue;
        };
        if failed_files.contains(&candidate.file) {
            continue;
        }
        if !cache.contains_key(&candidate.file) {
            source_reads += 1;
            match read_java_source(&index.root, file, index.limits.max_file_bytes) {
                Ok(source) if source.hash == candidate.source_hash => {
                    cache.insert(candidate.file.clone(), source);
                }
                Ok(source) => {
                    source_hash_mismatches += 1;
                    warnings.push(format!(
                        "Java source changed after indexing: {} (expected {}, found {})",
                        candidate.file.display(),
                        candidate.source_hash,
                        source.hash
                    ));
                    failed_files.insert(candidate.file.clone());
                    continue;
                }
                Err(error) => {
                    source_read_errors += 1;
                    warnings.push(format!(
                        "failed to read context source {}: {error}",
                        candidate.file.display()
                    ));
                    failed_files.insert(candidate.file.clone());
                    continue;
                }
            }
        }
        let Some(source) = cache.get(&candidate.file) else {
            continue;
        };
        match java_snippet(candidate, role, source, max_snippet_bytes) {
            Ok(snippet) => {
                selected_ranges.push((candidate.file.clone(), candidate.range));
                snippets.push(snippet);
            }
            Err(error) => {
                source_read_errors += 1;
                warnings.push(format!(
                    "failed to materialize context symbol {}: {error}",
                    candidate.symbol_id
                ));
            }
        }
    }

    for (path, role) in &potential_project_files {
        source_reads += 1;
        match read_project_file(&index.root, path, max_snippet_bytes, max_project_file_bytes) {
            Ok(snippet) => {
                snippets.push(project_snippet(path.clone(), *role, snippet));
            }
            Err(error) => {
                source_read_errors += 1;
                warnings.push(format!(
                    "failed to read project context file {}: {error}",
                    path.display()
                ));
            }
        }
    }

    snippets.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.role.cmp(&right.role))
            .then_with(|| normalized_path(&left.file).cmp(&normalized_path(&right.file)))
            .then_with(|| left.id.cmp(&right.id))
    });
    if snippets.len() > max_snippets {
        omitted_snippets = omitted_snippets.saturating_add(snippets.len() - max_snippets);
        snippets.truncate(max_snippets);
        limit_reached = true;
    }
    let content_truncated = snippets.iter().any(|snippet| snippet.truncated);

    SnippetBuildResult {
        snippets_truncated: limit_reached || content_truncated,
        omitted_snippets,
        snippets,
        source_reads,
        source_read_errors,
        source_hash_mismatches,
        warnings,
    }
}

fn candidate_role(
    primary_symbols: &[String],
    candidate: &JavaContextCandidate,
) -> JavaContextSnippetRole {
    if primary_symbols.contains(&candidate.symbol_id) {
        return JavaContextSnippetRole::PrimaryDeclaration;
    }
    if candidate
        .reasons
        .iter()
        .any(|reason| reason.kind == JavaContextMatchKind::CallerOfPrimary)
    {
        return JavaContextSnippetRole::Caller;
    }
    if candidate
        .reasons
        .iter()
        .any(|reason| reason.kind == JavaContextMatchKind::ReferencedByPrimary)
    {
        return JavaContextSnippetRole::RelatedDeclaration;
    }
    JavaContextSnippetRole::SupportingDeclaration
}

fn java_snippet(
    candidate: &JavaContextCandidate,
    role: JavaContextSnippetRole,
    source: &SourceFile,
    max_snippet_bytes: usize,
) -> Result<JavaContextSnippet, String> {
    let start = candidate.range.start.byte;
    let original_end = candidate.range.end.byte;
    if start >= original_end
        || original_end > source.content.len()
        || !source.content.is_char_boundary(start)
        || !source.content.is_char_boundary(original_end)
    {
        return Err("indexed AST range is outside the current UTF-8 source".to_string());
    }
    let desired_end = start.saturating_add(max_snippet_bytes).min(original_end);
    let content_end = previous_char_boundary(&source.content, desired_end, start);
    if content_end <= start {
        return Err("snippet byte limit cannot retain one UTF-8 character".to_string());
    }
    let content = source.content[start..content_end].to_string();
    let content_chars = content.chars().count();
    let content_hash = hash_bytes(content.as_bytes());
    let id_material = format!(
        "{}\0{}\0{}\0{}\0{}",
        role.as_str(),
        normalized_path(&candidate.file),
        candidate.symbol_id,
        start,
        source.hash
    );
    Ok(JavaContextSnippet {
        id: format!("java-context:{}", blake3::hash(id_material.as_bytes())),
        role,
        file: candidate.file.clone(),
        symbol_id: Some(candidate.symbol_id.clone()),
        source_hash: source.hash.clone(),
        ast_range: Some(candidate.range),
        content_range: Some(SourceRange {
            start: candidate.range.start,
            end: source_point_at(&source.content, content_end),
        }),
        score: candidate.score,
        selection_reasons: candidate
            .reasons
            .iter()
            .take(JAVA_CONTEXT_SNIPPET_REASON_LIMIT)
            .map(|reason| reason.detail.clone())
            .collect(),
        selection_reasons_truncated: candidate.reason_count > JAVA_CONTEXT_SNIPPET_REASON_LIMIT,
        original_bytes: original_end - start,
        content_bytes: content.len(),
        content_chars,
        estimated_tokens: estimate_tokens(&content),
        content_hash,
        truncated: content_end < original_end,
        content,
    })
}

fn project_snippet(
    path: PathBuf,
    role: JavaContextSnippetRole,
    source: ProjectFile,
) -> JavaContextSnippet {
    let content_chars = source.content.chars().count();
    let estimated_tokens = estimate_tokens(&source.content);
    let id_material = format!(
        "{}\0{}\0{}",
        role.as_str(),
        normalized_path(&path),
        source.source_hash
    );
    JavaContextSnippet {
        id: format!("java-context:{}", blake3::hash(id_material.as_bytes())),
        role,
        file: path,
        symbol_id: None,
        source_hash: source.source_hash,
        ast_range: None,
        content_range: None,
        score: match role {
            JavaContextSnippetRole::BukkitDescriptor => 2_100,
            JavaContextSnippetRole::BuildManifest => 2_000,
            _ => 0,
        },
        selection_reasons: vec![match role {
            JavaContextSnippetRole::BukkitDescriptor => {
                "Bukkit descriptor defines commands, permissions and entry point".to_string()
            }
            JavaContextSnippetRole::BuildManifest => {
                "build manifest defines Java level and dependencies".to_string()
            }
            _ => "project metadata".to_string(),
        }],
        selection_reasons_truncated: false,
        original_bytes: source.original_bytes,
        content_bytes: source.content.len(),
        content_chars,
        estimated_tokens,
        content_hash: hash_bytes(source.content.as_bytes()),
        truncated: source.truncated,
        content: source.content,
    }
}

struct ProjectFile {
    content: String,
    source_hash: String,
    original_bytes: usize,
    truncated: bool,
}

fn read_java_source(
    root: &Path,
    file: &JavaIndexFile,
    max_file_bytes: u64,
) -> Result<SourceFile, String> {
    let bytes = read_safe_file(root, &file.path, max_file_bytes as usize)?;
    let content = String::from_utf8(bytes).map_err(|_| "Java source is not UTF-8".to_string())?;
    Ok(SourceFile {
        hash: hash_bytes(content.as_bytes()),
        content,
    })
}

fn read_project_file(
    root: &Path,
    path: &Path,
    max_snippet_bytes: usize,
    max_file_bytes: usize,
) -> Result<ProjectFile, String> {
    let bytes = read_safe_file(root, path, max_file_bytes)?;
    let source_hash = hash_bytes(&bytes);
    let original_bytes = bytes.len();
    let content = String::from_utf8(bytes).map_err(|_| "project file is not UTF-8".to_string())?;
    let desired_end = content.len().min(max_snippet_bytes);
    let content_end = previous_char_boundary(&content, desired_end, 0);
    Ok(ProjectFile {
        content: content[..content_end].to_string(),
        source_hash,
        original_bytes,
        truncated: content_end < content.len(),
    })
}

fn read_safe_file(root: &Path, relative: &Path, max_bytes: usize) -> Result<Vec<u8>, String> {
    validate_relative_path(relative)?;
    let absolute = root.join(relative);
    let metadata = fs::symlink_metadata(&absolute)
        .map_err(|error| format!("metadata unavailable: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("path is not a regular non-symlink file".to_string());
    }
    if metadata.len() > max_bytes as u64 {
        return Err(format!(
            "file exceeds bounded read limit of {max_bytes} bytes"
        ));
    }
    let canonical =
        fs::canonicalize(&absolute).map_err(|error| format!("failed to resolve path: {error}"))?;
    if !canonical.starts_with(root) {
        return Err("resolved file escapes the Java project root".to_string());
    }
    let bytes = fs::read(&canonical).map_err(|error| format!("read failed: {error}"))?;
    if bytes.len() > max_bytes {
        return Err(format!(
            "file grew beyond bounded read limit of {max_bytes} bytes"
        ));
    }
    Ok(bytes)
}

fn validate_relative_path(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err("context path must be a non-empty relative path".to_string());
    }
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("context path contains a non-normal component".to_string());
    }
    Ok(())
}

fn discover_project_files(
    root: &Path,
    selection: ProjectFileSelection,
) -> Vec<(PathBuf, JavaContextSnippetRole)> {
    let manifests = ["pom.xml", "build.gradle", "build.gradle.kts"];
    let descriptors = [
        "src/main/resources/plugin.yml",
        "src/main/resources/plugin.yaml",
        "plugin.yml",
        "plugin.yaml",
    ];
    let mut files = Vec::new();
    if selection.build_manifest {
        if let Some(path) = manifests
            .iter()
            .map(PathBuf::from)
            .find(|path| root.join(path).is_file())
        {
            files.push((path, JavaContextSnippetRole::BuildManifest));
        }
    }
    if selection.bukkit_descriptor {
        if let Some(path) = descriptors
            .iter()
            .map(PathBuf::from)
            .find(|path| root.join(path).is_file())
        {
            files.push((path, JavaContextSnippetRole::BukkitDescriptor));
        }
    }
    files
}

fn previous_char_boundary(value: &str, mut index: usize, minimum: usize) -> usize {
    index = index.min(value.len());
    while index > minimum && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn source_point_at(source: &str, byte: usize) -> SourcePoint {
    let prefix = &source.as_bytes()[..byte];
    let row = prefix.iter().filter(|byte| **byte == b'\n').count();
    let column = prefix
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(prefix.len(), |position| prefix.len() - position - 1);
    SourcePoint { byte, row, column }
}

fn contains_range(outer: SourceRange, inner: SourceRange) -> bool {
    outer.start.byte <= inner.start.byte && outer.end.byte >= inner.end.byte
}

fn hash_bytes(bytes: &[u8]) -> String {
    format!("blake3:{}:{}", bytes.len(), blake3::hash(bytes))
}

fn normalized_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
