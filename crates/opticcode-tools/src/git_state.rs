//! Stable Git worktree snapshots and build-change attribution.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

const GIT_STATUS_ARGS: &[&str] = &[
    "-c",
    "core.quotepath=false",
    "-c",
    "status.relativePaths=false",
    "status",
    "--porcelain=v1",
    "-z",
    "--untracked-files=all",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitStateSnapshot {
    pub schema_version: u32,
    pub root: PathBuf,
    pub changes: Vec<GitChange>,
    pub metrics: GitSnapshotMetrics,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitSnapshotMetrics {
    pub duration_us: u64,
    pub status_entries: usize,
    pub fingerprinted_files: usize,
    pub fingerprinted_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitChange {
    pub index_status: char,
    pub worktree_status: char,
    pub kind: GitChangeKind,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitChangeKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    Untracked,
    TypeChanged,
    Unmerged,
    Ignored,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitChangeOrigin {
    PreExisting,
    BuildGenerated,
    TrackedChanged,
    UntrackedGenerated,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassifiedGitChange {
    pub change: GitChange,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<GitChange>,
    pub origin: GitChangeOrigin,
    pub existed_before: bool,
    pub changed_during_build: bool,
    pub tracked_was_clean_before: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitStateDiffCounts {
    pub before: usize,
    pub after: usize,
    pub pre_existing: usize,
    pub build_generated: usize,
    pub tracked_changed: usize,
    pub untracked_generated: usize,
    pub unknown: usize,
    pub changed_during_build: usize,
    pub resolved_pre_existing: usize,
    pub strict_candidates: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitStateDiff {
    pub changes_after: Vec<ClassifiedGitChange>,
    pub resolved_pre_existing: Vec<GitChange>,
    pub counts: GitStateDiffCounts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitGuardStatus {
    Captured,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitStrictPolicy {
    pub enabled: bool,
    pub passed: bool,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildGitReport {
    pub schema_version: u32,
    pub status: GitGuardStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<GitStateSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<GitStateSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<GitStateDiff>,
    pub strict_policy: GitStrictPolicy,
}

pub fn parse_porcelain_v1_z(input: &[u8]) -> Result<Vec<GitChange>> {
    let mut cursor = 0usize;
    let mut changes = Vec::new();

    while cursor < input.len() {
        let record = read_nul_field(input, &mut cursor, "status entry")?;
        if record.len() < 4 {
            bail!("invalid porcelain v1 entry: expected XY, space, and path");
        }
        if record[2] != b' ' {
            bail!("invalid porcelain v1 entry: missing separator after XY status");
        }
        if !record[0].is_ascii() || !record[1].is_ascii() {
            bail!("invalid porcelain v1 entry: status bytes must be ASCII");
        }

        let index_status = char::from(record[0]);
        let worktree_status = char::from(record[1]);
        let path = decode_path(&record[3..], "path")?;
        let kind = classify_kind(index_status, worktree_status);
        let original_path = if matches!(kind, GitChangeKind::Renamed | GitChangeKind::Copied) {
            Some(decode_path(
                read_nul_field(input, &mut cursor, "rename/copy source path")?,
                "rename/copy source path",
            )?)
        } else {
            None
        };

        changes.push(GitChange {
            index_status,
            worktree_status,
            kind,
            path,
            original_path,
            content_fingerprint: None,
        });
    }

    Ok(changes)
}

pub fn capture_git_state(path: &Path) -> Result<GitStateSnapshot> {
    let started_at = Instant::now();
    let root = discover_git_root(path)?;
    let output = run_git_status(&root)?;
    let mut changes = parse_porcelain_v1_z(&output)?;
    let mut fingerprinted_files = 0usize;
    let mut fingerprinted_bytes = 0u64;

    for change in &mut changes {
        if let Some(fingerprint) = fingerprint_worktree_path(&root, &change.path) {
            fingerprinted_files += 1;
            fingerprinted_bytes = fingerprinted_bytes.saturating_add(fingerprint.bytes_read);
            change.content_fingerprint = Some(fingerprint.value);
        }
    }
    changes.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.original_path.cmp(&right.original_path))
    });

    Ok(GitStateSnapshot {
        schema_version: 1,
        root,
        metrics: GitSnapshotMetrics {
            duration_us: duration_us(started_at),
            status_entries: changes.len(),
            fingerprinted_files,
            fingerprinted_bytes,
        },
        changes,
    })
}

pub fn compare_git_states(
    before: &GitStateSnapshot,
    after: &GitStateSnapshot,
) -> Result<GitStateDiff> {
    if !same_path(&before.root, &after.root) {
        bail!(
            "Git root changed during build: {} -> {}",
            before.root.display(),
            after.root.display()
        );
    }

    let mut matched_before = BTreeSet::new();
    let mut changes_after = Vec::new();

    for after_change in &after.changes {
        let before_match = before
            .changes
            .iter()
            .enumerate()
            .find(|(index, before_change)| {
                !matched_before.contains(index) && changes_overlap(before_change, after_change)
            });

        let (before_index, previous) = before_match
            .map(|(index, change)| (Some(index), Some(change.clone())))
            .unwrap_or((None, None));
        if let Some(index) = before_index {
            matched_before.insert(index);
        }

        let existed_before = previous.is_some();
        let changed_during_build = previous
            .as_ref()
            .is_none_or(|before_change| before_change != after_change);
        let tracked_was_clean_before = !existed_before && after_change.is_tracked();
        let origin = classify_origin(after_change, existed_before, changed_during_build);

        changes_after.push(ClassifiedGitChange {
            change: after_change.clone(),
            before: previous,
            origin,
            existed_before,
            changed_during_build,
            tracked_was_clean_before,
        });
    }

    let resolved_pre_existing = before
        .changes
        .iter()
        .enumerate()
        .filter(|(index, _)| !matched_before.contains(index))
        .map(|(_, change)| change.clone())
        .collect::<Vec<_>>();
    let counts = count_diff(before, after, &changes_after, &resolved_pre_existing);

    Ok(GitStateDiff {
        changes_after,
        resolved_pre_existing,
        counts,
    })
}

impl BuildGitReport {
    pub fn from_snapshots(
        before: GitStateSnapshot,
        after: GitStateSnapshot,
        strict: bool,
    ) -> Result<Self> {
        let diff = compare_git_states(&before, &after)?;
        let reasons = if strict {
            strict_reasons(&diff)
        } else {
            Vec::new()
        };

        Ok(Self {
            schema_version: 1,
            status: GitGuardStatus::Captured,
            message: None,
            before: Some(before),
            after: Some(after),
            diff: Some(diff),
            strict_policy: GitStrictPolicy {
                enabled: strict,
                passed: reasons.is_empty(),
                reasons,
            },
        })
    }

    pub fn from_capture_results(
        before: Result<GitStateSnapshot>,
        after: Result<GitStateSnapshot>,
        strict: bool,
    ) -> Self {
        match (before, after) {
            (Ok(before), Ok(after)) => Self::from_snapshots(before, after, strict)
                .unwrap_or_else(|error| Self::unavailable(error.to_string(), strict)),
            (before, after) => {
                let mut messages = Vec::new();
                if let Err(error) = before {
                    messages.push(format!("before build: {error:#}"));
                }
                if let Err(error) = after {
                    messages.push(format!("after build: {error:#}"));
                }
                Self::unavailable(messages.join("; "), strict)
            }
        }
    }

    pub fn unavailable(message: String, strict: bool) -> Self {
        let reasons = if strict {
            vec![format!(
                "strict Git guard could not verify the worktree: {message}"
            )]
        } else {
            Vec::new()
        };

        Self {
            schema_version: 1,
            status: GitGuardStatus::Unavailable,
            message: Some(message),
            before: None,
            after: None,
            diff: None,
            strict_policy: GitStrictPolicy {
                enabled: strict,
                passed: reasons.is_empty(),
                reasons,
            },
        }
    }

    pub fn strict_violation(&self) -> bool {
        self.strict_policy.enabled && !self.strict_policy.passed
    }

    pub fn to_display_string(&self) -> String {
        let mut out = String::new();
        out.push_str("Git state guard:\n");
        out.push_str(&format!("- status: {}\n", self.status.as_str()));

        if let Some(message) = &self.message {
            out.push_str(&format!("- message: {message}\n"));
        }

        if let (Some(before), Some(after), Some(diff)) = (&self.before, &self.after, &self.diff) {
            out.push_str(&format!("- Git root: {}\n", before.root.display()));
            out.push_str(&format!("- changes before: {}\n", before.changes.len()));
            out.push_str(&format!("- changes after: {}\n", after.changes.len()));
            out.push_str(&format!(
                "- snapshot before: {:.3} ms, {} file(s) fingerprinted, {} byte(s) read\n",
                before.metrics.duration_us as f64 / 1_000.0,
                before.metrics.fingerprinted_files,
                before.metrics.fingerprinted_bytes
            ));
            out.push_str(&format!(
                "- snapshot after: {:.3} ms, {} file(s) fingerprinted, {} byte(s) read\n",
                after.metrics.duration_us as f64 / 1_000.0,
                after.metrics.fingerprinted_files,
                after.metrics.fingerprinted_bytes
            ));
            out.push_str(&format!(
                "- changed during build: {}\n",
                diff.counts.changed_during_build
            ));
            out.push_str(&format!(
                "- strict candidates: {}\n",
                diff.counts.strict_candidates
            ));

            let changes_during_build = diff
                .changes_after
                .iter()
                .filter(|classified| classified.changed_during_build)
                .collect::<Vec<_>>();
            if diff.changes_after.is_empty() {
                out.push_str("- worktree clean before and after build\n");
            } else if changes_during_build.is_empty() {
                out.push_str("- no worktree state changed during build\n");
            } else {
                out.push_str("\nChanges detected during build:\n");
                for classified in changes_during_build {
                    let strict = if strict_candidate(classified) {
                        " [strict]"
                    } else {
                        ""
                    };
                    let evolved = if classified.existed_before && classified.changed_during_build {
                        " [pre-existing state changed]"
                    } else {
                        ""
                    };
                    out.push_str(&format!(
                        "- {} {}: {}{}{}\n",
                        classified.origin.as_str(),
                        classified.change.kind.as_str(),
                        classified.change.display_path(),
                        strict,
                        evolved
                    ));
                }
            }

            if diff.counts.pre_existing > 0 {
                out.push_str(&format!(
                    "- unchanged pre-existing changes: {} (full details available with --json)\n",
                    diff.counts.pre_existing
                ));
            }

            if !diff.resolved_pre_existing.is_empty() {
                out.push_str("\nPre-existing changes resolved during build:\n");
                for change in &diff.resolved_pre_existing {
                    out.push_str(&format!(
                        "- {}: {}\n",
                        change.kind.as_str(),
                        change.display_path()
                    ));
                }
            }
        }

        if self.strict_policy.enabled {
            out.push_str(&format!(
                "\nStrict policy: {}\n",
                if self.strict_policy.passed {
                    "PASSED"
                } else {
                    "FAILED"
                }
            ));
            for reason in &self.strict_policy.reasons {
                out.push_str(&format!("- {reason}\n"));
            }
        } else if let Some(diff) = &self.diff {
            out.push_str(&format!(
                "\nStrict policy: disabled ({} change(s) would fail strict mode)\n",
                diff.counts.strict_candidates
            ));
        } else {
            out.push_str("\nStrict policy: disabled\n");
        }

        out
    }
}

impl GitStateSnapshot {
    pub fn to_display_string(&self) -> String {
        let mut out = String::new();
        out.push_str("Git state snapshot:\n");
        out.push_str(&format!("- root: {}\n", self.root.display()));
        out.push_str(&format!(
            "- duration: {:.3} ms\n",
            self.metrics.duration_us as f64 / 1_000.0
        ));
        out.push_str(&format!(
            "- status entries: {}\n",
            self.metrics.status_entries
        ));
        out.push_str(&format!(
            "- fingerprinted files: {}\n",
            self.metrics.fingerprinted_files
        ));
        out.push_str(&format!(
            "- fingerprinted bytes: {}\n",
            self.metrics.fingerprinted_bytes
        ));

        if self.changes.is_empty() {
            out.push_str("\nWorktree clean.\n");
            return out;
        }

        out.push_str("\nChanges:\n");
        for change in self.changes.iter().take(100) {
            out.push_str(&format!(
                "- {}{} {}: {}\n",
                change.index_status,
                change.worktree_status,
                change.kind.as_str(),
                change.display_path()
            ));
        }
        if self.changes.len() > 100 {
            out.push_str(&format!(
                "- ... {} additional entries; use --json for full details\n",
                self.changes.len() - 100
            ));
        }
        out
    }
}

impl GitChange {
    pub fn is_tracked(&self) -> bool {
        !matches!(self.kind, GitChangeKind::Untracked | GitChangeKind::Ignored)
    }

    pub fn display_path(&self) -> String {
        self.original_path.as_ref().map_or_else(
            || self.path.clone(),
            |original| format!("{original} -> {}", self.path),
        )
    }
}

impl GitChangeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Modified => "modified",
            Self::Added => "added",
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
            Self::Copied => "copied",
            Self::Untracked => "untracked",
            Self::TypeChanged => "type_changed",
            Self::Unmerged => "unmerged",
            Self::Ignored => "ignored",
            Self::Unknown => "unknown",
        }
    }
}

impl GitChangeOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PreExisting => "pre_existing",
            Self::BuildGenerated => "build_generated",
            Self::TrackedChanged => "tracked_changed",
            Self::UntrackedGenerated => "untracked_generated",
            Self::Unknown => "unknown",
        }
    }
}

impl GitGuardStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Captured => "captured",
            Self::Unavailable => "unavailable",
        }
    }
}

fn discover_git_root(path: &Path) -> Result<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(path)
        .output()
        .with_context(|| format!("failed to discover Git root from {}", path.display()))?;
    if !output.status.success() {
        bail!(
            "not a Git worktree from {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let root = String::from_utf8(output.stdout)
        .context("Git root is not valid UTF-8")?
        .trim_end_matches(['\r', '\n'])
        .to_string();
    if root.is_empty() {
        bail!("Git returned an empty worktree root");
    }
    fs::canonicalize(&root).with_context(|| format!("failed to resolve Git root: {root}"))
}

fn run_git_status(root: &Path) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .args(GIT_STATUS_ARGS)
        .current_dir(root)
        .output()
        .with_context(|| format!("failed to capture Git status in {}", root.display()))?;
    if !output.status.success() {
        bail!(
            "git status failed in {}: {}",
            root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

fn read_nul_field<'a>(input: &'a [u8], cursor: &mut usize, label: &str) -> Result<&'a [u8]> {
    let remaining = &input[*cursor..];
    let Some(length) = remaining.iter().position(|byte| *byte == 0) else {
        bail!("truncated porcelain v1 output: missing NUL after {label}");
    };
    let field = &remaining[..length];
    *cursor += length + 1;
    Ok(field)
}

fn decode_path(bytes: &[u8], label: &str) -> Result<String> {
    if bytes.is_empty() {
        bail!("invalid porcelain v1 output: empty {label}");
    }
    String::from_utf8(bytes.to_vec()).with_context(|| format!("{label} is not valid UTF-8"))
}

fn classify_kind(index: char, worktree: char) -> GitChangeKind {
    let pair = [index, worktree];
    if pair == ['?', '?'] {
        GitChangeKind::Untracked
    } else if pair == ['!', '!'] {
        GitChangeKind::Ignored
    } else if is_unmerged(index, worktree) {
        GitChangeKind::Unmerged
    } else if pair.contains(&'R') {
        GitChangeKind::Renamed
    } else if pair.contains(&'C') {
        GitChangeKind::Copied
    } else if pair.contains(&'D') {
        GitChangeKind::Deleted
    } else if pair.contains(&'A') {
        GitChangeKind::Added
    } else if pair.contains(&'T') {
        GitChangeKind::TypeChanged
    } else if pair.contains(&'M') {
        GitChangeKind::Modified
    } else {
        GitChangeKind::Unknown
    }
}

fn is_unmerged(index: char, worktree: char) -> bool {
    matches!(
        (index, worktree),
        ('D', 'D') | ('A', 'U') | ('U', 'D') | ('U', 'A') | ('D', 'U') | ('A', 'A') | ('U', 'U')
    )
}

struct FileFingerprint {
    value: String,
    bytes_read: u64,
}

fn fingerprint_worktree_path(root: &Path, relative: &str) -> Option<FileFingerprint> {
    let path = root.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
    let file = File::open(path).ok()?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut buffer = [0u8; 64 * 1024];
    let mut hasher = blake3::Hasher::new();
    let mut length = 0u64;

    loop {
        let read = reader.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        length = length.saturating_add(read as u64);
        hasher.update(&buffer[..read]);
    }

    Some(FileFingerprint {
        value: format!("blake3:{length}:{}", hasher.finalize().to_hex()),
        bytes_read: length,
    })
}

fn duration_us(started_at: Instant) -> u64 {
    started_at.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
}

fn same_path(left: &Path, right: &Path) -> bool {
    if cfg!(windows) {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    } else {
        left == right
    }
}

fn changes_overlap(left: &GitChange, right: &GitChange) -> bool {
    let left_paths = affected_paths(left);
    let right_paths = affected_paths(right);
    left_paths
        .iter()
        .any(|left_path| right_paths.contains(left_path))
}

fn affected_paths(change: &GitChange) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    paths.insert(normalize_comparison_path(&change.path));
    if let Some(original) = &change.original_path {
        paths.insert(normalize_comparison_path(original));
    }
    paths
}

fn normalize_comparison_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    if cfg!(windows) {
        normalized.to_ascii_lowercase()
    } else {
        normalized
    }
}

fn classify_origin(
    change: &GitChange,
    existed_before: bool,
    changed_during_build: bool,
) -> GitChangeOrigin {
    if existed_before && !changed_during_build {
        GitChangeOrigin::PreExisting
    } else if matches!(
        change.kind,
        GitChangeKind::Unknown | GitChangeKind::Unmerged
    ) {
        GitChangeOrigin::Unknown
    } else if change.kind == GitChangeKind::Untracked {
        GitChangeOrigin::UntrackedGenerated
    } else if is_expected_build_generated_path(&change.path) {
        GitChangeOrigin::BuildGenerated
    } else if change.is_tracked() {
        GitChangeOrigin::TrackedChanged
    } else {
        GitChangeOrigin::Unknown
    }
}

fn is_expected_build_generated_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    let file_name = normalized.rsplit('/').next().unwrap_or(normalized.as_str());
    let known_file = matches!(
        file_name,
        "dependency-reduced-pom.xml"
            | ".flattened-pom.xml"
            | "pom.xml.versionsbackup"
            | "pom.xml.releasebackup"
            | "release.properties"
            | "buildnumber.properties"
    );
    let known_directory = normalized.split('/').any(|component| {
        matches!(
            component,
            "target"
                | "build"
                | "out"
                | ".gradle"
                | "generated-sources"
                | "generated-test-sources"
                | "surefire-reports"
                | "test-results"
                | "reports"
        )
    });
    known_file || known_directory
}

fn count_diff(
    before: &GitStateSnapshot,
    after: &GitStateSnapshot,
    changes: &[ClassifiedGitChange],
    resolved: &[GitChange],
) -> GitStateDiffCounts {
    let mut counts = GitStateDiffCounts {
        before: before.changes.len(),
        after: after.changes.len(),
        resolved_pre_existing: resolved.len(),
        ..GitStateDiffCounts::default()
    };

    for classified in changes {
        match classified.origin {
            GitChangeOrigin::PreExisting => counts.pre_existing += 1,
            GitChangeOrigin::BuildGenerated => counts.build_generated += 1,
            GitChangeOrigin::TrackedChanged => counts.tracked_changed += 1,
            GitChangeOrigin::UntrackedGenerated => counts.untracked_generated += 1,
            GitChangeOrigin::Unknown => counts.unknown += 1,
        }
        if classified.changed_during_build {
            counts.changed_during_build += 1;
        }
        if strict_candidate(classified) {
            counts.strict_candidates += 1;
        }
    }
    counts.changed_during_build = counts.changed_during_build.saturating_add(resolved.len());
    counts.strict_candidates = counts
        .strict_candidates
        .saturating_add(resolved.iter().filter(|change| change.is_tracked()).count());

    counts
}

fn strict_reasons(diff: &GitStateDiff) -> Vec<String> {
    let mut reasons = diff
        .changes_after
        .iter()
        .filter(|classified| strict_candidate(classified))
        .map(|classified| {
            if classified.tracked_was_clean_before {
                format!(
                    "tracked file was clean before build and changed: {} {} (origin={})",
                    classified.change.kind.as_str(),
                    classified.change.display_path(),
                    classified.origin.as_str()
                )
            } else {
                format!(
                    "tracked file changed again during build: {} {} (origin={})",
                    classified.change.kind.as_str(),
                    classified.change.display_path(),
                    classified.origin.as_str()
                )
            }
        })
        .collect::<Vec<_>>();
    reasons.extend(
        diff.resolved_pre_existing
            .iter()
            .filter(|change| change.is_tracked())
            .map(|change| {
                format!(
                    "tracked pre-existing change was removed or restored during build: {} {}",
                    change.kind.as_str(),
                    change.display_path()
                )
            }),
    );
    reasons
}

fn strict_candidate(classified: &ClassifiedGitChange) -> bool {
    classified.changed_during_build && classified.change.is_tracked()
}

#[cfg(test)]
mod tests {
    use super::{
        compare_git_states, parse_porcelain_v1_z, BuildGitReport, GitChange, GitChangeKind,
        GitChangeOrigin, GitStateSnapshot,
    };
    use std::path::PathBuf;

    #[test]
    fn parses_modified_added_deleted_and_untracked_entries() {
        let input = b" M src/Main.java\0A  src/New.java\0 D src/Old.java\0?? notes.txt\0";
        let changes = parse_porcelain_v1_z(input).expect("porcelain should parse");

        assert_eq!(changes.len(), 4);
        assert_eq!(changes[0].kind, GitChangeKind::Modified);
        assert_eq!(changes[1].kind, GitChangeKind::Added);
        assert_eq!(changes[2].kind, GitChangeKind::Deleted);
        assert_eq!(changes[3].kind, GitChangeKind::Untracked);
    }

    #[test]
    fn parses_rename_with_spaces_in_nul_format() {
        let changes = parse_porcelain_v1_z(b"R  src/New Name.java\0src/Old Name.java\0")
            .expect("rename should parse");

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, GitChangeKind::Renamed);
        assert_eq!(changes[0].path, "src/New Name.java");
        assert_eq!(
            changes[0].original_path.as_deref(),
            Some("src/Old Name.java")
        );
    }

    #[test]
    fn parses_unicode_and_windows_separator_paths() {
        let input = "?? docs/résumé Minecraft.md\0 M src\\legacy\\Épée.java\0";
        let changes = parse_porcelain_v1_z(input.as_bytes()).expect("Unicode should parse");

        assert_eq!(changes[0].path, "docs/résumé Minecraft.md");
        assert_eq!(changes[1].path, "src\\legacy\\Épée.java");
    }

    #[test]
    fn rejects_truncated_or_invalid_entries() {
        assert!(parse_porcelain_v1_z(b" M src/Main.java").is_err());
        assert!(parse_porcelain_v1_z(b"M src/Main.java\0").is_err());
        assert!(parse_porcelain_v1_z(b"R  new.java\0").is_err());
        assert!(parse_porcelain_v1_z(b"?? \0").is_err());
    }

    #[test]
    fn classifies_pre_existing_generated_and_strict_changes() {
        let root = PathBuf::from("C:/repo");
        let before = GitStateSnapshot {
            schema_version: 1,
            root: root.clone(),
            changes: vec![change(GitChangeKind::Modified, "README.md", Some("before"))],
            metrics: Default::default(),
        };
        let after = GitStateSnapshot {
            schema_version: 1,
            root,
            changes: vec![
                change(GitChangeKind::Modified, "README.md", Some("before")),
                change(
                    GitChangeKind::Modified,
                    "dependency-reduced-pom.xml",
                    Some("generated"),
                ),
                change(GitChangeKind::Modified, "src/Main.java", Some("changed")),
                untracked("target/generated.txt"),
            ],
            metrics: Default::default(),
        };

        let report =
            BuildGitReport::from_snapshots(before, after, true).expect("snapshots should compare");
        let diff = report.diff.expect("captured report should have diff");

        assert_eq!(diff.counts.pre_existing, 1);
        assert_eq!(diff.counts.build_generated, 1);
        assert_eq!(diff.counts.tracked_changed, 1);
        assert_eq!(diff.counts.untracked_generated, 1);
        assert_eq!(diff.counts.strict_candidates, 2);
        assert!(!report.strict_policy.passed);
    }

    #[test]
    fn detects_pre_existing_file_that_changes_again_during_build() {
        let root = PathBuf::from("C:/repo");
        let before = GitStateSnapshot {
            schema_version: 1,
            root: root.clone(),
            changes: vec![change(GitChangeKind::Modified, "README.md", Some("first"))],
            metrics: Default::default(),
        };
        let after = GitStateSnapshot {
            schema_version: 1,
            root,
            changes: vec![change(GitChangeKind::Modified, "README.md", Some("second"))],
            metrics: Default::default(),
        };

        let diff = compare_git_states(&before, &after).expect("snapshots should compare");
        let classified = &diff.changes_after[0];
        assert!(classified.existed_before);
        assert!(classified.changed_during_build);
        assert_eq!(classified.origin, GitChangeOrigin::TrackedChanged);
        assert!(!classified.tracked_was_clean_before);

        let report =
            BuildGitReport::from_snapshots(before, after, true).expect("snapshots should compare");
        assert!(report.strict_violation());
        assert_eq!(
            report
                .diff
                .as_ref()
                .expect("captured report should have diff")
                .counts
                .strict_candidates,
            1
        );
        assert!(report.strict_policy.reasons[0].contains("tracked file changed again during build"));
    }

    #[test]
    fn strict_policy_rejects_a_tracked_change_resolved_during_build() {
        let root = PathBuf::from("C:/repo");
        let before = GitStateSnapshot {
            schema_version: 1,
            root: root.clone(),
            changes: vec![change(
                GitChangeKind::Modified,
                "src/Main.java",
                Some("before"),
            )],
            metrics: Default::default(),
        };
        let after = GitStateSnapshot {
            schema_version: 1,
            root,
            changes: Vec::new(),
            metrics: Default::default(),
        };

        let report =
            BuildGitReport::from_snapshots(before, after, true).expect("snapshots should compare");
        let diff = report
            .diff
            .as_ref()
            .expect("captured report should have diff");
        assert_eq!(diff.counts.resolved_pre_existing, 1);
        assert_eq!(diff.counts.changed_during_build, 1);
        assert_eq!(diff.counts.strict_candidates, 1);
        assert!(report.strict_violation());
        assert!(report.strict_policy.reasons[0]
            .contains("tracked pre-existing change was removed or restored"));
    }

    #[test]
    fn strict_policy_fails_closed_when_git_is_unavailable() {
        let strict = BuildGitReport::unavailable("not a Git worktree".to_string(), true);
        let permissive = BuildGitReport::unavailable("not a Git worktree".to_string(), false);

        assert!(strict.strict_violation());
        assert!(!strict.strict_policy.passed);
        assert_eq!(strict.strict_policy.reasons.len(), 1);
        assert!(!permissive.strict_violation());
        assert!(permissive.strict_policy.passed);
    }

    fn change(kind: GitChangeKind, path: &str, fingerprint: Option<&str>) -> GitChange {
        GitChange {
            index_status: ' ',
            worktree_status: 'M',
            kind,
            path: path.to_string(),
            original_path: None,
            content_fingerprint: fingerprint.map(ToOwned::to_owned),
        }
    }

    fn untracked(path: &str) -> GitChange {
        GitChange {
            index_status: '?',
            worktree_status: '?',
            kind: GitChangeKind::Untracked,
            path: path.to_string(),
            original_path: None,
            content_fingerprint: Some("new".to_string()),
        }
    }
}
