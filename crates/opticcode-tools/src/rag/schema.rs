use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const RAG_INDEX_SCHEMA_VERSION: u32 = 2;
pub const RAG_SCAN_SCHEMA_VERSION: u32 = 1;
pub const RAG_SEARCH_SCHEMA_VERSION: u32 = 1;
pub const RAG_POLICY_VERSION: &str = "rag-safe-v1";
pub const RAG_CURRENT_FILE: &str = "CURRENT";
pub const RAG_GENERATIONS_DIR: &str = "generations";
pub const RAG_MANIFEST_FILE: &str = "manifest.json";
pub const RAG_DOCUMENTS_FILE: &str = "documents.jsonl";
pub const RAG_CHUNKS_FILE: &str = "chunks.jsonl";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RagPosition {
    pub line: u32,
    pub column: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RagExclusionRecord {
    pub collection: String,
    pub source: String,
    pub relative_path: String,
    pub entry_kind: String,
    pub rule_id: String,
    pub category: String,
    pub decision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<RagPosition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RagRuleDescriptor {
    pub rule_id: String,
    pub category: String,
    pub decision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RagSourceManifest {
    pub collection: String,
    pub profile: String,
    pub source: String,
    pub source_label: String,
    pub source_kind: String,
    pub root_fingerprint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RagIndexConfiguration {
    pub policy_version: String,
    pub chunk_chars: usize,
    pub max_file_bytes: u64,
    pub max_entries: usize,
    pub allowed_extensions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RagIndexFileDescriptor {
    pub name: String,
    pub bytes: u64,
    pub records: usize,
    pub blake3: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RagIndexMetrics {
    pub scan_us: u64,
    pub stable_read_us: u64,
    pub secret_scan_us: u64,
    pub write_us: u64,
    pub validation_us: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RagIndexManifest {
    pub schema_version: u32,
    pub generation_id: String,
    pub created_at_unix_ms: u64,
    pub opticcode_version: String,
    pub configuration_hash: String,
    pub configuration: RagIndexConfiguration,
    pub collections: Vec<RagSourceManifest>,
    pub documents: usize,
    pub chunks: usize,
    pub indexed_bytes: u64,
    pub excluded_entries: Vec<RagExclusionRecord>,
    pub exclusion_rules: Vec<RagRuleDescriptor>,
    pub index_complete: bool,
    pub documents_file: RagIndexFileDescriptor,
    pub chunks_file: RagIndexFileDescriptor,
    pub metrics: RagIndexMetrics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RagDocumentRecord {
    pub schema_version: u32,
    pub id: String,
    pub collection: String,
    pub profile: String,
    pub source: String,
    pub source_kind: String,
    pub relative_path: String,
    pub content_type: String,
    pub bytes: u64,
    pub chars: usize,
    pub blake3: String,
    pub inclusion_reason: String,
    pub allow_rule: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_modified_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RagChunkRecord {
    pub schema_version: u32,
    pub id: String,
    pub document_id: String,
    pub collection: String,
    pub profile: String,
    pub source: String,
    pub source_kind: String,
    pub relative_path: String,
    pub content_type: String,
    pub chunk_index: usize,
    pub blake3: String,
    pub inclusion_reason: String,
    pub allow_rule: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_modified_unix_ms: Option<u64>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RagSourceReport {
    pub schema_version: u32,
    pub root: PathBuf,
    pub collection: String,
    pub profile: String,
    pub source: String,
    pub source_kind: String,
    pub total_files: usize,
    pub indexable_files: usize,
    pub excluded_entries: usize,
    pub skipped_large_files: usize,
    pub indexable_bytes: u64,
    pub extensions: BTreeMap<String, usize>,
    pub important_files: Vec<PathBuf>,
    pub exclusions: Vec<RagExclusionRecord>,
    pub exclusions_truncated: bool,
    pub scan_us: u64,
    pub secret_scan_us: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RagIndexReport {
    pub schema_version: u32,
    pub output_dir: PathBuf,
    pub generation_id: String,
    pub manifest_path: PathBuf,
    pub sources: usize,
    pub documents: usize,
    pub chunks: usize,
    pub excluded_entries: usize,
    pub indexed_bytes: u64,
    pub recovered_staging_directories: usize,
    pub legacy_index_detected: bool,
    pub metrics: RagIndexMetrics,
    pub publish_us: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RagSearchHit {
    pub document_path: String,
    pub chunk_id: String,
    pub score: usize,
    pub preview: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RagSearchReport {
    pub schema_version: u32,
    pub generation_id: String,
    pub query: String,
    pub hits: Vec<RagSearchHit>,
    pub duration_us: u64,
}

impl RagSourceReport {
    pub fn to_display_string(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("RAG source: {}\n", self.root.display()));
        out.push_str(&format!("Collection: {}\n", self.collection));
        out.push_str(&format!("Profile: {}\n", self.profile));
        out.push_str(&format!("Files: {}\n", self.total_files));
        out.push_str(&format!("Indexable text files: {}\n", self.indexable_files));
        out.push_str(&format!("Indexable text bytes: {}\n", self.indexable_bytes));
        out.push_str(&format!("Excluded entries: {}\n", self.excluded_entries));
        out.push_str(&format!(
            "Skipped large text files: {}\n",
            self.skipped_large_files
        ));
        out.push_str(&format!("Secret scan: {} us\n", self.secret_scan_us));

        out.push_str("\nTop extensions:\n");
        let mut extensions = self.extensions.iter().collect::<Vec<_>>();
        extensions.sort_by(|left, right| right.1.cmp(left.1).then_with(|| left.0.cmp(right.0)));
        for (extension, count) in extensions.into_iter().take(14) {
            out.push_str(&format!("- {extension}: {count}\n"));
        }

        out.push_str("\nImportant files:\n");
        if self.important_files.is_empty() {
            out.push_str("- none\n");
        } else {
            for path in &self.important_files {
                out.push_str(&format!("- {}\n", path.display()));
            }
        }

        out.push_str("\nExclusions:\n");
        if self.exclusions.is_empty() {
            out.push_str("- none\n");
        } else {
            for exclusion in &self.exclusions {
                let position = exclusion
                    .position
                    .as_ref()
                    .map(|value| format!(" at {}:{}", value.line, value.column))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "- {} [{} / {}]{}\n",
                    exclusion.relative_path, exclusion.rule_id, exclusion.category, position
                ));
            }
            if self.exclusions_truncated {
                out.push_str("- additional exclusions omitted from display\n");
            }
        }
        out
    }
}

impl RagIndexReport {
    pub fn to_display_string(&self) -> String {
        format!(
            concat!(
                "Index: {}\n",
                "Schema: {}\n",
                "Generation: {}\n",
                "Sources: {}\n",
                "Documents: {}\n",
                "Chunks: {}\n",
                "Excluded entries: {}\n",
                "Indexed bytes: {}\n",
                "Recovered staging directories: {}\n",
                "Legacy files retained: {}\n",
                "Publish: {} us\n",
                "\nActive manifest:\n- {}\n"
            ),
            self.output_dir.display(),
            self.schema_version,
            self.generation_id,
            self.sources,
            self.documents,
            self.chunks,
            self.excluded_entries,
            self.indexed_bytes,
            self.recovered_staging_directories,
            if self.legacy_index_detected {
                "yes"
            } else {
                "no"
            },
            self.publish_us,
            self.manifest_path.display()
        )
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
