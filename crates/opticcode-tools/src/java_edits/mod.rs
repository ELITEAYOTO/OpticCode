//! Read-only, AST-ranged Java edit proposals with fail-closed validation.

pub(crate) mod legacy;
mod schema;
mod validation;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Result};

use crate::java_index::{
    analyze_java_index, JavaIndexFile, JavaIndexOptions, JavaIndexedReference, JavaResolutionStatus,
};
use crate::java_syntax::SourceRange;
use legacy::{is_exact_rule_target, qualifier_is_proven, rule_for_reference, LegacyJavaRule};
pub use schema::{
    JavaEditConfidence, JavaEditCounts, JavaEditFileValidation, JavaEditLimits, JavaEditProposal,
    JavaEditProposalReport, JavaEditRejection, JavaEditRejectionKind, JavaEditTimings,
};
use validation::{
    read_source_snapshot, validate_file_edits, AstSafetyScanner, SnapshotError, SourceSnapshot,
};

pub const JAVA_EDIT_SCHEMA_VERSION: u32 = 1;
pub const JAVA_EDIT_RULE_SET: &str = "minecraft_java_1_8_v1";
pub const DEFAULT_JAVA_EDIT_PROPOSAL_LIMIT: usize = 10_000;
pub const MAX_JAVA_EDIT_PROPOSAL_LIMIT: usize = 100_000;
pub const MAX_JAVA_EDIT_REJECTIONS: usize = 256;

#[derive(Debug, Clone, Copy)]
pub struct JavaEditOptions {
    pub index: JavaIndexOptions,
    pub max_proposals: usize,
}

impl Default for JavaEditOptions {
    fn default() -> Self {
        Self {
            index: JavaIndexOptions::default(),
            max_proposals: DEFAULT_JAVA_EDIT_PROPOSAL_LIMIT,
        }
    }
}

pub fn propose_java_edits(
    input: &Path,
    options: JavaEditOptions,
) -> Result<JavaEditProposalReport> {
    validate_options(options)?;
    let started_at = Instant::now();
    let index_started = Instant::now();
    let index = analyze_java_index(input, options.index)?;
    let index_us = duration_us(index_started.elapsed());
    let mut scanner = AstSafetyScanner::new()?;
    let files = index
        .files
        .iter()
        .map(|file| (file.path.clone(), file))
        .collect::<BTreeMap<_, _>>();
    let mut snapshots = BTreeMap::<PathBuf, SourceSnapshot>::new();
    let mut snapshot_failures = BTreeMap::<PathBuf, SnapshotError>::new();
    let mut candidate_proposals = Vec::new();
    let mut rejections = Vec::new();
    let mut counts = JavaEditCounts {
        references_examined: index.references.len(),
        ..JavaEditCounts::default()
    };
    let mut rejections_truncated = false;
    let mut proposals_truncated = false;
    let mut blocking_rejection = false;
    let mut source_validation_us = 0u64;

    for reference in &index.references {
        let Some(rule) = rule_for_reference(reference) else {
            continue;
        };
        counts.legacy_candidates += 1;
        let Some(file) = files.get(&reference.file).copied() else {
            push_rejection(
                JavaEditRejection {
                    kind: JavaEditRejectionKind::InvalidSource,
                    file: reference.file.clone(),
                    reference_id: Some(reference.id.clone()),
                    rule_id: Some(rule.id),
                    message: "indexed reference has no matching file record".to_string(),
                },
                &mut counts,
                &mut rejections,
                &mut rejections_truncated,
                &mut blocking_rejection,
            );
            continue;
        };
        if reference.source_hash != file.source_hash {
            push_rejection(
                rejection_for_reference(
                    JavaEditRejectionKind::InvalidSource,
                    reference,
                    rule,
                    "reference and file source hashes differ inside the Java index".to_string(),
                ),
                &mut counts,
                &mut rejections,
                &mut rejections_truncated,
                &mut blocking_rejection,
            );
            continue;
        }
        if reference.resolution.status != JavaResolutionStatus::Exact {
            push_rejection(
                rejection_for_reference(
                    JavaEditRejectionKind::NonExactResolution,
                    reference,
                    rule,
                    format!(
                        "legacy-looking reference remains {:?}: {}",
                        reference.resolution.status, reference.resolution.reason
                    ),
                ),
                &mut counts,
                &mut rejections,
                &mut rejections_truncated,
                &mut blocking_rejection,
            );
            continue;
        }
        if !is_exact_rule_target(rule, reference) {
            push_rejection(
                rejection_for_reference(
                    JavaEditRejectionKind::WrongTarget,
                    reference,
                    rule,
                    format!(
                        "exact reference targets {:?}, not {}",
                        reference.resolution.target_id,
                        rule.target_id()
                    ),
                ),
                &mut counts,
                &mut rejections,
                &mut rejections_truncated,
                &mut blocking_rejection,
            );
            continue;
        }
        counts.exact_target_matches += 1;
        if !qualifier_is_proven(rule, reference, file) {
            push_rejection(
                rejection_for_reference(
                    JavaEditRejectionKind::UnprovenQualifier,
                    reference,
                    rule,
                    format!(
                        "qualifier {:?} is neither {} nor an explicitly imported {}",
                        reference.qualifier,
                        rule.owner,
                        rule.owner_simple_name()
                    ),
                ),
                &mut counts,
                &mut rejections,
                &mut rejections_truncated,
                &mut blocking_rejection,
            );
            continue;
        }

        if !snapshots.contains_key(&reference.file)
            && !snapshot_failures.contains_key(&reference.file)
        {
            let validation_started = Instant::now();
            let loaded = read_source_snapshot(
                &index.root,
                &reference.file,
                &file.source_hash,
                options.index.syntax.max_file_bytes,
                &mut scanner,
            );
            source_validation_us =
                source_validation_us.saturating_add(duration_us(validation_started.elapsed()));
            match loaded {
                Ok(snapshot) => {
                    snapshots.insert(reference.file.clone(), snapshot);
                }
                Err(error) => {
                    snapshot_failures.insert(reference.file.clone(), error);
                }
            }
        }
        if let Some(error) = snapshot_failures.get(&reference.file) {
            let (kind, message) = match error {
                SnapshotError::Invalid(message) => {
                    (JavaEditRejectionKind::InvalidSource, message.clone())
                }
                SnapshotError::Changed(message) => {
                    (JavaEditRejectionKind::SourceChanged, message.clone())
                }
            };
            push_rejection(
                rejection_for_reference(kind, reference, rule, message),
                &mut counts,
                &mut rejections,
                &mut rejections_truncated,
                &mut blocking_rejection,
            );
            continue;
        }
        let Some(snapshot) = snapshots.get(&reference.file) else {
            unreachable!("snapshot was loaded or its error was retained")
        };
        if let Some(shadow) = qualifier_shadow(reference, file, snapshot) {
            push_rejection(
                rejection_for_reference(
                    JavaEditRejectionKind::ShadowedQualifier,
                    reference,
                    rule,
                    format!(
                        "qualifier root {shadow:?} may resolve to a value or static import in this file"
                    ),
                ),
                &mut counts,
                &mut rejections,
                &mut rejections_truncated,
                &mut blocking_rejection,
            );
            continue;
        }

        if candidate_proposals.len() >= options.max_proposals {
            if !proposals_truncated {
                proposals_truncated = true;
                push_rejection(
                    rejection_for_reference(
                        JavaEditRejectionKind::ProposalLimit,
                        reference,
                        rule,
                        format!(
                            "Java edit proposal limit reached at {} entries",
                            options.max_proposals
                        ),
                    ),
                    &mut counts,
                    &mut rejections,
                    &mut rejections_truncated,
                    &mut blocking_rejection,
                );
            }
            continue;
        }

        match build_proposal(rule, reference, snapshot) {
            Ok(proposal) => candidate_proposals.push(proposal),
            Err(message) => push_rejection(
                rejection_for_reference(
                    JavaEditRejectionKind::InvalidRange,
                    reference,
                    rule,
                    message,
                ),
                &mut counts,
                &mut rejections,
                &mut rejections_truncated,
                &mut blocking_rejection,
            ),
        }
    }

    candidate_proposals.sort_by(|left, right| {
        normalized_path(&left.file)
            .cmp(&normalized_path(&right.file))
            .then_with(|| left.edit_range.start.byte.cmp(&right.edit_range.start.byte))
            .then_with(|| left.id.cmp(&right.id))
    });
    let mut grouped = BTreeMap::<PathBuf, Vec<JavaEditProposal>>::new();
    for proposal in candidate_proposals {
        grouped
            .entry(proposal.file.clone())
            .or_default()
            .push(proposal);
    }

    let mut proposals = Vec::new();
    let mut file_validations = Vec::new();
    let mut reparse_us = 0u64;
    for (path, file_proposals) in grouped {
        let Some(snapshot) = snapshots.get(&path) else {
            continue;
        };
        let syntax_valid_before = files.get(&path).is_some_and(|file| file.syntax_valid);
        let validation = validate_file_edits(snapshot, &file_proposals, syntax_valid_before);
        reparse_us = reparse_us.saturating_add(validation.reparse_us);
        let accepted = validation.rejections.is_empty()
            && validation
                .validation
                .as_ref()
                .is_some_and(|result| result.syntax_valid_before && result.syntax_valid_after);
        if let Some(file_validation) = validation.validation {
            file_validations.push(file_validation);
        }
        for rejection in validation.rejections {
            push_rejection(
                rejection,
                &mut counts,
                &mut rejections,
                &mut rejections_truncated,
                &mut blocking_rejection,
            );
        }
        if accepted {
            proposals.extend(file_proposals);
        }
    }

    proposals.sort_by(|left, right| {
        normalized_path(&left.file)
            .cmp(&normalized_path(&right.file))
            .then_with(|| left.edit_range.start.byte.cmp(&right.edit_range.start.byte))
            .then_with(|| left.id.cmp(&right.id))
    });
    file_validations.sort_by_key(|validation| normalized_path(&validation.path));
    rejections.sort_by(|left, right| {
        normalized_path(&left.file)
            .cmp(&normalized_path(&right.file))
            .then_with(|| left.reference_id.cmp(&right.reference_id))
            .then_with(|| left.message.cmp(&right.message))
    });

    counts.proposals = proposals.len();
    counts.files_with_proposals = file_validations
        .iter()
        .filter(|validation| validation.syntax_valid_before && validation.syntax_valid_after)
        .count();
    let analysis_complete = index.analysis_complete && !blocking_rejection && !proposals_truncated;
    let safe_to_apply = analysis_complete;
    let truncated = index.truncated || proposals_truncated || rejections_truncated;
    let mut warnings = index.warnings.clone();
    if !index.analysis_complete {
        warnings.push(
            "Java edit analysis is fail-closed because the underlying index is incomplete"
                .to_string(),
        );
    }
    if proposals_truncated {
        warnings.push(format!(
            "Java edit proposal limit reached: retained at most {} proposals",
            options.max_proposals
        ));
    }
    if rejections_truncated {
        warnings.push(format!(
            "Java edit rejection details were limited to {} entries; counts remain complete",
            MAX_JAVA_EDIT_REJECTIONS
        ));
    }

    let total_us = duration_us(started_at.elapsed());
    let proposal_us = total_us
        .saturating_sub(index_us)
        .saturating_sub(source_validation_us)
        .saturating_sub(reparse_us);
    Ok(JavaEditProposalReport {
        schema_version: JAVA_EDIT_SCHEMA_VERSION,
        operation: "java_edit_proposals",
        rule_set: JAVA_EDIT_RULE_SET,
        root: index.root,
        input: index.input,
        index_schema_version: index.schema_version,
        index_source: index.source,
        index_truncation: index.truncation,
        index_counts: index.counts,
        limits: JavaEditLimits {
            index: index.limits,
            max_proposals: options.max_proposals,
            max_rejections: MAX_JAVA_EDIT_REJECTIONS,
        },
        index_analysis_complete: index.analysis_complete,
        analysis_complete,
        safe_to_apply,
        truncated,
        proposals_truncated,
        rejections_truncated,
        counts,
        timings: JavaEditTimings {
            index_us,
            source_validation_us,
            proposal_us,
            reparse_us,
            total_us,
            serialization_us: None,
        },
        proposals,
        file_validations,
        rejections,
        warnings,
    })
}

fn build_proposal(
    rule: LegacyJavaRule,
    reference: &JavaIndexedReference,
    snapshot: &SourceSnapshot,
) -> Result<JavaEditProposal, String> {
    let expected_node_content = slice_range(&snapshot.source, reference.range)
        .ok_or_else(|| "reference AST node range is invalid or not UTF-8 aligned".to_string())?;
    let expected_content = slice_range(&snapshot.source, reference.name_range)
        .ok_or_else(|| "reference member range is invalid or not UTF-8 aligned".to_string())?;
    if expected_content != rule.modern_member() || expected_content != reference.name {
        return Err(format!(
            "reference member bytes {:?} do not match rule member {:?}",
            expected_content,
            rule.modern_member()
        ));
    }
    if reference.name_range.start.byte < reference.range.start.byte
        || reference.name_range.end.byte > reference.range.end.byte
    {
        return Err("reference member range is outside its AST node range".to_string());
    }
    let target_id = reference
        .resolution
        .target_id
        .clone()
        .ok_or_else(|| "exact reference has no target id".to_string())?;
    let id_material = format!(
        "{}\0{}\0{}\0{}\0{}\0{}",
        rule.id,
        normalized_path(&reference.file),
        snapshot.source_hash,
        reference.name_range.start.byte,
        reference.name_range.end.byte,
        rule.legacy_member(),
    );
    let id = format!("java-edit:{}", blake3::hash(id_material.as_bytes()));

    Ok(JavaEditProposal {
        id,
        rule_id: rule.id,
        file: reference.file.clone(),
        source_hash: snapshot.source_hash.clone(),
        reference_id: reference.id.clone(),
        target_id,
        reference_kind: reference.kind,
        node_range: reference.range,
        edit_range: reference.name_range,
        expected_node_content: expected_node_content.to_string(),
        expected_content: expected_content.to_string(),
        replacement: rule.legacy_member(),
        reason: rule.reason,
        resolution_reason: reference.resolution.reason.clone(),
        confidence: JavaEditConfidence::SyntaxExact,
    })
}

fn qualifier_shadow(
    reference: &JavaIndexedReference,
    file: &JavaIndexFile,
    snapshot: &SourceSnapshot,
) -> Option<String> {
    let qualifier = reference.qualifier.as_deref()?;
    let root = qualifier.split('.').next().unwrap_or(qualifier);
    let static_import_conflict = file.imports.iter().any(|import| {
        if !import.is_static {
            return false;
        }
        import.wildcard
            || import
                .path
                .rsplit('.')
                .next()
                .is_some_and(|name| name == root)
    });
    (snapshot.declared_value_names.contains(root) || static_import_conflict)
        .then(|| root.to_string())
}

fn slice_range(source: &str, range: SourceRange) -> Option<&str> {
    (range.start.byte < range.end.byte
        && range.end.byte <= source.len()
        && source.is_char_boundary(range.start.byte)
        && source.is_char_boundary(range.end.byte))
    .then(|| &source[range.start.byte..range.end.byte])
}

fn rejection_for_reference(
    kind: JavaEditRejectionKind,
    reference: &JavaIndexedReference,
    rule: LegacyJavaRule,
    message: String,
) -> JavaEditRejection {
    JavaEditRejection {
        kind,
        file: reference.file.clone(),
        reference_id: Some(reference.id.clone()),
        rule_id: Some(rule.id),
        message,
    }
}

fn push_rejection(
    rejection: JavaEditRejection,
    counts: &mut JavaEditCounts,
    rejections: &mut Vec<JavaEditRejection>,
    rejections_truncated: &mut bool,
    blocking_rejection: &mut bool,
) {
    counts.rejections += 1;
    match rejection.kind {
        JavaEditRejectionKind::NonExactResolution => counts.rejected_non_exact += 1,
        JavaEditRejectionKind::WrongTarget => counts.rejected_wrong_target += 1,
        JavaEditRejectionKind::UnprovenQualifier => counts.rejected_unproven_qualifier += 1,
        JavaEditRejectionKind::ShadowedQualifier => counts.rejected_shadowed_qualifier += 1,
        JavaEditRejectionKind::InvalidSource => counts.rejected_invalid_source += 1,
        JavaEditRejectionKind::SourceChanged => counts.rejected_source_changed += 1,
        JavaEditRejectionKind::InvalidRange => counts.rejected_invalid_range += 1,
        JavaEditRejectionKind::OverlappingRange => counts.overlap_conflicts += 1,
        JavaEditRejectionKind::PostEditSyntaxInvalid => counts.post_edit_syntax_failures += 1,
        JavaEditRejectionKind::ProposalLimit => {}
    }
    *blocking_rejection |= rejection.kind.blocks_application();
    if rejections.len() < MAX_JAVA_EDIT_REJECTIONS {
        rejections.push(rejection);
    } else {
        *rejections_truncated = true;
    }
}

fn validate_options(options: JavaEditOptions) -> Result<()> {
    if options.max_proposals == 0 || options.max_proposals > MAX_JAVA_EDIT_PROPOSAL_LIMIT {
        bail!(
            "Java edit proposal limit must be between 1 and {}",
            MAX_JAVA_EDIT_PROPOSAL_LIMIT
        );
    }
    Ok(())
}

fn normalized_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn duration_us(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        normalized_path, propose_java_edits, JavaEditConfidence, JavaEditOptions,
        JavaEditRejectionKind,
    };

    #[test]
    fn proposes_all_legacy_rules_and_rejects_false_positives_without_writes() {
        let root = corpus_root();
        let before = snapshot_java_files(&root);
        let report = propose_java_edits(&root, JavaEditOptions::default())
            .expect("legacy edit corpus should analyze");
        let after = snapshot_java_files(&root);

        assert_eq!(before, after, "read-only analysis modified its corpus");
        assert_eq!(report.schema_version, 1);
        assert_eq!(report.operation, "java_edit_proposals");
        assert_eq!(report.rule_set, "minecraft_java_1_8_v1");
        assert!(report.index_analysis_complete);
        assert!(report.analysis_complete);
        assert!(report.safe_to_apply);
        assert!(!report.truncated);
        assert_eq!(report.counts.references_examined, 85);
        assert_eq!(report.counts.legacy_candidates, 26);
        assert_eq!(report.counts.exact_target_matches, 18);
        assert_eq!(report.counts.proposals, 16);
        assert_eq!(report.counts.files_with_proposals, 3);
        assert_eq!(report.counts.rejections, 10);
        assert_eq!(report.counts.rejected_non_exact, 1);
        assert_eq!(report.counts.rejected_wrong_target, 7);
        assert_eq!(report.counts.rejected_shadowed_qualifier, 2);
        assert_eq!(report.counts.rejected_invalid_range, 0);
        assert_eq!(report.counts.overlap_conflicts, 0);
        assert_eq!(report.counts.post_edit_syntax_failures, 0);

        let rule_ids = report
            .proposals
            .iter()
            .map(|proposal| proposal.rule_id)
            .collect::<BTreeSet<_>>();
        assert_eq!(rule_ids.len(), 14);
        assert!(rule_ids.contains("MC18-MATERIAL-001"));
        assert!(rule_ids.contains("MC18-ENTITY-003"));
        assert!(report.proposals.iter().all(|proposal| {
            proposal.confidence == JavaEditConfidence::SyntaxExact
                && proposal.target_id.starts_with("org.bukkit.")
                && proposal.edit_range.start.byte < proposal.edit_range.end.byte
                && proposal.node_range.start.byte <= proposal.edit_range.start.byte
                && proposal.node_range.end.byte >= proposal.edit_range.end.byte
                && proposal.source_hash.starts_with("blake3:")
                && proposal.id.starts_with("java-edit:")
                && !normalized_path(&proposal.file).contains("/negative/")
        }));
        assert_eq!(report.file_validations.len(), 3);
        assert!(report.file_validations.iter().all(|validation| {
            validation.syntax_valid_before
                && validation.syntax_valid_after
                && validation.diagnostics_after == 0
                && validation.source_hash != validation.proposed_hash
        }));
        assert!(report
            .rejections
            .iter()
            .any(|rejection| rejection.kind == JavaEditRejectionKind::NonExactResolution));
        assert!(report
            .rejections
            .iter()
            .any(|rejection| rejection.kind == JavaEditRejectionKind::WrongTarget));
        assert_eq!(
            report
                .rejections
                .iter()
                .filter(|rejection| { rejection.kind == JavaEditRejectionKind::ShadowedQualifier })
                .count(),
            2
        );
    }

    #[test]
    fn proposal_output_is_deterministic_and_limits_fail_closed() {
        let root = corpus_root();
        let first = propose_java_edits(&root, JavaEditOptions::default()).expect("first report");
        let second = propose_java_edits(&root, JavaEditOptions::default()).expect("second report");
        let mut first_json = serde_json::to_value(first).expect("first JSON");
        let mut second_json = serde_json::to_value(second).expect("second JSON");
        first_json
            .as_object_mut()
            .expect("report object")
            .remove("timings");
        second_json
            .as_object_mut()
            .expect("report object")
            .remove("timings");
        assert_eq!(first_json, second_json);

        let bounded = propose_java_edits(
            &root,
            JavaEditOptions {
                max_proposals: 3,
                ..JavaEditOptions::default()
            },
        )
        .expect("bounded report");
        assert_eq!(bounded.proposals.len(), 3);
        assert!(bounded.proposals_truncated);
        assert!(bounded.truncated);
        assert!(!bounded.analysis_complete);
        assert!(!bounded.safe_to_apply);
        assert!(bounded
            .rejections
            .iter()
            .any(|rejection| rejection.kind == JavaEditRejectionKind::ProposalLimit));
    }

    #[test]
    fn invalid_java_context_is_reported_fail_closed() {
        let fixture = TemporaryJavaFixture::new();
        fs::write(
            fixture.root.join("Broken.java"),
            concat!(
                "import org.bukkit.Material;\n",
                "class Broken { Object value = Material.GUNPOWDER; void run( { }\n",
            ),
        )
        .expect("broken source");

        let report = propose_java_edits(&fixture.root, JavaEditOptions::default())
            .expect("invalid syntax should remain reportable");
        assert!(!report.index_analysis_complete);
        assert!(!report.analysis_complete);
        assert!(!report.safe_to_apply);
        assert!(report.proposals.is_empty());
        assert!(report.counts.rejected_non_exact > 0);
    }

    #[test]
    fn unicode_crlf_source_keeps_exact_byte_ranges_and_remains_unchanged() {
        let fixture = TemporaryJavaFixture::new();
        let path = fixture.root.join("Caf\u{00e9}.java");
        let source = concat!(
            "import org.bukkit.Material;\r\n",
            "class Caf\u{00e9} { String label = \"\u{00e9}t\u{00e9}\"; Object value = Material.GUNPOWDER; }\r\n",
        );
        fs::write(&path, source.as_bytes()).expect("unicode CRLF source");

        let report = propose_java_edits(&fixture.root, JavaEditOptions::default())
            .expect("unicode CRLF source should analyze");
        assert!(report.safe_to_apply);
        assert_eq!(report.proposals.len(), 1);
        let proposal = &report.proposals[0];
        assert_eq!(proposal.expected_content, "GUNPOWDER");
        assert_eq!(
            &source[proposal.edit_range.start.byte..proposal.edit_range.end.byte],
            "GUNPOWDER"
        );
        assert_eq!(
            &source[proposal.node_range.start.byte..proposal.node_range.end.byte],
            "Material.GUNPOWDER"
        );
        assert_eq!(
            fs::read(path).expect("source after analysis"),
            source.as_bytes()
        );
    }

    fn corpus_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/java-edits-legacy")
    }

    fn snapshot_java_files(root: &Path) -> BTreeMap<String, Vec<u8>> {
        fn visit(root: &Path, current: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
            for entry in fs::read_dir(current).expect("read corpus directory") {
                let entry = entry.expect("read corpus entry");
                let path = entry.path();
                if path.is_dir() {
                    visit(root, &path, files);
                } else if path.extension().and_then(|value| value.to_str()) == Some("java") {
                    files.insert(
                        normalized_path(path.strip_prefix(root).expect("relative corpus path")),
                        fs::read(path).expect("read corpus source"),
                    );
                }
            }
        }

        let mut files = BTreeMap::new();
        visit(root, root, &mut files);
        files
    }

    struct TemporaryJavaFixture {
        root: PathBuf,
    }

    impl TemporaryJavaFixture {
        fn new() -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time after epoch")
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "opticcode-java-edits-{}-{stamp}",
                std::process::id()
            ));
            fs::create_dir_all(&root).expect("create fixture");
            Self { root }
        }
    }

    impl Drop for TemporaryJavaFixture {
        fn drop(&mut self) {
            if self.root.starts_with(std::env::temp_dir()) {
                let _ = fs::remove_dir_all(&self.root);
            }
        }
    }
}
