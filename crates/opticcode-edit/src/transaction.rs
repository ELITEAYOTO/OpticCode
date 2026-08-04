use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use opticcode_tools::apply_transaction::FileMutation;
use opticcode_tools::java_syntax::analyze_java_source;

use crate::{
    content_hash, EditStageReport, ProposalFileSnapshot, ProposalFileStatus, ProposalRecord,
};

pub(crate) fn proposal_mutations(files: &[ProposalFileSnapshot]) -> Vec<FileMutation> {
    files
        .iter()
        .map(|file| match file.status {
            ProposalFileStatus::Modified => FileMutation::replace(
                PathBuf::from(&file.path),
                file.base_content.clone().unwrap_or_default().into_bytes(),
                file.proposed_content.clone().into_bytes(),
            ),
            ProposalFileStatus::Created => FileMutation::create(
                PathBuf::from(&file.path),
                file.proposed_content.clone().into_bytes(),
            ),
        })
        .collect()
}

pub(crate) fn worktree_mutations(
    root: &Path,
    files: &[ProposalFileSnapshot],
) -> Result<Vec<FileMutation>> {
    files
        .iter()
        .map(|file| match file.status {
            ProposalFileStatus::Modified => {
                let actual = fs::read(root.join(&file.path))
                    .with_context(|| format!("failed to read worktree base {}", file.path))?;
                let base = file
                    .base_content
                    .as_deref()
                    .context("modified proposal snapshot has no base content")?;
                if actual != base.as_bytes() && !line_endings_equivalent(&actual, base.as_bytes()) {
                    bail!(
                        "{} differs from the source beyond Git line-ending checkout",
                        file.path
                    );
                }
                Ok(FileMutation::replace(
                    PathBuf::from(&file.path),
                    actual,
                    file.proposed_content.clone().into_bytes(),
                ))
            }
            ProposalFileStatus::Created => {
                if root.join(&file.path).exists() {
                    bail!(
                        "created proposal path already exists in worktree: {}",
                        file.path
                    );
                }
                Ok(FileMutation::create(
                    PathBuf::from(&file.path),
                    file.proposed_content.clone().into_bytes(),
                ))
            }
        })
        .collect()
}

pub(crate) fn worktree_expected_hash(
    root: &Path,
    file: &ProposalFileSnapshot,
) -> Result<Option<String>> {
    if file.status == ProposalFileStatus::Created {
        return Ok(None);
    }
    let actual = fs::read(root.join(&file.path))
        .with_context(|| format!("failed to read worktree base {}", file.path))?;
    let base = file
        .base_content
        .as_deref()
        .context("modified proposal snapshot has no base content")?;
    if actual != base.as_bytes() && !line_endings_equivalent(&actual, base.as_bytes()) {
        bail!(
            "{} differs from the source beyond Git line-ending checkout",
            file.path
        );
    }
    Ok(Some(content_hash(&actual)))
}

pub(crate) fn proposal_contract_bytes(record: &ProposalRecord) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(&serde_json::json!({
        "schema_version": record.schema_version,
        "proposal_id": record.proposal_id,
        "base_head": record.plan.base_head,
        "working_tree_digest": record.plan.working_tree_digest,
        "files": record.files.iter().map(|file| serde_json::json!({
            "path": file.path,
            "status": file.status,
            "base_hash": file.base_hash,
            "proposed_hash": file.proposed_hash,
            "proposed_bytes": file.proposed_bytes,
        })).collect::<Vec<_>>(),
    }))?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub(crate) fn proposal_paths(files: &[ProposalFileSnapshot]) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut existing = files
        .iter()
        .filter(|file| file.status == ProposalFileStatus::Modified)
        .map(|file| PathBuf::from(&file.path))
        .collect::<Vec<_>>();
    let mut created = files
        .iter()
        .filter(|file| file.status == ProposalFileStatus::Created)
        .map(|file| PathBuf::from(&file.path))
        .collect::<Vec<_>>();
    existing.sort();
    created.sort();
    (existing, created)
}

pub(crate) fn proposal_files_hash(files: &[ProposalFileSnapshot]) -> String {
    let mut entries = files
        .iter()
        .map(|file| {
            format!(
                "{}\0{:?}\0{}\0{}",
                file.path,
                file.status,
                file.base_hash.as_deref().unwrap_or("absent"),
                file.proposed_hash
            )
        })
        .collect::<Vec<_>>();
    entries.sort();
    content_hash(entries.join("\n").as_bytes())
}

pub(crate) fn verify_snapshots_at(
    root: &Path,
    files: &[ProposalFileSnapshot],
    proposed: bool,
) -> Result<()> {
    for file in files {
        let path = root.join(&file.path);
        let actual = match fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to read snapshot path {}", file.path))
            }
        };
        let expected = if proposed {
            Some(file.proposed_content.as_bytes())
        } else {
            file.base_content.as_deref().map(str::as_bytes)
        };
        if actual.as_deref() != expected {
            bail!(
                "{} no longer matches the {} snapshot",
                file.path,
                if proposed { "proposed" } else { "base" }
            );
        }
        if let Some(bytes) = actual {
            let expected_hash = if proposed {
                Some(file.proposed_hash.as_str())
            } else {
                file.base_hash.as_deref()
            };
            if expected_hash.is_some_and(|hash| content_hash(&bytes) != hash) {
                bail!("{} no longer matches its expected content hash", file.path);
            }
        }
    }
    Ok(())
}

pub(crate) fn verify_worktree_base_at(root: &Path, files: &[ProposalFileSnapshot]) -> Result<()> {
    for file in files {
        match file.status {
            ProposalFileStatus::Created if root.join(&file.path).exists() => {
                bail!(
                    "created proposal path already exists in worktree: {}",
                    file.path
                )
            }
            ProposalFileStatus::Created => {}
            ProposalFileStatus::Modified => {
                let actual = fs::read(root.join(&file.path))
                    .with_context(|| format!("failed to read worktree base {}", file.path))?;
                let base = file
                    .base_content
                    .as_deref()
                    .context("modified proposal snapshot has no base content")?;
                if actual != base.as_bytes() && !line_endings_equivalent(&actual, base.as_bytes()) {
                    bail!(
                        "{} differs from the source beyond Git line-ending checkout",
                        file.path
                    );
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn reparse_java_files(root: &Path, files: &[ProposalFileSnapshot]) -> EditStageReport {
    let started = Instant::now();
    let mut checked = 0usize;
    let mut errors = Vec::new();
    for file in files.iter().filter(|file| file.path.ends_with(".java")) {
        checked += 1;
        match fs::read_to_string(root.join(&file.path))
            .with_context(|| format!("failed to read Java file {}", file.path))
            .and_then(|source| analyze_java_source(&file.path, &source))
        {
            Ok(report) if report.syntax_valid => {}
            Ok(report) => errors.push(format!(
                "{} has {} syntax diagnostic(s)",
                file.path, report.counts.diagnostics
            )),
            Err(error) => errors.push(format!("{}: {error:#}", file.path)),
        }
    }
    let duration_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    if errors.is_empty() {
        EditStageReport::passed(
            format!("Tree-sitter parsed {checked} Java file(s)"),
            duration_ms,
        )
    } else {
        EditStageReport {
            status: crate::EditStageStatus::Failed,
            duration_ms,
            summary: format!("Tree-sitter rejected {} Java file(s)", errors.len()),
            errors,
        }
    }
}

fn line_endings_equivalent(left: &[u8], right: &[u8]) -> bool {
    let Ok(left) = std::str::from_utf8(left) else {
        return false;
    };
    let Ok(right) = std::str::from_utf8(right) else {
        return false;
    };
    normalize_line_endings(left) == normalize_line_endings(right)
}

fn normalize_line_endings(value: &str) -> String {
    value.replace("\r\n", "\n")
}
