use std::path::PathBuf;

use serde::Serialize;

use crate::java_index::{
    JavaIndexCounts, JavaIndexLimits, JavaIndexSourceSummary, JavaIndexTruncation,
};
use crate::java_syntax::{JavaReferenceKind, SourceRange};

#[derive(Debug, Clone, Copy, Serialize)]
pub struct JavaEditLimits {
    pub index: JavaIndexLimits,
    pub max_proposals: usize,
    pub max_rejections: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct JavaEditCounts {
    pub references_examined: usize,
    pub legacy_candidates: usize,
    pub exact_target_matches: usize,
    pub proposals: usize,
    pub files_with_proposals: usize,
    pub rejections: usize,
    pub rejected_non_exact: usize,
    pub rejected_wrong_target: usize,
    pub rejected_unproven_qualifier: usize,
    pub rejected_shadowed_qualifier: usize,
    pub rejected_invalid_source: usize,
    pub rejected_source_changed: usize,
    pub rejected_invalid_range: usize,
    pub overlap_conflicts: usize,
    pub post_edit_syntax_failures: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct JavaEditTimings {
    pub index_us: u64,
    pub source_validation_us: u64,
    pub proposal_us: u64,
    pub reparse_us: u64,
    pub total_us: u64,
    pub serialization_us: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JavaEditConfidence {
    SyntaxExact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JavaEditRejectionKind {
    NonExactResolution,
    WrongTarget,
    UnprovenQualifier,
    ShadowedQualifier,
    InvalidSource,
    SourceChanged,
    InvalidRange,
    OverlappingRange,
    PostEditSyntaxInvalid,
    ProposalLimit,
}

impl JavaEditRejectionKind {
    pub(crate) fn blocks_application(self) -> bool {
        matches!(
            self,
            Self::InvalidSource
                | Self::SourceChanged
                | Self::InvalidRange
                | Self::OverlappingRange
                | Self::PostEditSyntaxInvalid
                | Self::ProposalLimit
        )
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaEditProposal {
    pub id: String,
    pub rule_id: &'static str,
    pub file: PathBuf,
    pub source_hash: String,
    pub reference_id: String,
    pub target_id: String,
    pub reference_kind: JavaReferenceKind,
    pub node_range: SourceRange,
    pub edit_range: SourceRange,
    pub expected_node_content: String,
    pub expected_content: String,
    pub replacement: &'static str,
    pub reason: &'static str,
    pub resolution_reason: String,
    pub confidence: JavaEditConfidence,
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaEditFileValidation {
    pub path: PathBuf,
    pub source_hash: String,
    pub proposed_hash: String,
    pub source_bytes: usize,
    pub proposed_bytes: usize,
    pub edit_count: usize,
    pub proposal_ids: Vec<String>,
    pub syntax_valid_before: bool,
    pub syntax_valid_after: bool,
    pub diagnostics_after: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaEditRejection {
    pub kind: JavaEditRejectionKind,
    pub file: PathBuf,
    pub reference_id: Option<String>,
    pub rule_id: Option<&'static str>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaEditProposalReport {
    pub schema_version: u32,
    pub operation: &'static str,
    pub rule_set: &'static str,
    pub root: PathBuf,
    pub input: PathBuf,
    pub index_schema_version: u32,
    pub index_source: JavaIndexSourceSummary,
    pub index_truncation: JavaIndexTruncation,
    pub index_counts: JavaIndexCounts,
    pub limits: JavaEditLimits,
    pub index_analysis_complete: bool,
    pub analysis_complete: bool,
    pub safe_to_apply: bool,
    pub truncated: bool,
    pub proposals_truncated: bool,
    pub rejections_truncated: bool,
    pub counts: JavaEditCounts,
    pub timings: JavaEditTimings,
    pub proposals: Vec<JavaEditProposal>,
    pub file_validations: Vec<JavaEditFileValidation>,
    pub rejections: Vec<JavaEditRejection>,
    pub warnings: Vec<String>,
}

impl JavaEditProposalReport {
    pub fn to_display_string(&self) -> String {
        let mut output = format!(
            concat!(
                "Java edit proposals (read-only):\n",
                "- root: {}\n",
                "- rule set: {}\n",
                "- Java files: {}/{} parsed\n",
                "- references examined: {}\n",
                "- legacy candidates: {}\n",
                "- exact target matches: {}\n",
                "- proposals: {} in {} files\n",
                "- rejections: {}\n",
                "- analysis complete: {}\n",
                "- safe to apply downstream: {}\n",
                "- truncated: {}\n",
                "- duration: {:.3} ms\n"
            ),
            self.root.display(),
            self.rule_set,
            self.index_source.parsed_files,
            self.index_source.discovered_files,
            self.counts.references_examined,
            self.counts.legacy_candidates,
            self.counts.exact_target_matches,
            self.counts.proposals,
            self.counts.files_with_proposals,
            self.counts.rejections,
            self.analysis_complete,
            self.safe_to_apply,
            self.truncated,
            self.timings.total_us as f64 / 1_000.0,
        );

        for proposal in &self.proposals {
            output.push_str(&format!(
                "- {}:{}:{} {} -> {} [{}]\n",
                proposal.file.display(),
                proposal.edit_range.start.row + 1,
                proposal.edit_range.start.column + 1,
                proposal.expected_content,
                proposal.replacement,
                proposal.rule_id,
            ));
        }

        if self.proposals.is_empty() {
            output.push_str("- no eligible edit was proposed\n");
        }
        for warning in &self.warnings {
            output.push_str(&format!("Warning: {warning}\n"));
        }
        output
    }
}
