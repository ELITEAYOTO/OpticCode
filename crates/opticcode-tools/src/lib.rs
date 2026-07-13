pub mod apply_transaction;
pub mod git_state;
pub mod java_context;
pub mod java_edit_worktree;
pub mod java_edits;
pub mod java_index;
pub mod java_syntax;
pub mod process_runner;
pub mod worktree;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use apply_transaction::{
    append_apply_log_index, execute_apply_transaction, rollback_apply_transaction,
    validate_transaction_id, ApplyGitPolicy, ApplyTransactionRequest, ApplyTransactionResult,
    ApplyTransactionState, FileMutation, APPLY_TRANSACTION_SCHEMA_VERSION,
};
use git_state::{capture_git_state, BuildGitReport};
use java_edits::legacy::{
    LegacyJavaRule as LegacySymbolReplacement, LEGACY_JAVA_RULES as LEGACY_SYMBOL_REPLACEMENTS,
};
use process_runner::{
    run_process_with_cancellation, CancellationToken, ProcessLaunchMode, ProcessOutputStats,
    ProcessRequest, ProcessStatus, ProcessTermination, DEFAULT_PROCESS_OUTPUT_LIMIT_BYTES,
    DEFAULT_PROCESS_TIMEOUT,
};
use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use walkdir::{DirEntry, WalkDir};

const MAX_INSPECT_FILES: usize = 2_000;
const MAX_READ_BYTES: u64 = 512 * 1024;
const DEFAULT_CONTEXT_FILES: usize = 8;
const DEFAULT_CONTEXT_BYTES_PER_FILE: usize = 4 * 1024;
const DEFAULT_CONTEXT_TOTAL_BYTES: usize = 24 * 1024;

#[derive(Debug, Clone)]
pub struct WorkspaceReport {
    pub root: PathBuf,
    pub total_files_seen: usize,
    pub sampled_files: Vec<PathBuf>,
    pub extensions: BTreeMap<String, usize>,
    pub has_git: bool,
    pub has_maven: bool,
    pub has_gradle: bool,
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub path: PathBuf,
    pub line_number: usize,
    pub line: String,
}

#[derive(Debug, Clone)]
pub struct ProjectContext {
    pub report: WorkspaceReport,
    pub snippets: Vec<FileSnippet>,
    pub total_chars: usize,
}

#[derive(Debug, Clone)]
pub struct FileSnippet {
    pub path: PathBuf,
    pub content: String,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct JavaProjectAnalysis {
    pub root: PathBuf,
    pub build_tool: BuildTool,
    pub maven: Option<MavenAnalysis>,
    pub plugin: Option<PluginYmlAnalysis>,
    pub java_files: Vec<JavaFileAnalysis>,
    pub risks: Vec<String>,
    pub build_command: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildTool {
    Maven,
    Gradle,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct MavenAnalysis {
    pub group_id: Option<String>,
    pub artifact_id: Option<String>,
    pub version: Option<String>,
    pub source: Option<String>,
    pub target: Option<String>,
    pub dependencies: Vec<MavenDependency>,
}

#[derive(Debug, Clone)]
pub struct MavenDependency {
    pub group_id: Option<String>,
    pub artifact_id: Option<String>,
    pub version: Option<String>,
    pub scope: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PluginYmlAnalysis {
    pub name: Option<String>,
    pub version: Option<String>,
    pub main: Option<String>,
    pub has_api_version: bool,
    pub commands: Vec<String>,
    pub permissions: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct JavaFileAnalysis {
    pub path: PathBuf,
    pub package_name: Option<String>,
    pub class_name: Option<String>,
    pub is_command_executor: bool,
    pub is_listener: bool,
    pub imports: Vec<String>,
    pub registered_commands: Vec<String>,
    pub risks: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct BuildResult {
    pub root: PathBuf,
    pub command: String,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub duration: Duration,
    pub summary: Vec<String>,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub process_id: Option<u32>,
    pub process_status: ProcessStatus,
    pub timed_out: bool,
    pub cancelled: bool,
    pub timeout: Duration,
    pub output: ProcessOutputStats,
    pub termination: ProcessTermination,
    pub git_report: BuildGitReport,
}

#[derive(Debug, Clone, Copy)]
pub struct BuildOptions {
    pub fail_on_worktree_change: bool,
    pub timeout: Duration,
    pub output_limit_bytes: usize,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            fail_on_worktree_change: false,
            timeout: DEFAULT_PROCESS_TIMEOUT,
            output_limit_bytes: DEFAULT_PROCESS_OUTPUT_LIMIT_BYTES,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PatchProposal {
    pub root: PathBuf,
    pub changes: Vec<PatchFileChange>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PatchFileChange {
    pub path: PathBuf,
    pub reason: String,
    pub diff: String,
    pub original_content: Vec<u8>,
    pub proposed_content: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PatchCheckResult {
    pub command: String,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone)]
pub struct ApplyPlan {
    pub proposal: PatchProposal,
    pub check: Option<PatchCheckResult>,
    pub apply: Option<PatchCheckResult>,
    pub copied_from: Option<PathBuf>,
    pub dry_run: bool,
    pub log: Option<ApplyLogEntry>,
    pub transaction: Option<ApplyTransactionResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyLogEntry {
    #[serde(default = "apply_log_schema_version")]
    pub schema_version: u32,
    pub run_id: String,
    pub applied_at_unix_ms: u128,
    pub project_root: String,
    pub copied_from: Option<String>,
    pub change_count: usize,
    pub files: Vec<String>,
    pub patch_path: String,
    pub rollback_command: String,
    #[serde(default)]
    pub transaction_state: Option<ApplyTransactionState>,
}

#[derive(Debug, Clone)]
pub struct ApplyUndoResult {
    pub root: PathBuf,
    pub run_id: String,
    pub patch_path: PathBuf,
    pub check: PatchCheckResult,
    pub undo: Option<PatchCheckResult>,
    pub transaction: Option<ApplyTransactionResult>,
}

#[derive(Debug, Clone)]
pub struct ResourcePackReport {
    pub root: PathBuf,
    pub total_files: usize,
    pub has_pack_mcmeta: bool,
    pub categories: BTreeMap<String, usize>,
    pub legacy_matches: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct RagSourceReport {
    pub root: PathBuf,
    pub total_files: usize,
    pub indexable_files: usize,
    pub skipped_large_files: usize,
    pub indexable_bytes: u64,
    pub extensions: BTreeMap<String, usize>,
    pub important_files: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct RagIndexReport {
    pub output_dir: PathBuf,
    pub sources: usize,
    pub documents: usize,
    pub chunks: usize,
    pub indexed_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct RagSearchHit {
    pub document_path: String,
    pub chunk_id: String,
    pub score: usize,
    pub preview: String,
}

#[derive(Debug, Clone, Serialize)]
struct RagDocumentRecord {
    id: String,
    source_root: String,
    source_kind: String,
    relative_path: String,
    extension: String,
    bytes: u64,
    chars: usize,
    content_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RagChunkRecord {
    id: String,
    document_id: String,
    source_kind: String,
    source_root: String,
    relative_path: String,
    chunk_index: usize,
    text: String,
}

pub fn inspect_workspace(root: &Path) -> Result<WorkspaceReport> {
    let root = fs::canonicalize(root)
        .with_context(|| format!("failed to resolve workspace path: {}", root.display()))?;
    let mut report = WorkspaceReport {
        has_git: root.join(".git").exists(),
        has_maven: root.join("pom.xml").exists(),
        has_gradle: root.join("build.gradle").exists() || root.join("build.gradle.kts").exists(),
        root: root.clone(),
        total_files_seen: 0,
        sampled_files: Vec::new(),
        extensions: BTreeMap::new(),
    };

    for entry in WalkDir::new(&root)
        .into_iter()
        .filter_entry(should_enter)
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }

        report.total_files_seen += 1;

        if report.sampled_files.len() < MAX_INSPECT_FILES {
            report.sampled_files.push(to_relative(&root, entry.path()));
        }

        let extension = entry
            .path()
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase())
            .unwrap_or_else(|| "<none>".to_string());
        *report.extensions.entry(extension).or_insert(0) += 1;
    }

    Ok(report)
}

pub fn inspect_resource_pack(root: &Path, limit: usize) -> Result<ResourcePackReport> {
    let root = fs::canonicalize(root)
        .with_context(|| format!("failed to resolve resource pack path: {}", root.display()))?;
    let mut report = ResourcePackReport {
        has_pack_mcmeta: root.join("pack.mcmeta").exists(),
        root: root.clone(),
        total_files: 0,
        categories: BTreeMap::new(),
        legacy_matches: Vec::new(),
    };

    for entry in WalkDir::new(&root)
        .into_iter()
        .filter_entry(should_enter_resource_pack)
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }

        report.total_files += 1;
        let relative = to_relative(&root, entry.path());
        let normalized = relative.to_string_lossy().replace('\\', "/");
        let category = resource_pack_category(&normalized);
        *report.categories.entry(category).or_insert(0) += 1;

        if report.legacy_matches.len() < limit && is_legacy_resource_match(&normalized) {
            report.legacy_matches.push(relative);
        }
    }

    Ok(report)
}

pub fn inspect_rag_source(root: &Path, limit: usize) -> Result<RagSourceReport> {
    let root = fs::canonicalize(root)
        .with_context(|| format!("failed to resolve RAG source path: {}", root.display()))?;
    let mut report = RagSourceReport {
        root: root.clone(),
        total_files: 0,
        indexable_files: 0,
        skipped_large_files: 0,
        indexable_bytes: 0,
        extensions: BTreeMap::new(),
        important_files: Vec::new(),
    };

    for entry in WalkDir::new(&root)
        .into_iter()
        .filter_entry(should_enter_rag_source)
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }

        report.total_files += 1;
        let relative = to_relative(&root, entry.path());

        let extension = entry
            .path()
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase())
            .unwrap_or_else(|| "<none>".to_string());
        *report.extensions.entry(extension).or_insert(0) += 1;

        if is_important_rag_file(&relative) && report.important_files.len() < limit {
            report.important_files.push(relative.clone());
        }

        if !is_rag_indexable_text(entry.path()) {
            continue;
        }

        let metadata = match entry.metadata() {
            Ok(value) => value,
            Err(_) => continue,
        };

        if metadata.len() > MAX_READ_BYTES {
            report.skipped_large_files += 1;
            continue;
        }

        report.indexable_files += 1;
        report.indexable_bytes += metadata.len();
    }

    Ok(report)
}

pub fn build_rag_index(
    roots: &[PathBuf],
    output_dir: &Path,
    chunk_chars: usize,
) -> Result<RagIndexReport> {
    let output_dir = fs::canonicalize(output_dir).unwrap_or_else(|_| output_dir.to_path_buf());
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create index directory: {}", output_dir.display()))?;

    let documents_path = output_dir.join("documents.jsonl");
    let chunks_path = output_dir.join("chunks.jsonl");
    let mut documents = BufWriter::new(
        File::create(&documents_path)
            .with_context(|| format!("failed to create {}", documents_path.display()))?,
    );
    let mut chunks = BufWriter::new(
        File::create(&chunks_path)
            .with_context(|| format!("failed to create {}", chunks_path.display()))?,
    );

    let mut report = RagIndexReport {
        output_dir: output_dir.clone(),
        sources: roots.len(),
        documents: 0,
        chunks: 0,
        indexed_bytes: 0,
    };

    for root in roots {
        let root = fs::canonicalize(root)
            .with_context(|| format!("failed to resolve RAG source path: {}", root.display()))?;
        let source_kind = detect_rag_source_kind(&root);

        for entry in WalkDir::new(&root)
            .into_iter()
            .filter_entry(should_enter_rag_source)
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() || !is_rag_indexable_text(entry.path()) {
                continue;
            }

            let metadata = match entry.metadata() {
                Ok(value) => value,
                Err(_) => continue,
            };
            if metadata.len() > MAX_READ_BYTES {
                continue;
            }

            let content = match fs::read_to_string(entry.path()) {
                Ok(value) => value,
                Err(_) => continue,
            };
            let relative = to_relative(&root, entry.path());
            let relative_path = relative.to_string_lossy().replace('\\', "/");
            let source_root = root.to_string_lossy().to_string();
            let content_hash = stable_hash_hex(&content);
            let document_id =
                stable_hash_hex(&format!("{source_root}:{relative_path}:{content_hash}"));
            let extension = entry
                .path()
                .extension()
                .and_then(|value| value.to_str())
                .map(|value| value.to_ascii_lowercase())
                .unwrap_or_else(|| "<none>".to_string());

            let document = RagDocumentRecord {
                id: document_id.clone(),
                source_root: source_root.clone(),
                source_kind: source_kind.clone(),
                relative_path: relative_path.clone(),
                extension,
                bytes: metadata.len(),
                chars: content.chars().count(),
                content_hash,
            };
            serde_json::to_writer(&mut documents, &document)?;
            documents.write_all(b"\n")?;

            for (chunk_index, text) in chunk_text(&content, chunk_chars).into_iter().enumerate() {
                let chunk = RagChunkRecord {
                    id: format!("{document_id}:{chunk_index}"),
                    document_id: document_id.clone(),
                    source_kind: source_kind.clone(),
                    source_root: source_root.clone(),
                    relative_path: relative_path.clone(),
                    chunk_index,
                    text,
                };
                serde_json::to_writer(&mut chunks, &chunk)?;
                chunks.write_all(b"\n")?;
                report.chunks += 1;
            }

            report.documents += 1;
            report.indexed_bytes += metadata.len();
        }
    }

    documents.flush()?;
    chunks.flush()?;
    Ok(report)
}

pub fn search_rag_index(index_dir: &Path, query: &str, limit: usize) -> Result<Vec<RagSearchHit>> {
    let chunks_path = index_dir.join("chunks.jsonl");
    let file = File::open(&chunks_path)
        .with_context(|| format!("failed to open {}", chunks_path.display()))?;
    let reader = BufReader::new(file);
    let query_lower = query.to_ascii_lowercase();
    let terms = query_lower
        .split_whitespace()
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>();
    let mut hits = Vec::new();

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let chunk: RagChunkRecord = serde_json::from_str(&line)?;
        let text_lower = chunk.text.to_ascii_lowercase();
        if terms.len() > 1 && terms.iter().any(|term| !text_lower.contains(term)) {
            continue;
        }
        let term_score = terms
            .iter()
            .map(|term| text_lower.matches(term).count())
            .sum::<usize>();
        let phrase_score = if terms.len() > 1 {
            text_lower.matches(&query_lower).count() * terms.len() * 8
        } else {
            0
        };
        let score = term_score + phrase_score;
        if score == 0 {
            continue;
        }

        hits.push(RagSearchHit {
            document_path: format!("{}:{}", chunk.source_kind, chunk.relative_path),
            chunk_id: chunk.id,
            score,
            preview: make_preview(&chunk.text, &terms, 240),
        });
    }

    hits.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.document_path.cmp(&right.document_path))
    });
    hits.truncate(limit);
    Ok(hits)
}

pub fn build_project_context(root: &Path) -> Result<ProjectContext> {
    build_project_context_with_limits(
        root,
        DEFAULT_CONTEXT_FILES,
        DEFAULT_CONTEXT_BYTES_PER_FILE,
        DEFAULT_CONTEXT_TOTAL_BYTES,
    )
}

pub fn build_project_context_with_limits(
    root: &Path,
    max_files: usize,
    max_bytes_per_file: usize,
    max_total_bytes: usize,
) -> Result<ProjectContext> {
    let report = inspect_workspace(root)?;
    let candidates = select_context_files(&report);
    let mut snippets = Vec::new();
    let mut total_chars = 0usize;

    for relative_path in candidates.into_iter().take(max_files) {
        if total_chars >= max_total_bytes {
            break;
        }

        let absolute_path = report.root.join(&relative_path);
        if !is_probably_text(&absolute_path) {
            continue;
        }

        let metadata = match fs::metadata(&absolute_path) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if metadata.len() > MAX_READ_BYTES {
            continue;
        }

        let raw = match fs::read_to_string(&absolute_path) {
            Ok(value) => value,
            Err(_) => continue,
        };

        let remaining = max_total_bytes.saturating_sub(total_chars);
        let limit = max_bytes_per_file.min(remaining);
        if limit == 0 {
            break;
        }

        let (content, truncated) = truncate_chars(&raw, limit);
        total_chars += content.chars().count();
        snippets.push(FileSnippet {
            path: relative_path,
            content,
            truncated,
        });
    }

    Ok(ProjectContext {
        report,
        snippets,
        total_chars,
    })
}

pub fn build_java_project(root: &Path) -> Result<BuildResult> {
    build_java_project_with_options(root, BuildOptions::default())
}

pub fn build_java_project_with_options(root: &Path, options: BuildOptions) -> Result<BuildResult> {
    build_java_project_internal(root, options, None)
}

pub fn build_java_project_with_cancellation(
    root: &Path,
    options: BuildOptions,
    cancellation: &CancellationToken,
) -> Result<BuildResult> {
    build_java_project_internal(root, options, Some(cancellation))
}

fn build_java_project_internal(
    root: &Path,
    options: BuildOptions,
    cancellation: Option<&CancellationToken>,
) -> Result<BuildResult> {
    let analysis = analyze_java_project(root)?;
    let (program, args, command_display) = match analysis.build_tool {
        BuildTool::Maven => (
            "mvn",
            vec!["-q", "-DskipTests", "package"],
            "mvn -q -DskipTests package".to_string(),
        ),
        BuildTool::Gradle => ("gradle", vec!["build"], "gradle build".to_string()),
        BuildTool::Unknown => anyhow::bail!("No Maven or Gradle build file detected."),
    };

    let before_git = capture_git_state(&analysis.root);
    let mut request = ProcessRequest::new(program, &analysis.root);
    request.args = args.iter().map(OsString::from).collect();
    request.timeout = options.timeout;
    request.output_limit_bytes = options.output_limit_bytes;
    request.launch_mode = ProcessLaunchMode::WindowsCommandScript;
    let process = run_process_with_cancellation(&request, cancellation)
        .with_context(|| format!("failed to run build command: {command_display}"))?;

    let mut summary = match process.status {
        ProcessStatus::Success => summarize_build_output(&process.stdout, &process.stderr, true),
        ProcessStatus::Failed => summarize_build_output(&process.stdout, &process.stderr, false),
        ProcessStatus::TimedOut => vec![format!(
            "Build timed out after {:.2}s.",
            process.duration.as_secs_f64()
        )],
        ProcessStatus::Cancelled if process.process_id.is_none() => {
            vec!["Build was cancelled before its process started.".to_string()]
        }
        ProcessStatus::Cancelled => vec!["Build was cancelled.".to_string()],
    };
    if process.output.output_truncated {
        summary.push(format!(
            "Process output exceeded {} bytes per stream; only bounded tails were retained.",
            process.output.limit_bytes_per_stream
        ));
    }
    summary.extend(process.output.capture_errors.iter().cloned());
    if process.termination.attempted {
        if process.termination.succeeded {
            summary.push(format!(
                "Process termination completed with strategy `{}`.",
                process.termination.strategy.as_str()
            ));
        } else {
            summary.push("Process termination did not complete cleanly.".to_string());
        }
    }

    let after_git = capture_git_state(&analysis.root);
    let git_report = BuildGitReport::from_capture_results(
        before_git,
        after_git,
        options.fail_on_worktree_change,
    );

    Ok(BuildResult {
        root: analysis.root,
        command: command_display,
        success: process.success(),
        exit_code: process.exit_code,
        duration: process.duration,
        summary,
        stdout_tail: tail_lines(&process.stdout, 30),
        stderr_tail: tail_lines(&process.stderr, 30),
        process_id: process.process_id,
        process_status: process.status,
        timed_out: process.timed_out(),
        cancelled: process.cancelled(),
        timeout: options.timeout,
        output: process.output,
        termination: process.termination,
        git_report,
    })
}

pub fn analyze_java_project(root: &Path) -> Result<JavaProjectAnalysis> {
    let root = fs::canonicalize(root)
        .with_context(|| format!("failed to resolve workspace path: {}", root.display()))?;
    let pom_path = root.join("pom.xml");
    let gradle_path = root.join("build.gradle");
    let gradle_kts_path = root.join("build.gradle.kts");
    let plugin_yml_path = root.join("src/main/resources/plugin.yml");
    let plugin_yaml_path = root.join("src/main/resources/plugin.yaml");

    let build_tool = if pom_path.exists() {
        BuildTool::Maven
    } else if gradle_path.exists() || gradle_kts_path.exists() {
        BuildTool::Gradle
    } else {
        BuildTool::Unknown
    };

    let maven = if pom_path.exists() {
        let content = fs::read_to_string(&pom_path)
            .with_context(|| format!("failed to read {}", pom_path.display()))?;
        Some(parse_pom(&content)?)
    } else {
        None
    };

    let plugin = if plugin_yml_path.exists() {
        let content = fs::read_to_string(&plugin_yml_path)
            .with_context(|| format!("failed to read {}", plugin_yml_path.display()))?;
        Some(parse_plugin_yml(&content)?)
    } else if plugin_yaml_path.exists() {
        let content = fs::read_to_string(&plugin_yaml_path)
            .with_context(|| format!("failed to read {}", plugin_yaml_path.display()))?;
        Some(parse_plugin_yml(&content)?)
    } else {
        None
    };

    let mut java_files = Vec::new();
    for entry in WalkDir::new(&root)
        .into_iter()
        .filter_entry(should_enter)
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file()
            || entry.path().extension().and_then(|v| v.to_str()) != Some("java")
        {
            continue;
        }

        let content = match fs::read_to_string(entry.path()) {
            Ok(value) => value,
            Err(_) => continue,
        };
        java_files.push(parse_java_file(&root, entry.path(), &content));
    }

    let mut risks = collect_project_risks(build_tool, maven.as_ref(), plugin.as_ref(), &java_files);
    risks.sort();
    risks.dedup();

    let build_command = match build_tool {
        BuildTool::Maven => Some("mvn -q -DskipTests package".to_string()),
        BuildTool::Gradle => Some("gradle build".to_string()),
        BuildTool::Unknown => None,
    };

    Ok(JavaProjectAnalysis {
        root,
        build_tool,
        maven,
        plugin,
        java_files,
        risks,
        build_command,
    })
}

pub fn propose_java_legacy_patch(root: &Path) -> Result<PatchProposal> {
    let analysis = analyze_java_project(root)?;
    let mut changes = Vec::new();
    let mut notes = Vec::new();

    for file in &analysis.java_files {
        let absolute_path = analysis.root.join(&file.path);
        let content = match fs::read_to_string(&absolute_path) {
            Ok(value) => value,
            Err(_) => continue,
        };

        let Some((proposed, replacements)) = apply_legacy_replacements(&content) else {
            continue;
        };
        let reason = replacements
            .iter()
            .map(|replacement| format!("{} -> {}", replacement.modern, replacement.legacy))
            .collect::<Vec<_>>()
            .join(", ");
        let diff = build_unified_diff(
            &file.path,
            &content,
            &proposed,
            "Bukkit 1.8.8 legacy symbol names.",
        );

        changes.push(PatchFileChange {
            path: file.path.clone(),
            reason: format!("Replace modern symbols with Bukkit 1.8.8 legacy names: {reason}."),
            diff,
            original_content: content.into_bytes(),
            proposed_content: proposed.into_bytes(),
        });
    }

    for plugin_path in [
        PathBuf::from("src/main/resources/plugin.yml"),
        PathBuf::from("src/main/resources/plugin.yaml"),
    ] {
        let absolute_path = analysis.root.join(&plugin_path);
        if !absolute_path.exists() {
            continue;
        }

        let content = match fs::read_to_string(&absolute_path) {
            Ok(value) => value,
            Err(_) => continue,
        };

        let Some(proposed) = remove_plugin_api_version(&content) else {
            continue;
        };
        let diff = build_unified_diff(
            &plugin_path,
            &content,
            &proposed,
            "Bukkit 1.8.8 plugin.yml compatibility.",
        );

        changes.push(PatchFileChange {
            path: plugin_path,
            reason: "Disable plugin.yml api-version with a YAML comment; Bukkit 1.8.8 does not expect it.".to_string(),
            diff,
            original_content: content.into_bytes(),
            proposed_content: proposed.into_bytes(),
        });
    }

    if changes.is_empty() {
        notes.push("No deterministic Java legacy patch is currently needed.".to_string());
    } else {
        notes.push("Patch is a preview only; no file was modified.".to_string());
        notes.push(
            "Run `build` after applying the patch manually or via a future safe-apply command."
                .to_string(),
        );
    }

    Ok(PatchProposal {
        root: analysis.root,
        changes,
        notes,
    })
}

pub fn check_patch_with_git(proposal: &PatchProposal) -> Result<Option<PatchCheckResult>> {
    if proposal.changes.is_empty() {
        return Ok(None);
    }

    let patch = proposal.combined_diff();
    let command_display =
        "git apply --check --ignore-space-change --ignore-whitespace -".to_string();
    let mut command = build_process_command(
        "git",
        &[
            "apply",
            "--check",
            "--ignore-space-change",
            "--ignore-whitespace",
            "-",
        ],
    );
    let mut child = command
        .current_dir(&proposal.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to run patch check command: {command_display}"))?;

    if let Some(stdin) = &mut child.stdin {
        stdin
            .write_all(patch.as_bytes())
            .context("failed to send patch to git apply --check")?;
    }

    let output = child
        .wait_with_output()
        .context("failed to read patch check result")?;

    Ok(Some(PatchCheckResult {
        command: command_display,
        success: output.status.success(),
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    }))
}

pub fn apply_patch_with_git(proposal: &PatchProposal) -> Result<Option<PatchCheckResult>> {
    if proposal.changes.is_empty() {
        return Ok(None);
    }

    let line_endings = collect_line_ending_styles(
        &proposal.root,
        proposal.changes.iter().map(|change| change.path.as_path()),
    )?;
    let patch = proposal.combined_diff();
    let command_display = "git apply --ignore-space-change --ignore-whitespace -".to_string();
    let mut command = build_process_command(
        "git",
        &["apply", "--ignore-space-change", "--ignore-whitespace", "-"],
    );
    let mut child = command
        .current_dir(&proposal.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to run patch apply command: {command_display}"))?;

    if let Some(stdin) = &mut child.stdin {
        stdin
            .write_all(patch.as_bytes())
            .context("failed to send patch to git apply")?;
    }

    let output = child
        .wait_with_output()
        .context("failed to read patch apply result")?;
    if output.status.success() {
        restore_line_endings(&proposal.root, &line_endings)?;
    }

    Ok(Some(PatchCheckResult {
        command: command_display,
        success: output.status.success(),
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    }))
}

fn check_reverse_patch_file_with_git(root: &Path, patch_path: &Path) -> Result<PatchCheckResult> {
    run_git_apply_patch_file(root, patch_path, true, true)
}

fn apply_reverse_patch_file_with_git(root: &Path, patch_path: &Path) -> Result<PatchCheckResult> {
    run_git_apply_patch_file(root, patch_path, true, false)
}

fn run_git_apply_patch_file(
    root: &Path,
    patch_path: &Path,
    reverse: bool,
    check: bool,
) -> Result<PatchCheckResult> {
    let line_endings = if check {
        Vec::new()
    } else {
        let patch_paths = patch_file_paths(patch_path)?;
        collect_line_ending_styles(root, patch_paths.iter().map(|path| path.as_path()))?
    };
    let relative_patch = to_relative(root, patch_path);
    let mut args = vec!["apply"];
    if check {
        args.push("--check");
    }
    if reverse {
        args.push("-R");
    }
    args.push("--ignore-space-change");
    args.push("--ignore-whitespace");

    let relative_patch_arg = relative_patch.to_string_lossy().to_string();
    args.push(&relative_patch_arg);

    let command_display = format!("git {}", args.join(" "));
    let output = build_process_command("git", &args)
        .current_dir(root)
        .output()
        .with_context(|| format!("failed to run patch file command: {command_display}"))?;
    if output.status.success() && !check {
        restore_line_endings(root, &line_endings)?;
    }

    Ok(PatchCheckResult {
        command: command_display,
        success: output.status.success(),
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

pub fn prepare_java_legacy_apply_plan(root: &Path, dry_run: bool) -> Result<ApplyPlan> {
    let proposal = propose_java_legacy_patch(root)?;
    let check = check_patch_with_git(&proposal)?;

    Ok(ApplyPlan {
        proposal,
        check,
        apply: None,
        copied_from: None,
        dry_run,
        log: None,
        transaction: None,
    })
}

pub fn apply_java_legacy_patch_to_copy(source: &Path, copy_to: &Path) -> Result<ApplyPlan> {
    let target_root = copy_project_to(source, copy_to)?;
    let mut plan = prepare_java_legacy_apply_plan(&target_root, false)?;
    plan.copied_from = Some(fs::canonicalize(source).with_context(|| {
        format!(
            "failed to resolve source project path before apply: {}",
            source.display()
        )
    })?);

    if plan.proposal.changes.is_empty() {
        return Ok(plan);
    }

    if let Some(check) = &plan.check {
        if !check.success {
            return Ok(plan);
        }
    }

    execute_transactional_apply(&mut plan, ApplyGitPolicy::Optional)?;
    Ok(plan)
}

pub fn apply_java_legacy_patch_in_place(root: &Path) -> Result<ApplyPlan> {
    apply_java_legacy_patch_in_place_with_options(root, false)
}

pub fn apply_java_legacy_patch_in_place_with_options(
    root: &Path,
    allow_dirty: bool,
) -> Result<ApplyPlan> {
    let mut plan = prepare_java_legacy_apply_plan(root, false)?;

    if plan.proposal.changes.is_empty() {
        return Ok(plan);
    }

    if let Some(check) = &plan.check {
        if !check.success {
            return Ok(plan);
        }
    }

    execute_transactional_apply(
        &mut plan,
        if allow_dirty {
            ApplyGitPolicy::AllowDirty
        } else {
            ApplyGitPolicy::RequireClean
        },
    )?;
    Ok(plan)
}

fn execute_transactional_apply(plan: &mut ApplyPlan, git_policy: ApplyGitPolicy) -> Result<()> {
    let patch = plan.proposal.combined_diff();
    let mutations = plan
        .proposal
        .changes
        .iter()
        .map(|change| {
            FileMutation::replace(
                change.path.clone(),
                change.original_content.clone(),
                change.proposed_content.clone(),
            )
        })
        .collect::<Vec<_>>();
    let mut request = ApplyTransactionRequest::new(&plan.proposal.root, patch, mutations)
        .with_git_policy(git_policy);
    if let Some(copied_from) = &plan.copied_from {
        request = request.with_copied_from(copied_from);
    }

    let mut transaction = execute_apply_transaction(request)?;
    plan.apply = Some(PatchCheckResult {
        command: "opticcode transactional file apply".to_string(),
        success: transaction.committed(),
        exit_code: Some(if transaction.committed() {
            0
        } else if transaction.rollback_failed() {
            3
        } else {
            2
        }),
        stdout: String::new(),
        stderr: transaction.errors.join("\n"),
    });

    if transaction.committed() {
        match write_apply_log(plan, &transaction) {
            Ok(log) => plan.log = Some(log),
            Err(error) => transaction.warnings.push(format!(
                "authoritative transaction committed, but compatibility apply-log index failed: {error:#}"
            )),
        }
    }
    plan.transaction = Some(transaction);
    Ok(())
}

pub fn undo_apply_run(root: &Path, run_id: &str) -> Result<ApplyUndoResult> {
    validate_transaction_id(run_id)?;

    let root = fs::canonicalize(root)
        .with_context(|| format!("failed to resolve project path: {}", root.display()))?;
    let patch_path = root
        .join(".opticcode")
        .join("runs")
        .join(run_id)
        .join("patch.diff");
    if !patch_path.is_file() {
        anyhow::bail!(
            "apply run patch not found: {}",
            to_relative(&root, &patch_path).display()
        );
    }

    let manifest_path = patch_path
        .parent()
        .expect("transaction patch should have a parent")
        .join("manifest.json");
    if manifest_path.is_file() {
        let transaction = rollback_apply_transaction(&root, run_id)?;
        let success = transaction.rolled_back();
        return Ok(ApplyUndoResult {
            root,
            run_id: run_id.to_string(),
            patch_path,
            check: PatchCheckResult {
                command: "opticcode transaction backup verification".to_string(),
                success,
                exit_code: Some(if success { 0 } else { 1 }),
                stdout: String::new(),
                stderr: transaction.errors.join("\n"),
            },
            undo: Some(PatchCheckResult {
                command: "opticcode transactional rollback".to_string(),
                success,
                exit_code: Some(if success { 0 } else { 3 }),
                stdout: String::new(),
                stderr: transaction.errors.join("\n"),
            }),
            transaction: Some(transaction),
        });
    }

    let check = check_reverse_patch_file_with_git(&root, &patch_path)?;
    if !check.success {
        return Ok(ApplyUndoResult {
            root,
            run_id: run_id.to_string(),
            patch_path,
            check,
            undo: None,
            transaction: None,
        });
    }

    let undo = apply_reverse_patch_file_with_git(&root, &patch_path)?;
    Ok(ApplyUndoResult {
        root,
        run_id: run_id.to_string(),
        patch_path,
        check,
        undo: Some(undo),
        transaction: None,
    })
}

fn write_apply_log(
    plan: &ApplyPlan,
    transaction: &ApplyTransactionResult,
) -> Result<ApplyLogEntry> {
    let applied_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time should be after unix epoch")?
        .as_millis();
    let run_id = transaction.transaction_id.clone();
    let opticcode_dir = plan.proposal.root.join(".opticcode");
    let run_dir = opticcode_dir.join("runs").join(&run_id);
    let patch_path = run_dir.join("patch.diff");
    if !patch_path.is_file() {
        anyhow::bail!(
            "authoritative transaction patch is missing: {}",
            patch_path.display()
        );
    }

    let patch_relative = to_relative(&plan.proposal.root, &patch_path);
    let patch_relative_display = patch_relative.display().to_string();
    let rollback_command = format!("git apply -R \"{}\"", patch_relative_display);
    let entry = ApplyLogEntry {
        schema_version: APPLY_TRANSACTION_SCHEMA_VERSION,
        run_id,
        applied_at_unix_ms,
        project_root: plan.proposal.root.display().to_string(),
        copied_from: plan
            .copied_from
            .as_ref()
            .map(|path| path.display().to_string()),
        change_count: plan.proposal.changes.len(),
        files: plan
            .proposal
            .changes
            .iter()
            .map(|change| change.path.display().to_string())
            .collect(),
        patch_path: patch_relative_display,
        rollback_command,
        transaction_state: Some(transaction.final_state),
    };

    let serialized = serde_json::to_vec(&entry)?;
    append_apply_log_index(&plan.proposal.root, &serialized)?;

    Ok(entry)
}

fn apply_log_schema_version() -> u32 {
    APPLY_TRANSACTION_SCHEMA_VERSION
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineEndingStyle {
    Lf,
    Crlf,
}

fn collect_line_ending_styles<'a>(
    root: &Path,
    paths: impl Iterator<Item = &'a Path>,
) -> Result<Vec<(PathBuf, Option<LineEndingStyle>)>> {
    let mut styles = Vec::new();
    for relative_path in paths {
        let absolute_path = root.join(relative_path);
        let bytes = fs::read(&absolute_path).with_context(|| {
            format!(
                "failed to read line ending style: {}",
                absolute_path.display()
            )
        })?;
        styles.push((
            relative_path.to_path_buf(),
            detect_line_ending_style(&bytes),
        ));
    }

    Ok(styles)
}

fn restore_line_endings(root: &Path, styles: &[(PathBuf, Option<LineEndingStyle>)]) -> Result<()> {
    for (relative_path, style) in styles {
        let Some(style) = style else {
            continue;
        };
        let absolute_path = root.join(relative_path);
        let bytes = fs::read(&absolute_path).with_context(|| {
            format!(
                "failed to read file for line ending restore: {}",
                absolute_path.display()
            )
        })?;
        let normalized = normalize_line_endings(&bytes, *style);
        if normalized != bytes {
            fs::write(&absolute_path, normalized).with_context(|| {
                format!(
                    "failed to restore line endings: {}",
                    absolute_path.display()
                )
            })?;
        }
    }

    Ok(())
}

fn detect_line_ending_style(bytes: &[u8]) -> Option<LineEndingStyle> {
    let mut crlf = 0usize;
    let mut lf = 0usize;
    let mut index = 0usize;

    while index < bytes.len() {
        match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => {
                crlf += 1;
                index += 2;
            }
            b'\n' => {
                lf += 1;
                index += 1;
            }
            _ => index += 1,
        }
    }

    match (crlf, lf) {
        (0, 0) => None,
        (crlf, lf) if crlf >= lf => Some(LineEndingStyle::Crlf),
        _ => Some(LineEndingStyle::Lf),
    }
}

fn normalize_line_endings(bytes: &[u8], style: LineEndingStyle) -> Vec<u8> {
    let mut lf_normalized = Vec::with_capacity(bytes.len());
    let mut index = 0usize;

    while index < bytes.len() {
        if bytes[index] == b'\r' {
            if bytes.get(index + 1) == Some(&b'\n') {
                index += 2;
            } else {
                index += 1;
            }
            lf_normalized.push(b'\n');
        } else {
            lf_normalized.push(bytes[index]);
            index += 1;
        }
    }

    if style == LineEndingStyle::Lf {
        return lf_normalized;
    }

    let mut crlf_normalized = Vec::with_capacity(lf_normalized.len());
    for byte in lf_normalized {
        if byte == b'\n' {
            crlf_normalized.extend_from_slice(b"\r\n");
        } else {
            crlf_normalized.push(byte);
        }
    }

    crlf_normalized
}

fn patch_file_paths(patch_path: &Path) -> Result<Vec<PathBuf>> {
    let content = fs::read_to_string(patch_path)
        .with_context(|| format!("failed to read patch file: {}", patch_path.display()))?;
    let mut paths = Vec::new();

    for line in content.lines() {
        let Some(path) = line.strip_prefix("+++ b/") else {
            continue;
        };
        if path == "/dev/null" {
            continue;
        }
        paths.push(PathBuf::from(path));
    }

    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn copy_project_to(source: &Path, copy_to: &Path) -> Result<PathBuf> {
    let source = fs::canonicalize(source).with_context(|| {
        format!(
            "failed to resolve source project path: {}",
            source.display()
        )
    })?;

    if !source.is_dir() {
        anyhow::bail!("source project is not a directory: {}", source.display());
    }

    let target = absolute_path(copy_to)?;
    if target.exists() {
        anyhow::bail!("copy target already exists: {}", target.display());
    }
    if target.starts_with(&source) {
        anyhow::bail!(
            "copy target must not be inside source project: {}",
            target.display()
        );
    }

    fs::create_dir_all(&target)
        .with_context(|| format!("failed to create copy target: {}", target.display()))?;

    for entry in WalkDir::new(&source).follow_links(false) {
        let entry =
            entry.with_context(|| format!("failed to walk source: {}", source.display()))?;
        let path = entry.path();
        if path == source {
            continue;
        }

        let relative = path
            .strip_prefix(&source)
            .with_context(|| format!("failed to compute relative path for {}", path.display()))?;
        let destination = target.join(relative);

        if entry.file_type().is_dir() {
            fs::create_dir_all(&destination).with_context(|| {
                format!(
                    "failed to create copied directory: {}",
                    destination.display()
                )
            })?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("failed to create copied parent: {}", parent.display())
                })?;
            }
            fs::copy(path, &destination).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    path.display(),
                    destination.display()
                )
            })?;
        }
    }

    Ok(target)
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()
            .context("failed to read current directory")?
            .join(path))
    }
}

fn build_process_command(program: &str, args: &[&str]) -> Command {
    if cfg!(windows) {
        let mut command = Command::new("cmd");
        command.arg("/C").arg(program).args(args);
        command
    } else {
        let mut command = Command::new(program);
        command.args(args);
        command
    }
}

impl BuildResult {
    pub fn command_succeeded(&self) -> bool {
        self.success && !self.git_report.strict_violation()
    }

    pub fn to_display_string(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("Project: {}\n", self.root.display()));
        out.push_str(&format!("Command: {}\n", self.command));
        out.push_str(&format!("Status: {}\n", self.process_status.as_str()));
        out.push_str(&format!(
            "Process ID: {}\n",
            self.process_id
                .map_or_else(|| "not started".to_string(), |id| id.to_string())
        ));
        out.push_str(&format!(
            "Exit code: {}\n",
            self.exit_code
                .map_or_else(|| "unknown".to_string(), |code| code.to_string())
        ));
        out.push_str(&format!("Duration: {:.2}s\n", self.duration.as_secs_f64()));
        out.push_str(&format!("Timeout: {:.2}s\n", self.timeout.as_secs_f64()));
        out.push_str(&format!(
            "Output: stdout={} B, stderr={} B, limit={} B/stream, truncated={}\n",
            self.output.stdout_bytes,
            self.output.stderr_bytes,
            self.output.limit_bytes_per_stream,
            self.output.output_truncated
        ));
        if self.termination.attempted {
            out.push_str(&format!(
                "Termination: strategy={}, succeeded={}\n",
                self.termination.strategy.as_str(),
                self.termination.succeeded
            ));
            if let Some(error) = &self.termination.error {
                out.push_str(&format!("Termination error: {error}\n"));
            }
        }
        out.push_str(&format!(
            "Overall status: {}\n",
            if self.command_succeeded() {
                "OK"
            } else {
                "FAILED"
            }
        ));

        out.push_str("\nSummary:\n");
        if self.summary.is_empty() {
            out.push_str("- no notable errors detected\n");
        } else {
            for item in &self.summary {
                out.push_str(&format!("- {item}\n"));
            }
        }

        if !self.stdout_tail.trim().is_empty() {
            out.push_str("\nStdout tail:\n");
            out.push_str(&self.stdout_tail);
            if !self.stdout_tail.ends_with('\n') {
                out.push('\n');
            }
        }

        if !self.stderr_tail.trim().is_empty() {
            out.push_str("\nStderr tail:\n");
            out.push_str(&self.stderr_tail);
            if !self.stderr_tail.ends_with('\n') {
                out.push('\n');
            }
        }

        out.push('\n');
        out.push_str(&self.git_report.to_display_string());

        out
    }
}

impl PatchProposal {
    pub fn to_display_string(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("Project: {}\n", self.root.display()));
        out.push_str("Mode: preview only\n");
        out.push_str(&format!("Changes: {}\n", self.changes.len()));

        out.push_str("\nNotes:\n");
        for note in &self.notes {
            out.push_str(&format!("- {note}\n"));
        }

        for change in &self.changes {
            out.push_str(&format!("\n## {}\n", change.path.display()));
            out.push_str(&format!("Reason: {}\n\n", change.reason));
            out.push_str(&change.diff);
            if !change.diff.ends_with('\n') {
                out.push('\n');
            }
        }

        out
    }

    pub fn combined_diff(&self) -> String {
        let mut out = String::new();
        for change in &self.changes {
            out.push_str(&change.diff);
            if !change.diff.ends_with('\n') {
                out.push('\n');
            }
        }
        out
    }
}

impl PatchCheckResult {
    pub fn to_display_string(&self) -> String {
        let mut out = String::new();
        out.push_str("\nPatch check:\n");
        out.push_str(&format!("Command: {}\n", self.command));
        out.push_str(&format!(
            "Status: {}\n",
            if self.success { "OK" } else { "FAILED" }
        ));
        out.push_str(&format!(
            "Exit code: {}\n",
            self.exit_code
                .map_or_else(|| "unknown".to_string(), |code| code.to_string())
        ));

        if !self.stdout.trim().is_empty() {
            out.push_str("\nStdout:\n");
            out.push_str(&self.stdout);
            if !self.stdout.ends_with('\n') {
                out.push('\n');
            }
        }

        if !self.stderr.trim().is_empty() {
            out.push_str("\nStderr:\n");
            out.push_str(&self.stderr);
            if !self.stderr.ends_with('\n') {
                out.push('\n');
            }
        }

        out
    }
}

impl ApplyPlan {
    pub fn success(&self) -> bool {
        if self.dry_run {
            return self.proposal.changes.is_empty()
                || self.check.as_ref().is_some_and(|check| check.success);
        }

        self.proposal.changes.is_empty()
            || self
                .transaction
                .as_ref()
                .is_some_and(ApplyTransactionResult::committed)
    }

    pub fn to_display_string(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("Project: {}\n", self.proposal.root.display()));
        if let Some(source) = &self.copied_from {
            out.push_str(&format!("Copied from: {}\n", source.display()));
        }
        out.push_str(&format!(
            "Mode: {}\n",
            if self.dry_run {
                "apply dry-run"
            } else if self.copied_from.is_some() {
                "apply copy"
            } else {
                "apply"
            }
        ));
        out.push_str(&format!("Changes: {}\n", self.proposal.changes.len()));

        if self.proposal.changes.is_empty() {
            out.push_str("\nNo deterministic Java legacy patch is currently needed.\n");
            return out;
        }

        out.push_str("\nFiles:\n");
        for change in &self.proposal.changes {
            out.push_str(&format!("- {}\n", change.path.display()));
            out.push_str(&format!("  reason: {}\n", change.reason));
        }

        match &self.check {
            Some(check) => {
                out.push_str("\nPatch check:\n");
                out.push_str(&format!("Command: {}\n", check.command));
                out.push_str(&format!(
                    "Status: {}\n",
                    if check.success { "OK" } else { "FAILED" }
                ));
                out.push_str(&format!(
                    "Exit code: {}\n",
                    check
                        .exit_code
                        .map_or_else(|| "unknown".to_string(), |code| code.to_string())
                ));
                if !check.stderr.trim().is_empty() {
                    out.push_str("\nStderr:\n");
                    out.push_str(&check.stderr);
                    if !check.stderr.ends_with('\n') {
                        out.push('\n');
                    }
                }
            }
            None => out.push_str("\nPatch check: skipped, no changes.\n"),
        }

        if let Some(apply) = &self.apply {
            out.push_str("\nPatch apply:\n");
            out.push_str(&format!("Command: {}\n", apply.command));
            out.push_str(&format!(
                "Status: {}\n",
                if apply.success { "OK" } else { "FAILED" }
            ));
            out.push_str(&format!(
                "Exit code: {}\n",
                apply
                    .exit_code
                    .map_or_else(|| "unknown".to_string(), |code| code.to_string())
            ));
            if !apply.stderr.trim().is_empty() {
                out.push_str("\nStderr:\n");
                out.push_str(&apply.stderr);
                if !apply.stderr.ends_with('\n') {
                    out.push('\n');
                }
            }
        }

        if self.dry_run {
            out.push_str("\nDry run: no file was modified.\n");
        } else if self.copied_from.is_some() && self.success() {
            out.push_str("\nApplied in copy only; source project was not modified.\n");
        } else if self.success() {
            out.push_str("\nApplied in source project.\n");
        }

        if let Some(transaction) = &self.transaction {
            out.push_str("\nTransaction:\n");
            out.push_str(&format!("Schema: {}\n", transaction.schema_version));
            out.push_str(&format!("Id: {}\n", transaction.transaction_id));
            out.push_str(&format!(
                "Final state: {}\n",
                transaction.final_state.as_str()
            ));
            out.push_str(&format!(
                "Rollback: attempted={}, success={}\n",
                transaction.rollback_attempted,
                transaction
                    .rollback_success
                    .map_or_else(|| "not_needed".to_string(), |success| success.to_string())
            ));
            if !transaction.errors.is_empty() {
                out.push_str("Errors:\n");
                for error in &transaction.errors {
                    out.push_str(&format!("- {error}\n"));
                }
            }
            if !transaction.warnings.is_empty() {
                out.push_str("Warnings:\n");
                for warning in &transaction.warnings {
                    out.push_str(&format!("- {warning}\n"));
                }
            }
        }

        if let Some(log) = &self.log {
            out.push_str("\nApply log:\n");
            out.push_str(&format!("Run id: {}\n", log.run_id));
            out.push_str(&format!("Patch: {}\n", log.patch_path));
            out.push_str(&format!("Rollback: {}\n", log.rollback_command));
        }

        out
    }
}

impl ApplyUndoResult {
    pub fn success(&self) -> bool {
        self.transaction
            .as_ref()
            .is_some_and(ApplyTransactionResult::rolled_back)
            || self.undo.as_ref().is_some_and(|undo| undo.success)
    }

    pub fn to_display_string(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("Project: {}\n", self.root.display()));
        out.push_str("Mode: apply undo\n");
        out.push_str(&format!("Run id: {}\n", self.run_id));
        out.push_str(&format!(
            "Patch: {}\n",
            to_relative(&self.root, &self.patch_path).display()
        ));

        out.push_str("\nRollback check:\n");
        out.push_str(&format!("Command: {}\n", self.check.command));
        out.push_str(&format!(
            "Status: {}\n",
            if self.check.success { "OK" } else { "FAILED" }
        ));
        out.push_str(&format!(
            "Exit code: {}\n",
            self.check
                .exit_code
                .map_or_else(|| "unknown".to_string(), |code| code.to_string())
        ));
        if !self.check.stderr.trim().is_empty() {
            out.push_str("\nStderr:\n");
            out.push_str(&self.check.stderr);
            if !self.check.stderr.ends_with('\n') {
                out.push('\n');
            }
        }

        if let Some(undo) = &self.undo {
            out.push_str("\nRollback apply:\n");
            out.push_str(&format!("Command: {}\n", undo.command));
            out.push_str(&format!(
                "Status: {}\n",
                if undo.success { "OK" } else { "FAILED" }
            ));
            out.push_str(&format!(
                "Exit code: {}\n",
                undo.exit_code
                    .map_or_else(|| "unknown".to_string(), |code| code.to_string())
            ));
            if !undo.stderr.trim().is_empty() {
                out.push_str("\nStderr:\n");
                out.push_str(&undo.stderr);
                if !undo.stderr.ends_with('\n') {
                    out.push('\n');
                }
            }
        }

        if let Some(transaction) = &self.transaction {
            out.push_str("\nTransaction rollback:\n");
            out.push_str(&format!(
                "Final state: {}\n",
                transaction.final_state.as_str()
            ));
            out.push_str(&format!(
                "Restored files: {}\n",
                transaction.restored_files.len()
            ));
            out.push_str(&format!(
                "Git restored: {}\n",
                transaction
                    .git_restored
                    .map_or_else(|| "not_available".to_string(), |value| value.to_string())
            ));
        }

        if self.success() {
            out.push_str("\nUndo applied.\n");
        } else {
            out.push_str("\nUndo was not applied.\n");
        }

        out
    }
}

impl ResourcePackReport {
    pub fn to_display_string(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("Resource pack: {}\n", self.root.display()));
        out.push_str(&format!("pack.mcmeta: {}\n", yes_no(self.has_pack_mcmeta)));
        out.push_str(&format!("Files: {}\n", self.total_files));

        out.push_str("\nCategories:\n");
        for (category, count) in top_named_counts(&self.categories, 16) {
            out.push_str(&format!("- {}: {}\n", category, count));
        }

        out.push_str("\nLegacy/resource matches:\n");
        if self.legacy_matches.is_empty() {
            out.push_str("- none\n");
        } else {
            for path in &self.legacy_matches {
                out.push_str(&format!("- {}\n", path.display()));
            }
        }

        out
    }
}

impl RagSourceReport {
    pub fn to_display_string(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("RAG source: {}\n", self.root.display()));
        out.push_str(&format!("Files: {}\n", self.total_files));
        out.push_str(&format!("Indexable text files: {}\n", self.indexable_files));
        out.push_str(&format!("Indexable text bytes: {}\n", self.indexable_bytes));
        out.push_str(&format!(
            "Skipped large text files: {}\n",
            self.skipped_large_files
        ));

        out.push_str("\nTop extensions:\n");
        for (extension, count) in top_named_counts(&self.extensions, 14) {
            out.push_str(&format!("- {}: {}\n", extension, count));
        }

        out.push_str("\nImportant files:\n");
        if self.important_files.is_empty() {
            out.push_str("- none\n");
        } else {
            for path in &self.important_files {
                out.push_str(&format!("- {}\n", path.display()));
            }
        }

        out
    }
}

impl RagIndexReport {
    pub fn to_display_string(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("Index: {}\n", self.output_dir.display()));
        out.push_str(&format!("Sources: {}\n", self.sources));
        out.push_str(&format!("Documents: {}\n", self.documents));
        out.push_str(&format!("Chunks: {}\n", self.chunks));
        out.push_str(&format!("Indexed bytes: {}\n", self.indexed_bytes));
        out.push_str("\nFiles:\n");
        out.push_str("- documents.jsonl\n");
        out.push_str("- chunks.jsonl\n");
        out
    }
}

impl RagSearchHit {
    pub fn to_display_string(&self) -> String {
        format!(
            "{}\nscore: {}\nchunk: {}\n{}\n",
            self.document_path, self.score, self.chunk_id, self.preview
        )
    }
}

fn parse_pom(content: &str) -> Result<MavenAnalysis> {
    let doc = roxmltree::Document::parse(content).context("failed to parse pom.xml")?;
    let root = doc.root_element();
    let properties = child(root, "properties");

    let source = properties
        .and_then(|node| child_text(node, "maven.compiler.source"))
        .or_else(|| properties.and_then(|node| child_text(node, "java.version")));
    let target = properties
        .and_then(|node| child_text(node, "maven.compiler.target"))
        .or_else(|| source.clone());

    let mut dependencies = Vec::new();
    if let Some(deps_node) = child(root, "dependencies") {
        for dep in deps_node
            .children()
            .filter(|node| node.is_element() && node.tag_name().name() == "dependency")
        {
            dependencies.push(MavenDependency {
                group_id: child_text(dep, "groupId"),
                artifact_id: child_text(dep, "artifactId"),
                version: child_text(dep, "version"),
                scope: child_text(dep, "scope"),
            });
        }
    }

    Ok(MavenAnalysis {
        group_id: child_text(root, "groupId"),
        artifact_id: child_text(root, "artifactId"),
        version: child_text(root, "version"),
        source,
        target,
        dependencies,
    })
}

fn parse_plugin_yml(content: &str) -> Result<PluginYmlAnalysis> {
    let value: Value = serde_yaml::from_str(content).context("failed to parse plugin.yml")?;

    Ok(PluginYmlAnalysis {
        name: yaml_string(&value, "name"),
        version: yaml_string(&value, "version"),
        main: yaml_string(&value, "main"),
        has_api_version: yaml_has_key(&value, "api-version"),
        commands: yaml_mapping_keys(&value, "commands"),
        permissions: yaml_mapping_keys(&value, "permissions"),
    })
}

fn parse_java_file(root: &Path, path: &Path, content: &str) -> JavaFileAnalysis {
    let imports = content
        .lines()
        .filter_map(|line| line.trim().strip_prefix("import "))
        .filter_map(|line| line.trim_end_matches(';').split_whitespace().next())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();

    let package_name = content
        .lines()
        .find_map(|line| line.trim().strip_prefix("package "))
        .map(|line| line.trim_end_matches(';').trim().to_string());

    let class_name = content.lines().find_map(extract_class_name);
    let is_command_executor = content.contains("CommandExecutor");
    let is_listener = content.contains("Listener") && content.contains("@EventHandler");
    let registered_commands = extract_string_calls(content, "getCommand");

    let mut risks = Vec::new();
    for replacement in detect_legacy_replacements(content) {
        risks.push(format!(
            "Uses {}; Bukkit 1.8.8 expects {}. {}",
            replacement.modern, replacement.legacy, replacement.reason
        ));
    }
    if content.contains("record ") {
        risks.push("Contains `record`, not compatible with Java 8.".to_string());
    }
    if content.contains(" var ") || content.contains("\nvar ") {
        risks.push("Contains `var`, not compatible with Java 8 local variable syntax.".to_string());
    }
    if imports
        .iter()
        .any(|item| item.starts_with("net.kyori.adventure"))
    {
        risks.push("Uses Adventure API imports, likely not Bukkit 1.8.8 native.".to_string());
    }
    if imports
        .iter()
        .any(|item| item.starts_with("org.bukkit.persistence"))
    {
        risks.push("Uses PersistentDataContainer API, not available in Bukkit 1.8.8.".to_string());
    }

    JavaFileAnalysis {
        path: to_relative(root, path),
        package_name,
        class_name,
        is_command_executor,
        is_listener,
        imports,
        registered_commands,
        risks,
    }
}

impl JavaProjectAnalysis {
    pub fn to_display_string(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("Project: {}\n", self.root.display()));
        out.push_str(&format!("Build tool: {}\n", self.build_tool));

        if let Some(maven) = &self.maven {
            out.push_str("\nMaven:\n");
            out.push_str(&format!("- groupId: {}\n", optional(&maven.group_id)));
            out.push_str(&format!("- artifactId: {}\n", optional(&maven.artifact_id)));
            out.push_str(&format!("- version: {}\n", optional(&maven.version)));
            out.push_str(&format!("- source: {}\n", optional(&maven.source)));
            out.push_str(&format!("- target: {}\n", optional(&maven.target)));
            out.push_str("- dependencies:\n");
            for dependency in &maven.dependencies {
                out.push_str(&format!(
                    "  - {}:{}:{} scope={}\n",
                    optional(&dependency.group_id),
                    optional(&dependency.artifact_id),
                    optional(&dependency.version),
                    optional(&dependency.scope)
                ));
            }
        }

        if let Some(plugin) = &self.plugin {
            out.push_str("\nplugin.yml:\n");
            out.push_str(&format!("- name: {}\n", optional(&plugin.name)));
            out.push_str(&format!("- version: {}\n", optional(&plugin.version)));
            out.push_str(&format!("- main: {}\n", optional(&plugin.main)));
            out.push_str(&format!(
                "- api-version present: {}\n",
                yes_no(plugin.has_api_version)
            ));
            out.push_str(&format!("- commands: {}\n", list_or_none(&plugin.commands)));
            out.push_str(&format!(
                "- permissions: {}\n",
                list_or_none(&plugin.permissions)
            ));
        } else {
            out.push_str("\nplugin.yml: not found\n");
        }

        let command_executors = self
            .java_files
            .iter()
            .filter(|file| file.is_command_executor)
            .collect::<Vec<_>>();
        let listeners = self
            .java_files
            .iter()
            .filter(|file| file.is_listener)
            .collect::<Vec<_>>();
        let registered_commands = self
            .java_files
            .iter()
            .flat_map(|file| {
                file.registered_commands
                    .iter()
                    .map(|command| format!("{} in {}", command, file.path.display()))
            })
            .collect::<Vec<_>>();

        out.push_str("\nJava:\n");
        out.push_str(&format!("- files: {}\n", self.java_files.len()));
        out.push_str("- command executors:\n");
        for file in command_executors {
            out.push_str(&format!("  - {}\n", file.path.display()));
        }
        out.push_str("- listeners:\n");
        for file in listeners {
            out.push_str(&format!("  - {}\n", file.path.display()));
        }
        out.push_str(&format!(
            "- registered commands: {}\n",
            list_or_none(&registered_commands)
        ));

        out.push_str("\nRisks:\n");
        if self.risks.is_empty() {
            out.push_str("- none detected\n");
        } else {
            for risk in &self.risks {
                out.push_str(&format!("- {risk}\n"));
            }
        }

        out.push_str("\nSuggested build command:\n");
        out.push_str(&format!("- {}\n", optional(&self.build_command)));

        out
    }
}

impl std::fmt::Display for BuildTool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildTool::Maven => write!(formatter, "Maven"),
            BuildTool::Gradle => write!(formatter, "Gradle"),
            BuildTool::Unknown => write!(formatter, "unknown"),
        }
    }
}

pub fn search_workspace(root: &Path, pattern: &str, limit: usize) -> Result<Vec<SearchHit>> {
    let root = fs::canonicalize(root)
        .with_context(|| format!("failed to resolve workspace path: {}", root.display()))?;
    let needle = pattern.to_ascii_lowercase();
    let mut hits = Vec::new();

    for entry in WalkDir::new(&root)
        .into_iter()
        .filter_entry(should_enter)
        .filter_map(Result::ok)
    {
        if hits.len() >= limit {
            break;
        }
        if !entry.file_type().is_file() || !is_probably_text(entry.path()) {
            continue;
        }

        let metadata = match entry.metadata() {
            Ok(value) => value,
            Err(_) => continue,
        };
        if metadata.len() > MAX_READ_BYTES {
            continue;
        }

        let content = match fs::read_to_string(entry.path()) {
            Ok(value) => value,
            Err(_) => continue,
        };

        for (index, line) in content.lines().enumerate() {
            if line.to_ascii_lowercase().contains(&needle) {
                hits.push(SearchHit {
                    path: to_relative(&root, entry.path()),
                    line_number: index + 1,
                    line: line.to_string(),
                });
                if hits.len() >= limit {
                    break;
                }
            }
        }
    }

    Ok(hits)
}

fn summarize_build_output(stdout: &str, stderr: &str, success: bool) -> Vec<String> {
    let combined = format!("{stdout}\n{stderr}");
    let mut summary = Vec::new();

    if success {
        summary.push("Build completed successfully.".to_string());
        return summary;
    }

    for line in combined
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let lower = line.to_ascii_lowercase();
        if lower.contains("compilation failure")
            || lower.contains("cannot find symbol")
            || lower.contains("package") && lower.contains("does not exist")
            || lower.contains("[error]")
            || lower.contains("failed to execute goal")
            || lower.contains("error:")
        {
            summary.push(line.to_string());
        }

        if summary.len() >= 12 {
            break;
        }
    }

    for replacement in LEGACY_SYMBOL_REPLACEMENTS {
        if combined.contains(replacement.modern)
            || symbol_name_appears(&combined, replacement.modern)
        {
            summary.push(format!(
                "Likely Bukkit 1.8.8 legacy fix: use {} instead of {}.",
                replacement.legacy, replacement.modern
            ));
        }
    }

    if summary.is_empty() {
        summary.push("Build failed, but no known error pattern was extracted.".to_string());
    }

    summary
}

fn tail_lines(value: &str, limit: usize) -> String {
    let lines = value.lines().collect::<Vec<_>>();
    let start = lines.len().saturating_sub(limit);
    lines[start..].join("\n")
}

fn detect_legacy_replacements(content: &str) -> Vec<LegacySymbolReplacement> {
    LEGACY_SYMBOL_REPLACEMENTS
        .iter()
        .copied()
        .filter(|replacement| content.contains(replacement.modern))
        .collect()
}

fn apply_legacy_replacements(content: &str) -> Option<(String, Vec<LegacySymbolReplacement>)> {
    let replacements = detect_legacy_replacements(content);
    if replacements.is_empty() {
        return None;
    }

    let mut proposed = content.to_string();
    for replacement in &replacements {
        proposed = proposed.replace(replacement.modern, replacement.legacy);
    }

    Some((proposed, replacements))
}

fn remove_plugin_api_version(content: &str) -> Option<String> {
    let mut proposed = String::new();
    let mut removed = false;

    for line in content.split_inclusive('\n') {
        let logical = line.trim_end_matches(['\r', '\n']);
        let is_top_level = logical
            .chars()
            .next()
            .is_some_and(|value| !value.is_whitespace());
        if is_top_level && logical.starts_with("api-version:") {
            removed = true;
            let newline = if line.ends_with("\r\n") {
                "\r\n"
            } else if line.ends_with('\n') {
                "\n"
            } else {
                ""
            };
            proposed.push_str("# api-version disabled for Bukkit 1.8.8 compatibility");
            proposed.push_str(newline);
            continue;
        }

        proposed.push_str(line);
    }

    removed.then_some(proposed)
}

fn symbol_name_appears(content: &str, qualified_symbol: &str) -> bool {
    qualified_symbol
        .rsplit_once('.')
        .is_some_and(|(_, name)| content.contains(name))
}

fn resource_pack_category(path: &str) -> String {
    if path.starts_with("assets/minecraft/blockstates/") {
        "blockstates".to_string()
    } else if path.starts_with("assets/minecraft/models/block/") {
        "models/block".to_string()
    } else if path.starts_with("assets/minecraft/models/item/") {
        "models/item".to_string()
    } else if path.starts_with("assets/minecraft/textures/blocks/") {
        "textures/blocks".to_string()
    } else if path.starts_with("assets/minecraft/textures/items/") {
        "textures/items".to_string()
    } else if path.starts_with("assets/minecraft/lang/") {
        "lang".to_string()
    } else if path.starts_with("assets/minecraft/mcpatcher/cit/") {
        "mcpatcher/cit".to_string()
    } else if path.starts_with("assets/") {
        "assets/other".to_string()
    } else {
        "root/other".to_string()
    }
}

fn is_legacy_resource_match(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [
        "spawner",
        "mob_spawner",
        "monster_placer",
        "nether_stalk",
        "nether_wart",
        "shovel",
        "spade",
        "spawn_egg",
    ]
    .iter()
    .any(|term| lower.contains(term))
}

fn is_rag_indexable_text(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return true;
    };

    matches!(
        extension.to_ascii_lowercase().as_str(),
        "java"
            | "kt"
            | "kts"
            | "groovy"
            | "xml"
            | "yml"
            | "yaml"
            | "properties"
            | "json"
            | "mcmeta"
            | "lang"
            | "md"
            | "txt"
            | "patch"
            | "gradle"
            | "toml"
            | "rs"
    )
}

fn is_important_rag_file(path: &Path) -> bool {
    let normalized = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
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
            | "README.md"
            | "readme.md"
    ) || normalized.starts_with("patches/")
        || normalized.contains("/patches/")
        || normalized.starts_with("src/main/resources/")
}

fn detect_rag_source_kind(root: &Path) -> String {
    let normalized = root
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
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

fn stable_hash_hex(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn chunk_text(content: &str, chunk_chars: usize) -> Vec<String> {
    let chunk_chars = chunk_chars.max(512);
    let mut chunks = Vec::new();
    let mut current = String::new();

    for character in content.chars() {
        current.push(character);
        if current.chars().count() >= chunk_chars {
            chunks.push(current);
            current = String::new();
        }
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    chunks
}

fn make_preview(text: &str, terms: &[&str], max_chars: usize) -> String {
    let lower = text.to_ascii_lowercase();
    let phrase = terms.join(" ");
    let first_match = lower
        .find(&phrase)
        .or_else(|| terms.iter().filter_map(|term| lower.find(term)).min())
        .unwrap_or(0);
    let start = first_match.saturating_sub(80);
    text.chars()
        .skip(start)
        .take(max_chars)
        .collect::<String>()
        .replace('\n', " ")
        .trim()
        .to_string()
}

fn build_unified_diff(path: &Path, original: &str, proposed: &str, reason: &str) -> String {
    let original_lines = original.lines().collect::<Vec<_>>();
    let proposed_lines = proposed.lines().collect::<Vec<_>>();
    let path = path.to_string_lossy().replace('\\', "/");

    let mut changed_indices = original_lines
        .iter()
        .zip(proposed_lines.iter())
        .enumerate()
        .filter_map(|(index, (left, right))| (left != right).then_some(index))
        .collect::<Vec<_>>();

    if original_lines.len() != proposed_lines.len() {
        let min_len = original_lines.len().min(proposed_lines.len());
        changed_indices.extend(min_len..original_lines.len().max(proposed_lines.len()));
    }

    let mut out = String::new();
    out.push_str(&format!("# {reason}\n"));
    out.push_str(&format!("--- a/{path}\n"));
    out.push_str(&format!("+++ b/{path}\n"));

    if changed_indices.is_empty() {
        return out;
    }

    let context = 3usize;
    let mut groups = Vec::new();
    let mut start = changed_indices[0];
    let mut end = changed_indices[0];

    for index in changed_indices.into_iter().skip(1) {
        if index <= end + context * 2 + 1 {
            end = index;
        } else {
            groups.push((start, end));
            start = index;
            end = index;
        }
    }
    groups.push((start, end));

    for (change_start, change_end) in groups {
        let hunk_start = change_start.saturating_sub(context);
        let hunk_end = (change_end + context + 1)
            .min(original_lines.len())
            .max((change_end + 1).min(proposed_lines.len()));
        let old_count = hunk_end.saturating_sub(hunk_start);
        let new_count = old_count;

        out.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            hunk_start + 1,
            old_count,
            hunk_start + 1,
            new_count
        ));

        for index in hunk_start..hunk_end {
            let original_line = original_lines.get(index);
            let proposed_line = proposed_lines.get(index);

            match (original_line, proposed_line) {
                (Some(left), Some(right)) if left == right => {
                    out.push_str(&format!(" {left}\n"));
                }
                (Some(left), Some(right)) => {
                    out.push_str(&format!("-{left}\n"));
                    out.push_str(&format!("+{right}\n"));
                }
                (Some(left), None) => out.push_str(&format!("-{left}\n")),
                (None, Some(right)) => out.push_str(&format!("+{right}\n")),
                (None, None) => {}
            }
        }
    }

    out
}

fn collect_project_risks(
    build_tool: BuildTool,
    maven: Option<&MavenAnalysis>,
    plugin: Option<&PluginYmlAnalysis>,
    java_files: &[JavaFileAnalysis],
) -> Vec<String> {
    let mut risks = Vec::new();

    if build_tool == BuildTool::Unknown {
        risks.push("No Maven or Gradle build file detected.".to_string());
    }

    if let Some(maven) = maven {
        if maven.source.as_deref() != Some("1.8") && maven.source.as_deref() != Some("8") {
            risks.push("Maven source level is not explicitly Java 8.".to_string());
        }
        if maven.target.as_deref() != Some("1.8") && maven.target.as_deref() != Some("8") {
            risks.push("Maven target level is not explicitly Java 8.".to_string());
        }

        let has_bukkit_dependency = maven.dependencies.iter().any(|dependency| {
            dependency
                .artifact_id
                .as_deref()
                .is_some_and(|artifact| artifact.contains("bukkit") || artifact.contains("spigot"))
        });
        if !has_bukkit_dependency {
            risks.push("No Bukkit/Spigot dependency detected in pom.xml.".to_string());
        }
    }

    if let Some(plugin) = plugin {
        if plugin.has_api_version {
            risks.push(
                "plugin.yml contains api-version; this is not expected for Bukkit 1.8.8."
                    .to_string(),
            );
        }
        if plugin.main.is_none() {
            risks.push("plugin.yml does not declare a main class.".to_string());
        }

        let registered_commands = java_files
            .iter()
            .flat_map(|file| file.registered_commands.iter())
            .cloned()
            .collect::<Vec<_>>();

        for command in &plugin.commands {
            if !registered_commands.iter().any(|value| value == command) {
                risks.push(format!(
                    "plugin.yml declares command `{command}`, but no getCommand(\"{command}\") call was detected."
                ));
            }
        }

        for command in &registered_commands {
            if !plugin.commands.iter().any(|value| value == command) {
                risks.push(format!(
                    "Java registers command `{command}`, but plugin.yml does not declare it."
                ));
            }
        }
    } else {
        risks.push("plugin.yml not found under src/main/resources.".to_string());
    }

    for file in java_files {
        for risk in &file.risks {
            risks.push(format!("{}: {}", file.path.display(), risk));
        }
    }

    risks
}

fn child<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    name: &str,
) -> Option<roxmltree::Node<'a, 'input>> {
    node.children()
        .find(|child| child.is_element() && child.tag_name().name() == name)
}

fn child_text(node: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    child(node, name)
        .and_then(|node| node.text())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn yaml_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(Value::String(key.to_string()))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn yaml_has_key(value: &Value, key: &str) -> bool {
    value.get(Value::String(key.to_string())).is_some()
}

fn yaml_mapping_keys(value: &Value, key: &str) -> Vec<String> {
    value
        .get(Value::String(key.to_string()))
        .and_then(Value::as_mapping)
        .map(|mapping| {
            mapping
                .keys()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn extract_class_name(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let marker = if trimmed.contains(" class ") {
        " class "
    } else if trimmed.starts_with("class ") {
        "class "
    } else {
        return None;
    };
    let after = trimmed.split_once(marker)?.1;
    after
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .next()
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn extract_string_calls(content: &str, function_name: &str) -> Vec<String> {
    let marker = format!("{function_name}(\"");
    let mut values = Vec::new();
    let mut rest = content;

    while let Some(start) = rest.find(&marker) {
        let after_marker = &rest[start + marker.len()..];
        let Some(end) = after_marker.find('"') else {
            break;
        };
        let value = &after_marker[..end];
        if !value.is_empty() {
            values.push(value.to_string());
        }
        rest = &after_marker[end + 1..];
    }

    values.sort();
    values.dedup();
    values
}

fn optional(value: &Option<String>) -> &str {
    value.as_deref().unwrap_or("unknown")
}

fn list_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_string()
    } else {
        values.join(", ")
    }
}

impl ProjectContext {
    pub fn to_display_string(&self) -> String {
        let mut out = self.report.to_display_string();
        out.push_str("\nContext snippets:\n");
        out.push_str(&format!("Total snippet chars: {}\n", self.total_chars));

        for snippet in &self.snippets {
            out.push_str(&format!("\n--- {} ---\n", snippet.path.display()));
            out.push_str(&snippet.content);
            if snippet.truncated {
                out.push_str("\n[truncated]\n");
            }
        }

        out
    }

    pub fn to_prompt_context(&self) -> String {
        let mut out = self.report.to_prompt_context();
        out.push_str("\n\nRelevant file snippets:\n");

        for snippet in &self.snippets {
            out.push_str(&format!("\n--- {} ---\n", snippet.path.display()));
            out.push_str(&snippet.content);
            if !snippet.content.ends_with('\n') {
                out.push('\n');
            }
            if snippet.truncated {
                out.push_str("[truncated]\n");
            }
        }

        out
    }
}

impl WorkspaceReport {
    pub fn to_display_string(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("Workspace: {}\n", self.root.display()));
        out.push_str(&format!("Files seen: {}\n", self.total_files_seen));
        out.push_str(&format!("Git: {}\n", yes_no(self.has_git)));
        out.push_str(&format!("Maven: {}\n", yes_no(self.has_maven)));
        out.push_str(&format!("Gradle: {}\n", yes_no(self.has_gradle)));
        out.push_str("\nTop extensions:\n");

        for (extension, count) in top_extensions(&self.extensions, 12) {
            out.push_str(&format!("- {}: {}\n", extension, count));
        }

        out.push_str("\nSample files:\n");
        for path in self.sampled_files.iter().take(40) {
            out.push_str(&format!("- {}\n", path.display()));
        }

        out
    }

    pub fn to_prompt_context(&self) -> String {
        let extensions = top_extensions(&self.extensions, 10)
            .into_iter()
            .map(|(extension, count)| format!("{extension}:{count}"))
            .collect::<Vec<_>>()
            .join(", ");

        let sample = self
            .sampled_files
            .iter()
            .take(30)
            .map(|path| format!("- {}", path.display()))
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "Root: {}\nGit: {}\nMaven: {}\nGradle: {}\nFiles seen: {}\nExtensions: {}\nSample files:\n{}",
            self.root.display(),
            yes_no(self.has_git),
            yes_no(self.has_maven),
            yes_no(self.has_gradle),
            self.total_files_seen,
            extensions,
            sample
        )
    }
}

fn select_context_files(report: &WorkspaceReport) -> Vec<PathBuf> {
    let mut files = report.sampled_files.clone();
    files.sort_by_key(|path| context_priority(path));
    files
}

fn context_priority(path: &Path) -> (u8, String) {
    let normalized = path.to_string_lossy().replace('\\', "/");
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();

    let priority = match file_name {
        "pom.xml" => 0,
        "build.gradle" | "build.gradle.kts" => 1,
        "plugin.yml" | "plugin.yaml" => 2,
        _ if normalized.ends_with("Plugin.java") || normalized.ends_with("Main.java") => 3,
        _ if normalized.contains("/command/") && normalized.ends_with(".java") => 4,
        _ if normalized.contains("/listener/") && normalized.ends_with(".java") => 5,
        _ if normalized.ends_with(".java") => 6,
        "README.md" => 7,
        _ => 9,
    };

    (priority, normalized)
}

fn truncate_chars(value: &str, max_chars: usize) -> (String, bool) {
    let mut iter = value.chars();
    let content = iter.by_ref().take(max_chars).collect::<String>();
    let truncated = iter.next().is_some();
    (content, truncated)
}

fn should_enter(entry: &DirEntry) -> bool {
    if entry.depth() == 0 {
        return true;
    }

    let name = entry.file_name().to_string_lossy();
    !matches!(
        name.as_ref(),
        ".git"
            | ".opticcode"
            | "target"
            | "build"
            | ".gradle"
            | ".idea"
            | ".vscode"
            | "node_modules"
            | "models"
            | "data"
    )
}

fn should_enter_resource_pack(entry: &DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    !matches!(
        name.as_ref(),
        ".git" | ".opticcode" | ".idea" | ".vscode" | "target" | "build"
    )
}

fn should_enter_rag_source(entry: &DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
    !matches!(
        name.as_str(),
        ".git"
            | ".opticcode"
            | ".gradle"
            | ".idea"
            | ".settings"
            | ".vscode"
            | "target"
            | "build"
            | "bin"
            | "classes"
            | "out"
            | "lib"
            | "libs"
            | "node_modules"
    )
}

fn is_probably_text(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return true;
    };

    matches!(
        extension.to_ascii_lowercase().as_str(),
        "rs" | "toml"
            | "md"
            | "txt"
            | "java"
            | "xml"
            | "yml"
            | "yaml"
            | "json"
            | "properties"
            | "gradle"
            | "kt"
            | "kts"
            | "gitignore"
    )
}

fn to_relative(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn top_extensions(extensions: &BTreeMap<String, usize>, limit: usize) -> Vec<(&str, usize)> {
    let mut values = extensions
        .iter()
        .map(|(extension, count)| (extension.as_str(), *count))
        .collect::<Vec<_>>();
    values.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(right.0)));
    values.truncate(limit);
    values
}

fn top_named_counts(values: &BTreeMap<String, usize>, limit: usize) -> Vec<(&str, usize)> {
    let mut values = values
        .iter()
        .map(|(name, count)| (name.as_str(), *count))
        .collect::<Vec<_>>();
    values.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(right.0)));
    values.truncate(limit);
    values
}

#[cfg(test)]
mod tests {
    use super::{
        apply_java_legacy_patch_in_place, apply_java_legacy_patch_to_copy,
        apply_legacy_replacements, build_unified_diff, check_patch_with_git, chunk_text,
        collect_project_risks, context_priority, is_important_rag_file, is_legacy_resource_match,
        is_probably_text, is_rag_indexable_text, parse_java_file, parse_plugin_yml, parse_pom,
        propose_java_legacy_patch, resource_pack_category, summarize_build_output, tail_lines,
        truncate_chars, undo_apply_run, ApplyLogEntry, ApplyPlan, BuildTool, LineEndingStyle,
        PatchCheckResult, PatchFileChange, PatchProposal,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{stamp}"))
    }

    fn all_newlines_are_crlf(bytes: &[u8]) -> bool {
        bytes
            .iter()
            .enumerate()
            .filter(|(_, byte)| **byte == b'\n')
            .all(|(index, _)| index > 0 && bytes[index - 1] == b'\r')
    }

    fn initialize_git_fixture(root: &Path) {
        let run = |args: &[&str]| {
            let output = Command::new("git")
                .args(["-c", "core.autocrlf=false", "-c", "commit.gpgsign=false"])
                .args(args)
                .current_dir(root)
                .output()
                .expect("Git fixture command should start");
            assert!(
                output.status.success(),
                "Git fixture command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&["init", "--quiet"]);
        run(&["add", "--all"]);
        run(&[
            "-c",
            "user.name=OpticCode Test",
            "-c",
            "user.email=opticcode-test@example.invalid",
            "commit",
            "--quiet",
            "--no-verify",
            "-m",
            "fixture",
        ]);
    }

    #[test]
    fn detects_common_project_text_files() {
        assert!(is_probably_text(Path::new("pom.xml")));
        assert!(is_probably_text(Path::new("plugin.yml")));
        assert!(is_probably_text(Path::new("src/Main.java")));
        assert!(!is_probably_text(Path::new("server.jar")));
    }

    #[test]
    fn categorizes_resource_pack_paths() {
        assert_eq!(
            resource_pack_category("assets/minecraft/models/item/nether_wart.json"),
            "models/item"
        );
        assert_eq!(
            resource_pack_category("assets/minecraft/mcpatcher/cit/item_gui/spawners/a.properties"),
            "mcpatcher/cit"
        );
        assert!(is_legacy_resource_match(
            "assets/minecraft/textures/items/wood_shovel.png"
        ));
        assert!(is_legacy_resource_match(
            "assets/minecraft/models/item/mob_spawner.json"
        ));
    }

    #[test]
    fn detects_rag_indexable_and_important_files() {
        assert!(is_rag_indexable_text(Path::new("src/main/java/Main.java")));
        assert!(is_rag_indexable_text(Path::new(
            "patches/server/0001.patch"
        )));
        assert!(!is_rag_indexable_text(Path::new("target/plugin.jar")));
        assert!(is_important_rag_file(Path::new("plugin.yml")));
        assert!(is_important_rag_file(Path::new(
            "src/main/resources/config.yml"
        )));
        assert!(is_important_rag_file(Path::new(
            "patches/server/0001.patch"
        )));
    }

    #[test]
    fn chunks_text_with_minimum_size() {
        let chunks = chunk_text(&"a".repeat(1200), 200);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 512));
    }

    #[test]
    fn prioritizes_bukkit_context_files() {
        assert!(context_priority(Path::new("pom.xml")) < context_priority(Path::new("README.md")));
        assert!(
            context_priority(Path::new("src/main/resources/plugin.yml"))
                < context_priority(Path::new("src/main/java/Foo.java"))
        );
    }

    #[test]
    fn truncates_by_chars() {
        let (content, truncated) = truncate_chars("abcdef", 3);
        assert_eq!(content, "abc");
        assert!(truncated);
    }

    #[test]
    fn parses_maven_java_8_properties() {
        let pom = r#"
            <project>
              <groupId>dev.test</groupId>
              <artifactId>demo</artifactId>
              <version>1.0</version>
              <properties>
                <maven.compiler.source>1.8</maven.compiler.source>
                <maven.compiler.target>1.8</maven.compiler.target>
              </properties>
              <dependencies>
                <dependency>
                  <groupId>org.spigotmc</groupId>
                  <artifactId>spigot-api</artifactId>
                  <version>1.8.8-R0.1-SNAPSHOT</version>
                  <scope>provided</scope>
                </dependency>
              </dependencies>
            </project>
        "#;

        let parsed = parse_pom(pom).expect("pom should parse");

        assert_eq!(parsed.source.as_deref(), Some("1.8"));
        assert_eq!(parsed.target.as_deref(), Some("1.8"));
        assert_eq!(parsed.dependencies.len(), 1);
    }

    #[test]
    fn parses_plugin_yml_commands() {
        let plugin = r#"
name: Demo
version: 1.0
main: dev.test.DemoPlugin
commands:
  coins:
    description: Coins
"#;

        let parsed = parse_plugin_yml(plugin).expect("plugin.yml should parse");

        assert_eq!(parsed.main.as_deref(), Some("dev.test.DemoPlugin"));
        assert_eq!(parsed.commands, vec!["coins"]);
        assert!(!parsed.has_api_version);
    }

    #[test]
    fn detects_java_command_executor_and_listener() {
        let file = parse_java_file(
            Path::new("root"),
            Path::new("root/src/Test.java"),
            "package dev.test;\nimport org.bukkit.command.CommandExecutor;\npublic class Test implements CommandExecutor { @EventHandler public void onJoin() {} }",
        );

        assert_eq!(file.package_name.as_deref(), Some("dev.test"));
        assert_eq!(file.class_name.as_deref(), Some("Test"));
        assert!(file.is_command_executor);
    }

    #[test]
    fn detects_get_command_registrations() {
        let file = parse_java_file(
            Path::new("root"),
            Path::new("root/src/Test.java"),
            "class Test { void onEnable() { getCommand(\"coins\"); getCommand(\"spawn\"); } }",
        );

        assert_eq!(file.registered_commands, vec!["coins", "spawn"]);
    }

    #[test]
    fn flags_plugin_command_registration_mismatch() {
        let plugin = parse_plugin_yml(
            r#"
name: Demo
version: 1.0
main: dev.test.DemoPlugin
commands:
  coins:
    description: Coins
"#,
        )
        .expect("plugin should parse");
        let java_file = parse_java_file(
            Path::new("root"),
            Path::new("root/src/Test.java"),
            "class Test { void onEnable() { getCommand(\"spawn\"); } }",
        );

        let risks = collect_project_risks(BuildTool::Maven, None, Some(&plugin), &[java_file]);

        assert!(risks
            .iter()
            .any(|risk| risk.contains("plugin.yml declares command `coins`")));
        assert!(risks
            .iter()
            .any(|risk| risk.contains("Java registers command `spawn`")));
    }

    #[test]
    fn summarizes_maven_errors() {
        let summary = summarize_build_output(
            "",
            "[ERROR] Failed to execute goal\n[ERROR] cannot find symbol\n[ERROR] symbol: variable GUNPOWDER\n[ERROR] location: class org.bukkit.Material\n",
            false,
        );

        assert!(summary
            .iter()
            .any(|line| line.contains("cannot find symbol")));
        assert!(summary.iter().any(|line| line.contains("Material.SULPHUR")));
    }

    #[test]
    fn applies_known_legacy_symbol_replacements() {
        let original = "Material.WOODEN_SHOVEL\nMaterial.NETHER_WART\nMaterial.SPAWNER\nMaterial.SPAWN_EGG\nEntityType.ZOMBIFIED_PIGLIN";
        let (proposed, replacements) =
            apply_legacy_replacements(original).expect("legacy replacements should be found");

        assert_eq!(replacements.len(), 5);
        assert!(proposed.contains("Material.WOOD_SPADE"));
        assert!(proposed.contains("Material.NETHER_STALK"));
        assert!(proposed.contains("Material.MOB_SPAWNER"));
        assert!(proposed.contains("Material.MONSTER_EGG"));
        assert!(proposed.contains("EntityType.PIG_ZOMBIE"));
    }

    #[test]
    fn removes_plugin_yml_api_version_for_legacy_bukkit_patch() {
        let root = unique_temp_dir("opticcode-plugin-yml-patch");
        fs::create_dir_all(root.join("src/main/resources")).expect("resource dirs");
        fs::write(
            root.join("pom.xml"),
            "<project><dependencies><dependency><groupId>org.spigotmc</groupId><artifactId>spigot-api</artifactId><version>1.8.8-R0.1-SNAPSHOT</version></dependency></dependencies></project>",
        )
        .expect("pom");
        let plugin_path = root.join("src/main/resources/plugin.yml");
        fs::write(
            &plugin_path,
            "name: Demo\nversion: 1.0\nmain: dev.test.DemoPlugin\napi-version: 1.13\ncommands:\n  demo:\n    description: Demo\n",
        )
        .expect("plugin.yml");

        let proposal = propose_java_legacy_patch(&root).expect("proposal");
        let unchanged = fs::read_to_string(&plugin_path).expect("plugin.yml unchanged");

        assert_eq!(proposal.changes.len(), 1);
        assert_eq!(
            proposal.changes[0].path,
            Path::new("src/main/resources/plugin.yml")
        );
        assert!(proposal.changes[0].diff.contains("-api-version: 1.13"));
        assert!(proposal.changes[0]
            .diff
            .contains("+# api-version disabled for Bukkit 1.8.8 compatibility"));
        assert!(unchanged.contains("api-version: 1.13"));
    }

    #[test]
    fn normalizes_line_endings_to_detected_style() {
        let mixed = b"a\r\nb\nc\r\nd\n";

        assert_eq!(
            super::detect_line_ending_style(mixed),
            Some(LineEndingStyle::Crlf)
        );
        assert_eq!(
            super::normalize_line_endings(mixed, LineEndingStyle::Lf),
            b"a\nb\nc\nd\n"
        );
        assert_eq!(
            super::normalize_line_endings(mixed, LineEndingStyle::Crlf),
            b"a\r\nb\r\nc\r\nd\r\n"
        );
    }

    #[test]
    fn preserves_crlf_when_applying_and_undoing_plugin_yml_patch() {
        let root = unique_temp_dir("opticcode-plugin-yml-crlf");
        fs::create_dir_all(root.join("src/main/resources")).expect("resource dirs");
        fs::write(
            root.join("pom.xml"),
            "<project><dependencies><dependency><groupId>org.spigotmc</groupId><artifactId>spigot-api</artifactId><version>1.8.8-R0.1-SNAPSHOT</version></dependency></dependencies></project>",
        )
        .expect("pom");
        let plugin_path = root.join("src/main/resources/plugin.yml");
        fs::write(
            &plugin_path,
            b"name: Demo\r\nversion: 1.0\r\nmain: dev.test.DemoPlugin\r\napi-version: 1.13\r\ncommands:\r\n  demo:\r\n    description: Demo\r\n",
        )
        .expect("plugin.yml");
        initialize_git_fixture(&root);

        let plan = apply_java_legacy_patch_in_place(&root).expect("apply plugin.yml patch");
        let applied = fs::read(&plugin_path).expect("patched plugin.yml");
        let run_id = plan.log.as_ref().expect("apply log").run_id.clone();

        assert!(plan.success());
        assert!(all_newlines_are_crlf(&applied));
        assert!(String::from_utf8_lossy(&applied)
            .contains("# api-version disabled for Bukkit 1.8.8 compatibility"));

        let undo = undo_apply_run(&root, &run_id).expect("undo plugin.yml patch");
        let undone = fs::read(&plugin_path).expect("undone plugin.yml");

        assert!(undo.success());
        assert!(all_newlines_are_crlf(&undone));
        assert!(String::from_utf8_lossy(&undone).contains("api-version: 1.13"));
    }

    #[test]
    fn keeps_tail_lines() {
        assert_eq!(tail_lines("a\nb\nc", 2), "b\nc");
    }

    #[test]
    fn builds_unified_diff_for_line_replacement() {
        let diff = build_unified_diff(
            Path::new("src/Test.java"),
            "class Test {\n  Material.GUNPOWDER;\n}\n",
            "class Test {\n  Material.SULPHUR;\n}\n",
            "test",
        );

        assert!(diff.contains("--- a/src/Test.java"));
        assert!(diff.contains("+++ b/src/Test.java"));
        assert!(diff.contains("-  Material.GUNPOWDER;"));
        assert!(diff.contains("+  Material.SULPHUR;"));
    }

    #[test]
    fn proposes_legacy_material_patch_without_writing_files() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should work")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("opticcode-test-{unique}"));
        let java_dir = root.join("src/main/java/dev/test");
        fs::create_dir_all(&java_dir).expect("test dir should be created");
        fs::write(
            root.join("pom.xml"),
            "<project><properties><maven.compiler.source>1.8</maven.compiler.source><maven.compiler.target>1.8</maven.compiler.target></properties><dependencies><dependency><groupId>org.spigotmc</groupId><artifactId>spigot-api</artifactId><version>1.8.8-R0.1-SNAPSHOT</version></dependency></dependencies></project>",
        )
        .expect("pom should be written");
        let java_path = java_dir.join("Test.java");
        fs::write(
            &java_path,
            "package dev.test;\nclass Test { Object item = Material.GUNPOWDER; Object tool = Material.WOODEN_SHOVEL; }\n",
        )
        .expect("java should be written");

        let proposal = propose_java_legacy_patch(&root).expect("proposal should succeed");
        let unchanged = fs::read_to_string(&java_path).expect("java should still exist");
        fs::remove_dir_all(&root).expect("test dir should be cleaned");

        assert_eq!(proposal.changes.len(), 1);
        assert!(proposal.changes[0].diff.contains("Material.SULPHUR"));
        assert!(proposal.changes[0].diff.contains("Material.WOOD_SPADE"));
        assert!(unchanged.contains("Material.GUNPOWDER"));
        assert!(unchanged.contains("Material.WOODEN_SHOVEL"));
    }

    #[test]
    fn combines_patch_diffs() {
        let proposal = PatchProposal {
            root: Path::new("root").to_path_buf(),
            changes: vec![PatchFileChange {
                path: Path::new("src/Test.java").to_path_buf(),
                reason: "test".to_string(),
                diff: "--- a/src/Test.java\n+++ b/src/Test.java\n".to_string(),
                original_content: Vec::new(),
                proposed_content: Vec::new(),
            }],
            notes: Vec::new(),
        };

        assert_eq!(
            proposal.combined_diff(),
            "--- a/src/Test.java\n+++ b/src/Test.java\n"
        );
    }

    #[test]
    fn displays_apply_dry_run_without_changes() {
        let plan = ApplyPlan {
            proposal: PatchProposal {
                root: Path::new("root").to_path_buf(),
                changes: Vec::new(),
                notes: Vec::new(),
            },
            check: None,
            apply: None,
            copied_from: None,
            dry_run: true,
            log: None,
            transaction: None,
        };
        let display = plan.to_display_string();

        assert!(plan.success());
        assert!(display.contains("Mode: apply dry-run"));
        assert!(display.contains("Changes: 0"));
        assert!(display.contains("No deterministic Java legacy patch"));
    }

    #[test]
    fn displays_apply_dry_run_check_failure() {
        let plan = ApplyPlan {
            proposal: PatchProposal {
                root: Path::new("root").to_path_buf(),
                changes: vec![PatchFileChange {
                    path: Path::new("src/Test.java").to_path_buf(),
                    reason: "test reason".to_string(),
                    diff: String::new(),
                    original_content: Vec::new(),
                    proposed_content: Vec::new(),
                }],
                notes: Vec::new(),
            },
            check: Some(PatchCheckResult {
                command: "git apply --check -".to_string(),
                success: false,
                exit_code: Some(1),
                stdout: String::new(),
                stderr: "patch failed".to_string(),
            }),
            apply: None,
            copied_from: None,
            dry_run: true,
            log: None,
            transaction: None,
        };
        let display = plan.to_display_string();

        assert!(!plan.success());
        assert!(display.contains("Status: FAILED"));
        assert!(display.contains("patch failed"));
        assert!(display.contains("Dry run: no file was modified"));
    }

    #[test]
    fn rejects_invalid_patch_without_modifying_the_target() {
        let root = unique_temp_dir("opticcode-invalid-patch");
        fs::create_dir_all(root.join("src")).expect("source directory should be created");
        let target = root.join("src/Test.java");
        let original = b"class Test {}\n";
        fs::write(&target, original).expect("target should be written");
        initialize_git_fixture(&root);
        let proposal = PatchProposal {
            root: root.clone(),
            changes: vec![PatchFileChange {
                path: PathBuf::from("src/Test.java"),
                reason: "invalid patch fixture".to_string(),
                diff: "this is not a unified patch\n".to_string(),
                original_content: original.to_vec(),
                proposed_content: b"class Test { int changed; }\n".to_vec(),
            }],
            notes: Vec::new(),
        };

        let check = check_patch_with_git(&proposal)
            .expect("patch check should run")
            .expect("proposal should produce a check");

        assert!(!check.success);
        assert_ne!(check.exit_code, Some(0));
        assert_eq!(
            fs::read(&target).expect("target should remain readable"),
            original
        );
        fs::remove_dir_all(root).expect("fixture should be removed");
    }

    #[test]
    fn applies_legacy_patch_to_copy_without_touching_source() {
        let source = unique_temp_dir("opticcode-apply-source");
        let copy = unique_temp_dir("opticcode-apply-copy");
        fs::create_dir_all(source.join("src/main/java/dev/test")).expect("source dirs");
        fs::write(
            source.join("src/main/java/dev/test/Test.java"),
            "package dev.test;\nclass Test { Object item = Material.GUNPOWDER; }\n",
        )
        .expect("write source java");

        let plan = apply_java_legacy_patch_to_copy(&source, &copy).expect("apply to copy");
        let source_content =
            fs::read_to_string(source.join("src/main/java/dev/test/Test.java")).expect("source");
        let copy_content =
            fs::read_to_string(copy.join("src/main/java/dev/test/Test.java")).expect("copy");

        assert!(plan.success());
        assert!(plan.apply.as_ref().is_some_and(|apply| apply.success));
        assert!(source_content.contains("Material.GUNPOWDER"));
        assert!(copy_content.contains("Material.SULPHUR"));
        assert!(!source.join(".opticcode").exists());
        assert!(copy.join(".opticcode/apply-log.jsonl").exists());
        assert!(plan.log.as_ref().is_some_and(|log| log.change_count == 1));
        assert!(plan
            .to_display_string()
            .contains("Applied in copy only; source project was not modified."));
    }

    #[test]
    fn applies_legacy_patch_in_place_with_explicit_function() {
        let root = unique_temp_dir("opticcode-apply-in-place");

        fs::create_dir_all(root.join("src/main/java/dev/test")).expect("source dirs");
        let java_path = root.join("src/main/java/dev/test/Test.java");
        fs::write(
            &java_path,
            "package dev.test;\nclass Test { Object item = Material.GUNPOWDER; }\n",
        )
        .expect("write source java");
        initialize_git_fixture(&root);

        let plan = apply_java_legacy_patch_in_place(&root).expect("apply in place");
        let content = fs::read_to_string(&java_path).expect("patched java");

        assert!(plan.success());
        assert!(plan.apply.as_ref().is_some_and(|apply| apply.success));
        assert!(content.contains("Material.SULPHUR"));
        assert!(!content.contains("Material.GUNPOWDER"));
        let log = plan.log.as_ref().expect("apply log");
        let log_path = root.join(".opticcode/apply-log.jsonl");
        let patch_path = root
            .join(".opticcode/runs")
            .join(&log.run_id)
            .join("patch.diff");
        let log_line = fs::read_to_string(&log_path).expect("apply log jsonl");
        let logged: ApplyLogEntry =
            serde_json::from_str(log_line.trim()).expect("parse apply log entry");

        assert_eq!(logged.run_id, log.run_id);
        assert_eq!(logged.change_count, 1);
        assert!(logged.patch_path.starts_with(".opticcode"));
        assert!(patch_path.exists());
        assert!(fs::read_to_string(&patch_path)
            .expect("patch log")
            .contains("Material.SULPHUR"));
        assert!(logged.rollback_command.contains("git apply -R"));
        assert!(plan
            .to_display_string()
            .contains("Applied in source project."));
        assert!(plan.to_display_string().contains("Apply log:"));
    }

    #[test]
    fn undoes_legacy_patch_from_apply_run() {
        let root = unique_temp_dir("opticcode-apply-undo");

        fs::create_dir_all(root.join("src/main/java/dev/test")).expect("source dirs");
        let java_path = root.join("src/main/java/dev/test/Test.java");
        fs::write(
            &java_path,
            "package dev.test;\nclass Test { Object item = Material.GUNPOWDER; }\n",
        )
        .expect("write source java");
        initialize_git_fixture(&root);

        let plan = apply_java_legacy_patch_in_place(&root).expect("apply in place");
        let run_id = plan.log.as_ref().expect("apply log").run_id.clone();
        assert!(fs::read_to_string(&java_path)
            .expect("patched java")
            .contains("Material.SULPHUR"));

        let undo = undo_apply_run(&root, &run_id).expect("undo apply run");
        let content = fs::read_to_string(&java_path).expect("undone java");

        assert!(undo.success());
        assert!(content.contains("Material.GUNPOWDER"));
        assert!(!content.contains("Material.SULPHUR"));
        assert!(undo.to_display_string().contains("Mode: apply undo"));
        assert!(undo.to_display_string().contains("Undo applied."));
    }
}
