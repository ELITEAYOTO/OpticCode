use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use opticcode_tools::git_state::{capture_git_state, GitChangeKind};
use opticcode_tools::process_runner::{
    run_process_with_cancellation, CancellationToken, ProcessRequest, ProcessStatus,
};
use serde::{Deserialize, Serialize};

use crate::{
    content_hash, ProposalFileSnapshot, ProposalFileStatus, MAX_EDIT_DIFF_DISPLAY_BYTES,
    MAX_EDIT_PROPOSAL_BYTES,
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiffStatistics {
    pub files: usize,
    pub added_files: usize,
    pub modified_files: usize,
    pub deleted_files: usize,
    pub renamed_files: usize,
    pub binary_files: usize,
    pub additions: usize,
    pub deletions: usize,
    pub hunks: usize,
    pub patch_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiffFileStatistics {
    pub path: String,
    pub status: ProposalFileStatus,
    pub additions: usize,
    pub deletions: usize,
    pub hunks: usize,
    pub binary: bool,
    pub base_hash: Option<String>,
    pub proposed_hash: String,
    pub proposed_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedDiff {
    pub schema_version: u32,
    pub patch: String,
    pub patch_hash: String,
    pub display_patch: String,
    pub display_truncated: bool,
    pub statistics: DiffStatistics,
    pub files: Vec<DiffFileStatistics>,
}

pub fn capture_verified_diff(
    worktree_root: &Path,
    files: &[ProposalFileSnapshot],
    git_timeout: Duration,
    cancellation: &CancellationToken,
) -> Result<VerifiedDiff> {
    let state = capture_git_state(worktree_root).context("failed to inspect edited worktree")?;
    let expected = files
        .iter()
        .map(|file| (normalize_path(&file.path), file))
        .collect::<BTreeMap<_, _>>();
    let observed = state
        .changes
        .iter()
        .filter(|change| !is_transaction_state_path(&change.path))
        .map(|change| (normalize_path(&change.path), change))
        .collect::<BTreeMap<_, _>>();
    if expected.keys().collect::<BTreeSet<_>>() != observed.keys().collect::<BTreeSet<_>>() {
        bail!(
            "worktree diff path set differs from the validated proposal: expected {:?}, observed {:?}",
            expected.keys().collect::<Vec<_>>(),
            observed.keys().collect::<Vec<_>>()
        );
    }

    let mut patch = String::new();
    let mut file_stats = Vec::with_capacity(files.len());
    for file in files {
        if cancellation.is_cancelled() {
            bail!("diff capture was cancelled");
        }
        let normalized = normalize_path(&file.path);
        let change = observed
            .get(&normalized)
            .context("validated path disappeared from Git state")?;
        match file.status {
            ProposalFileStatus::Modified
                if change.kind != GitChangeKind::Modified
                    && change.kind != GitChangeKind::TypeChanged =>
            {
                bail!("{} is not reported by Git as a modified file", file.path);
            }
            ProposalFileStatus::Created
                if change.kind != GitChangeKind::Untracked
                    && change.kind != GitChangeKind::Added =>
            {
                bail!("{} is not reported by Git as a created file", file.path);
            }
            _ => {}
        }

        let actual = fs::read(worktree_root.join(Path::new(&file.path)))
            .with_context(|| format!("failed to read proposed file {}", file.path))?;
        if actual != file.proposed_content.as_bytes() || content_hash(&actual) != file.proposed_hash
        {
            bail!("proposed file drifted before diff capture: {}", file.path);
        }

        let file_patch = match file.status {
            ProposalFileStatus::Modified => git_diff_for_path(
                worktree_root,
                &file.path,
                git_timeout,
                MAX_EDIT_PROPOSAL_BYTES,
                cancellation,
            )?,
            ProposalFileStatus::Created => created_file_patch(file),
        };
        let (additions, deletions) = match file.status {
            ProposalFileStatus::Modified => {
                git_numstat_for_path(worktree_root, &file.path, git_timeout, cancellation)?
            }
            ProposalFileStatus::Created => (line_count(&file.proposed_content), 0),
        };
        let hunks = count_hunks(&file_patch);
        if file_patch.contains("GIT binary patch") || file_patch.contains("Binary files ") {
            bail!("binary diff is refused for {}", file.path);
        }
        patch.push_str(&file_patch);
        if !file_patch.ends_with('\n') {
            patch.push('\n');
        }
        file_stats.push(DiffFileStatistics {
            path: file.path.clone(),
            status: file.status,
            additions,
            deletions,
            hunks,
            binary: false,
            base_hash: file.base_hash.clone(),
            proposed_hash: file.proposed_hash.clone(),
            proposed_bytes: file.proposed_bytes,
        });
    }
    if patch.len() > MAX_EDIT_PROPOSAL_BYTES {
        bail!("verified Git patch exceeds the proposal byte limit");
    }

    let statistics = DiffStatistics {
        files: file_stats.len(),
        added_files: file_stats
            .iter()
            .filter(|item| item.status == ProposalFileStatus::Created)
            .count(),
        modified_files: file_stats
            .iter()
            .filter(|item| item.status == ProposalFileStatus::Modified)
            .count(),
        deleted_files: 0,
        renamed_files: 0,
        binary_files: 0,
        additions: file_stats.iter().map(|item| item.additions).sum(),
        deletions: file_stats.iter().map(|item| item.deletions).sum(),
        hunks: file_stats.iter().map(|item| item.hunks).sum(),
        patch_bytes: patch.len(),
    };
    let (display_patch, display_truncated) = truncate_utf8(&patch, MAX_EDIT_DIFF_DISPLAY_BYTES);
    Ok(VerifiedDiff {
        schema_version: 1,
        patch_hash: content_hash(patch.as_bytes()),
        patch,
        display_patch,
        display_truncated,
        statistics,
        files: file_stats,
    })
}

fn git_diff_for_path(
    root: &Path,
    path: &str,
    timeout: Duration,
    output_limit: usize,
    cancellation: &CancellationToken,
) -> Result<String> {
    let result = run_git(
        root,
        [
            "--no-pager",
            "diff",
            "--no-ext-diff",
            "--no-color",
            "--binary",
            "HEAD",
            "--",
            path,
        ],
        timeout,
        output_limit,
        cancellation,
    )?;
    if !result.success() || result.output.output_truncated {
        bail!(
            "git diff failed for {}: status={}, stderr={}",
            path,
            result.status.as_str(),
            result.stderr.trim()
        );
    }
    if result.stdout.is_empty() {
        bail!("Git produced no patch for modified file {path}");
    }
    Ok(result.stdout)
}

fn git_numstat_for_path(
    root: &Path,
    path: &str,
    timeout: Duration,
    cancellation: &CancellationToken,
) -> Result<(usize, usize)> {
    let result = run_git(
        root,
        ["diff", "--numstat", "HEAD", "--", path],
        timeout,
        64 * 1024,
        cancellation,
    )?;
    if !result.success() || result.output.output_truncated {
        bail!("git numstat failed for {path}");
    }
    let line = result
        .stdout
        .lines()
        .next()
        .context("git numstat returned no row")?;
    let mut fields = line.split('\t');
    let additions = fields
        .next()
        .context("git numstat additions are missing")?
        .parse::<usize>()
        .context("binary or invalid additions in git numstat")?;
    let deletions = fields
        .next()
        .context("git numstat deletions are missing")?
        .parse::<usize>()
        .context("binary or invalid deletions in git numstat")?;
    Ok((additions, deletions))
}

fn run_git<const N: usize>(
    root: &Path,
    args: [&str; N],
    timeout: Duration,
    output_limit: usize,
    cancellation: &CancellationToken,
) -> Result<opticcode_tools::process_runner::ProcessResult> {
    let mut request = ProcessRequest::new("git", root);
    request.args = args.into_iter().map(OsString::from).collect();
    request.timeout = timeout;
    request.output_limit_bytes = output_limit;
    let result = run_process_with_cancellation(&request, Some(cancellation))?;
    if matches!(
        result.status,
        ProcessStatus::TimedOut | ProcessStatus::Cancelled
    ) {
        bail!("bounded Git command ended with {}", result.status.as_str());
    }
    Ok(result)
}

fn created_file_patch(file: &ProposalFileSnapshot) -> String {
    let line_count = line_count(&file.proposed_content);
    let mut output = format!(
        "diff --git a/{0} b/{0}\nnew file mode 100644\n--- /dev/null\n+++ b/{0}\n@@ -0,0 +1,{line_count} @@\n",
        file.path.replace('\\', "/")
    );
    for line in file.proposed_content.split_inclusive('\n') {
        output.push('+');
        output.push_str(line);
    }
    if !file.proposed_content.is_empty() && !file.proposed_content.ends_with('\n') {
        output.push_str("\n\\ No newline at end of file\n");
    }
    output
}

fn count_hunks(patch: &str) -> usize {
    patch.lines().filter(|line| line.starts_with("@@ ")).count()
}

fn line_count(value: &str) -> usize {
    if value.is_empty() {
        0
    } else {
        value.bytes().filter(|byte| *byte == b'\n').count() + usize::from(!value.ends_with('\n'))
    }
}

fn truncate_utf8(value: &str, limit: usize) -> (String, bool) {
    if value.len() <= limit {
        return (value.to_string(), false);
    }
    let mut end = limit;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut retained = value[..end].to_string();
    retained.push_str("\n[display truncated; stored patch hash remains authoritative]\n");
    (retained, true)
}

fn normalize_path(value: &str) -> String {
    let mut value = value.replace('\\', "/");
    if cfg!(windows) {
        value.make_ascii_lowercase();
    }
    value
}

fn is_transaction_state_path(value: &str) -> bool {
    let normalized = normalize_path(value);
    normalized == ".opticcode" || normalized.starts_with(".opticcode/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LineEnding, TextEncoding};

    #[test]
    fn created_patch_and_stats_are_bounded_and_path_relative() {
        let snapshot = ProposalFileSnapshot {
            path: "src/main/java/dev/Été.java".to_string(),
            status: ProposalFileStatus::Created,
            encoding: TextEncoding::Utf8,
            line_ending: LineEnding::Lf,
            base_content: None,
            base_hash: None,
            proposed_content: "class Été {}\n".to_string(),
            proposed_hash: content_hash("class Été {}\n".as_bytes()),
            proposed_bytes: "class Été {}\n".len(),
        };
        let patch = created_file_patch(&snapshot);
        assert!(patch.contains("--- /dev/null"));
        assert!(patch.contains("+++ b/src/main/java/dev/Été.java"));
        assert!(!patch.contains("C:\\Users"));
        assert_eq!(count_hunks(&patch), 1);
    }

    #[test]
    fn utf8_display_truncation_never_splits_a_character() {
        let (value, truncated) = truncate_utf8("ééé", 3);
        assert!(truncated);
        assert!(value.starts_with('é'));
    }
}
