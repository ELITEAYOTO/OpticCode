mod index;
mod policy;
mod reference;
mod schema;
mod secrets;

pub use index::{
    build_rag_index, inspect_rag_source, load_active_rag_manifest, search_rag_index,
    search_rag_index_queries, search_rag_index_report,
};
pub use reference::{
    inspect_sensitive_text, read_safe_workspace_file, SafeReferenceError, SafeTextSecret,
    SafeWorkspaceFile, MAX_SAFE_REFERENCE_FILE_BYTES,
};
pub use schema::{
    RagIndexManifest, RagIndexReport, RagSearchHit, RagSearchReport, RagSourceReport,
    RAG_INDEX_SCHEMA_VERSION, RAG_POLICY_VERSION,
};

#[cfg(test)]
mod tests;
