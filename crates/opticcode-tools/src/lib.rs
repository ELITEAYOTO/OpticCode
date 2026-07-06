use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
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
    let name = entry.file_name().to_string_lossy();
    !matches!(
        name.as_ref(),
        ".git"
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

#[cfg(test)]
mod tests {
    use super::{context_priority, is_probably_text, truncate_chars};
    use std::path::Path;

    #[test]
    fn detects_common_project_text_files() {
        assert!(is_probably_text(Path::new("pom.xml")));
        assert!(is_probably_text(Path::new("plugin.yml")));
        assert!(is_probably_text(Path::new("src/Main.java")));
        assert!(!is_probably_text(Path::new("server.jar")));
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
}
