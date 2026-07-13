use std::path::PathBuf;

use serde::Serialize;

use crate::java_syntax::{JavaImport, JavaReferenceKind, SourceRange};

#[derive(Debug, Clone, Copy, Serialize)]
pub struct JavaIndexLimits {
    pub max_files: usize,
    pub max_file_bytes: u64,
    pub max_items_per_file_kind: usize,
    pub max_symbols: usize,
    pub max_references: usize,
    pub max_candidates_per_reference: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaIndexSourceSummary {
    pub syntax_schema_version: u32,
    pub discovered_files: usize,
    pub selected_files: usize,
    pub parsed_files: usize,
    pub syntax_error_files: usize,
    pub skipped_large_files: usize,
    pub skipped_non_utf8_files: usize,
    pub skipped_linked_entries: usize,
    pub walk_errors: usize,
    pub read_errors: usize,
    pub source_analysis_complete: bool,
    pub source_truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct JavaIndexCounts {
    pub packages: usize,
    pub declarations: usize,
    pub references: usize,
    pub exact: usize,
    pub unique_candidate: usize,
    pub ambiguous: usize,
    pub unresolved: usize,
    pub invalid_syntax_context: usize,
    pub candidate_lists_truncated: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct JavaIndexTruncation {
    pub source: bool,
    pub symbols: bool,
    pub references: bool,
    pub candidates: bool,
}

impl JavaIndexTruncation {
    pub fn any(&self) -> bool {
        self.source || self.symbols || self.references || self.candidates
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct JavaIndexTimings {
    pub discovery_and_read_us: u64,
    pub parse_us: u64,
    pub syntax_collection_us: u64,
    pub declaration_index_us: u64,
    pub resolution_us: u64,
    pub total_us: u64,
    pub serialization_us: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaIndexedPackage {
    pub name: String,
    pub files: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaIndexFile {
    pub path: PathBuf,
    pub source_hash: String,
    pub syntax_valid: bool,
    pub retained_items_truncated: bool,
    pub package: Option<String>,
    pub imports: Vec<JavaImport>,
    pub diagnostic_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JavaIndexedSymbolKind {
    Class,
    Interface,
    Enum,
    AnnotationType,
    Record,
    Method,
    Constructor,
    Field,
    EnumConstant,
}

impl JavaIndexedSymbolKind {
    pub fn is_type(self) -> bool {
        matches!(
            self,
            Self::Class | Self::Interface | Self::Enum | Self::AnnotationType | Self::Record
        )
    }

    pub fn is_callable(self) -> bool {
        matches!(self, Self::Method | Self::Constructor)
    }

    pub fn is_field(self) -> bool {
        matches!(self, Self::Field | Self::EnumConstant)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JavaVisibility {
    Public,
    Protected,
    Private,
    Default,
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaIndexedSymbol {
    pub id: String,
    pub kind: JavaIndexedSymbolKind,
    pub name: String,
    pub qualified_name: String,
    pub owner_id: Option<String>,
    pub signature: Option<String>,
    pub signature_complete: bool,
    pub parameter_count: Option<usize>,
    pub visibility: JavaVisibility,
    pub is_static: bool,
    pub annotations: Vec<String>,
    pub file: PathBuf,
    pub source_hash: String,
    pub range: SourceRange,
    pub name_range: SourceRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JavaResolutionStatus {
    Exact,
    UniqueCandidate,
    Ambiguous,
    Unresolved,
    InvalidSyntaxContext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JavaCandidateOrigin {
    LocalOrNested,
    SamePackage,
    ExplicitImport,
    StaticExplicitImport,
    WildcardImport,
    StaticWildcardImport,
    JavaLang,
    FullyQualified,
    OwnerMember,
    GlobalIndex,
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaResolutionCandidate {
    pub symbol_id: String,
    pub origin: JavaCandidateOrigin,
    pub external: bool,
    pub file: Option<PathBuf>,
    pub range: Option<SourceRange>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaReferenceResolution {
    pub status: JavaResolutionStatus,
    pub target_id: Option<String>,
    pub reason: String,
    pub candidates: Vec<JavaResolutionCandidate>,
    pub candidates_truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaIndexedReference {
    pub id: String,
    pub kind: JavaReferenceKind,
    pub name: String,
    pub qualifier: Option<String>,
    pub owner_id: Option<String>,
    pub argument_count: Option<usize>,
    pub file: PathBuf,
    pub source_hash: String,
    pub range: SourceRange,
    pub name_range: SourceRange,
    pub resolution: JavaReferenceResolution,
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaIndexProjectReport {
    pub schema_version: u32,
    pub operation: &'static str,
    pub root: PathBuf,
    pub input: PathBuf,
    pub limits: JavaIndexLimits,
    pub source: JavaIndexSourceSummary,
    pub analysis_complete: bool,
    pub truncated: bool,
    pub truncation: JavaIndexTruncation,
    pub counts: JavaIndexCounts,
    pub timings: JavaIndexTimings,
    pub packages: Vec<JavaIndexedPackage>,
    pub files: Vec<JavaIndexFile>,
    pub symbols: Vec<JavaIndexedSymbol>,
    pub references: Vec<JavaIndexedReference>,
    pub warnings: Vec<String>,
}

impl JavaIndexProjectReport {
    pub fn to_display_string(&self) -> String {
        format!(
            concat!(
                "Java cross-file index (read-only):\n",
                "- root: {}\n",
                "- files: {}/{} parsed\n",
                "- declarations: {}\n",
                "- references: {}\n",
                "- exact: {}\n",
                "- unique candidates: {}\n",
                "- ambiguous: {}\n",
                "- unresolved: {}\n",
                "- invalid syntax context: {}\n",
                "- analysis complete: {}\n",
                "- truncated: {}\n",
                "- duration: {:.3} ms\n",
                "  - discovery/read: {:.3} ms\n",
                "  - parse: {:.3} ms\n",
                "  - syntax collection: {:.3} ms\n",
                "  - declaration index: {:.3} ms\n",
                "  - resolution: {:.3} ms"
            ),
            self.root.display(),
            self.source.parsed_files,
            self.source.discovered_files,
            self.counts.declarations,
            self.counts.references,
            self.counts.exact,
            self.counts.unique_candidate,
            self.counts.ambiguous,
            self.counts.unresolved,
            self.counts.invalid_syntax_context,
            self.analysis_complete,
            self.truncated,
            self.timings.total_us as f64 / 1_000.0,
            self.timings.discovery_and_read_us as f64 / 1_000.0,
            self.timings.parse_us as f64 / 1_000.0,
            self.timings.syntax_collection_us as f64 / 1_000.0,
            self.timings.declaration_index_us as f64 / 1_000.0,
            self.timings.resolution_us as f64 / 1_000.0
        )
    }
}
