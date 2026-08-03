use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};

use super::policy::{
    collect_inventory, configuration_allowed_extensions, ensure_unique_paths, exclusion_rules,
    metadata_is_link_or_reparse, prepare_sources, read_stable_candidate,
    record_matches_allow_policy, DEFAULT_RAG_MAX_FILE_BYTES, MAX_RAG_ENTRIES, MAX_RAG_EXCLUSIONS,
};
use super::schema::{
    RagChunkRecord, RagDocumentRecord, RagExclusionRecord, RagIndexConfiguration,
    RagIndexFileDescriptor, RagIndexManifest, RagIndexMetrics, RagIndexReport, RagSearchHit,
    RagSearchReport, RagSourceReport, RAG_CHUNKS_FILE, RAG_CURRENT_FILE, RAG_DOCUMENTS_FILE,
    RAG_GENERATIONS_DIR, RAG_INDEX_SCHEMA_VERSION, RAG_MANIFEST_FILE, RAG_POLICY_VERSION,
    RAG_SCAN_SCHEMA_VERSION, RAG_SEARCH_SCHEMA_VERSION,
};
use super::secrets::detect_secret;

const MIN_CHUNK_CHARS: usize = 512;
const MAX_CHUNK_CHARS: usize = 256 * 1024;
const STAGING_PREFIX: &str = ".staging-";
const CURRENT_TEMP_PREFIX: &str = ".CURRENT-";
const CURRENT_TEMP_SUFFIX: &str = ".tmp";
static GENERATION_COUNTER: AtomicU64 = AtomicU64::new(0);

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RagFaultPoint {
    DuringScan,
    DuringDocumentsWrite,
    DuringChunksWrite,
    BeforeManifestFinalization,
    DuringPublication,
}

#[derive(Debug)]
struct ActiveGeneration {
    generation_dir: PathBuf,
    manifest: RagIndexManifest,
}

#[derive(Debug)]
struct ValidatedData {
    documents_file: RagIndexFileDescriptor,
    chunks_file: RagIndexFileDescriptor,
    indexed_bytes: u64,
}

#[derive(Debug)]
struct PreparedSearch {
    query: String,
    query_lower: String,
    terms: Vec<String>,
    hits: Vec<RagSearchHit>,
}

pub fn inspect_rag_source(root: &Path, limit: usize) -> Result<RagSourceReport> {
    let source = prepare_sources(&[root.to_path_buf()])?
        .into_iter()
        .next()
        .context("RAG source preparation returned no source")?;
    let inventory = collect_inventory(&source)?;
    let mut exclusions = inventory.exclusions;
    let mut indexable_files = 0usize;
    let mut indexable_bytes = 0u64;
    let mut stable_read_us = 0u64;
    let mut secret_scan_us = 0u64;
    let mut included_paths = HashSet::new();

    for candidate in &inventory.candidates {
        let outcome = read_stable_candidate(&source, candidate);
        stable_read_us = stable_read_us.saturating_add(outcome.stable_read_us);
        secret_scan_us = secret_scan_us.saturating_add(outcome.secret_scan_us);
        if let Some(content) = outcome.content {
            indexable_files += 1;
            indexable_bytes = indexable_bytes.saturating_add(content.bytes);
            included_paths.insert(candidate.relative_path.clone());
        } else if let Some(exclusion) = outcome.exclusion {
            push_global_exclusion(&mut exclusions, exclusion)?;
        }
    }
    exclusions.sort_by(exclusion_order);
    let excluded_entries = exclusions.len();
    let exclusions_truncated = exclusions.len() > limit;
    exclusions.truncate(limit);
    let important_files = inventory
        .important_files
        .into_iter()
        .filter(|path| included_paths.contains(&normalized_relative(path)))
        .take(limit)
        .collect();

    Ok(RagSourceReport {
        schema_version: RAG_SCAN_SCHEMA_VERSION,
        root: source.root,
        collection: source.manifest.collection,
        profile: source.manifest.profile,
        source: source.manifest.source,
        source_kind: source.manifest.source_kind,
        total_files: inventory.total_files,
        indexable_files,
        excluded_entries,
        skipped_large_files: inventory.skipped_large_files,
        indexable_bytes,
        extensions: inventory.extensions,
        important_files,
        exclusions,
        exclusions_truncated,
        scan_us: inventory.scan_us.saturating_add(stable_read_us),
        secret_scan_us,
    })
}

pub fn build_rag_index(
    roots: &[PathBuf],
    output_dir: &Path,
    chunk_chars: usize,
) -> Result<RagIndexReport> {
    build_rag_index_internal(roots, output_dir, chunk_chars, None)
}

#[cfg(test)]
pub(crate) fn build_rag_index_with_fault(
    roots: &[PathBuf],
    output_dir: &Path,
    chunk_chars: usize,
    fault: RagFaultPoint,
) -> Result<RagIndexReport> {
    build_rag_index_internal(roots, output_dir, chunk_chars, Some(fault))
}

fn build_rag_index_internal(
    roots: &[PathBuf],
    output_dir: &Path,
    chunk_chars: usize,
    fault: Option<RagFaultPoint>,
) -> Result<RagIndexReport> {
    if !(MIN_CHUNK_CHARS..=MAX_CHUNK_CHARS).contains(&chunk_chars) {
        bail!("RAG chunk size must be between {MIN_CHUNK_CHARS} and {MAX_CHUNK_CHARS} characters");
    }
    let sources = prepare_sources(roots)?;
    let index_root = prepare_index_root(output_dir)?;
    let legacy_index_detected = legacy_files_exist(&index_root);
    let recovered_staging_directories = recover_incomplete_publications(&index_root)?;
    if index_root.join(RAG_CURRENT_FILE).exists() {
        let active = resolve_active_generation(&index_root)?;
        validate_generation(&active.generation_dir, &active.manifest)?;
    }

    let generation_id = new_generation_id();
    let staging_dir = index_root.join(format!("{STAGING_PREFIX}{generation_id}"));
    fs::create_dir(&staging_dir).with_context(|| {
        format!(
            "failed to create RAG staging directory: {}",
            staging_dir.display()
        )
    })?;
    let documents_path = staging_dir.join(RAG_DOCUMENTS_FILE);
    let chunks_path = staging_dir.join(RAG_CHUNKS_FILE);
    let documents_file = create_new_file(&documents_path)?;
    let chunks_file = create_new_file(&chunks_path)?;
    let mut documents_writer = BufWriter::new(documents_file);
    let mut chunks_writer = BufWriter::new(chunks_file);

    let pipeline_started = Instant::now();
    let mut metrics = RagIndexMetrics::default();
    let mut documents = 0usize;
    let mut chunks = 0usize;
    let mut indexed_bytes = 0u64;
    let mut exclusions = Vec::new();
    let mut scan_fault_checked = false;
    let mut documents_fault_checked = false;
    let mut chunks_fault_checked = false;

    for source in &sources {
        let inventory = collect_inventory(source)?;
        metrics.scan_us = metrics.scan_us.saturating_add(inventory.scan_us);
        extend_global_exclusions(&mut exclusions, inventory.exclusions)?;
        if !scan_fault_checked {
            scan_fault_checked = true;
            inject_fault(fault, RagFaultPoint::DuringScan)?;
        }

        for candidate in inventory.candidates {
            let outcome = read_stable_candidate(source, &candidate);
            metrics.stable_read_us = metrics
                .stable_read_us
                .saturating_add(outcome.stable_read_us);
            metrics.secret_scan_us = metrics
                .secret_scan_us
                .saturating_add(outcome.secret_scan_us);
            let Some(content) = outcome.content else {
                if let Some(exclusion) = outcome.exclusion {
                    push_global_exclusion(&mut exclusions, exclusion)?;
                }
                continue;
            };

            let document_id = hash_parts(&[
                &source.manifest.source,
                &candidate.relative_path,
                &content.blake3,
            ]);
            let document = RagDocumentRecord {
                schema_version: RAG_INDEX_SCHEMA_VERSION,
                id: document_id.clone(),
                collection: source.manifest.collection.clone(),
                profile: source.manifest.profile.clone(),
                source: source.manifest.source.clone(),
                source_kind: source.manifest.source_kind.clone(),
                relative_path: candidate.relative_path.clone(),
                content_type: candidate.content_type.clone(),
                bytes: content.bytes,
                chars: content.chars,
                blake3: content.blake3.clone(),
                inclusion_reason: "explicit_extension_allowlist".to_string(),
                allow_rule: candidate.allow_rule.clone(),
                source_modified_unix_ms: content.source_modified_unix_ms,
            };
            write_json_line(&mut documents_writer, &document)?;
            documents += 1;
            if !documents_fault_checked {
                documents_fault_checked = true;
                inject_fault(fault, RagFaultPoint::DuringDocumentsWrite)?;
            }

            for (chunk_index, text) in chunk_text(&content.text, chunk_chars)
                .into_iter()
                .enumerate()
            {
                let chunk_hash = blake3::hash(text.as_bytes()).to_hex().to_string();
                let chunk = RagChunkRecord {
                    schema_version: RAG_INDEX_SCHEMA_VERSION,
                    id: hash_parts(&[&document_id, &chunk_index.to_string(), &chunk_hash]),
                    document_id: document_id.clone(),
                    collection: source.manifest.collection.clone(),
                    profile: source.manifest.profile.clone(),
                    source: source.manifest.source.clone(),
                    source_kind: source.manifest.source_kind.clone(),
                    relative_path: candidate.relative_path.clone(),
                    content_type: candidate.content_type.clone(),
                    chunk_index,
                    blake3: chunk_hash,
                    inclusion_reason: "bounded_character_chunk".to_string(),
                    allow_rule: candidate.allow_rule.clone(),
                    source_modified_unix_ms: content.source_modified_unix_ms,
                    text,
                };
                write_json_line(&mut chunks_writer, &chunk)?;
                chunks += 1;
                if !chunks_fault_checked {
                    chunks_fault_checked = true;
                    inject_fault(fault, RagFaultPoint::DuringChunksWrite)?;
                }
            }
            indexed_bytes = indexed_bytes.saturating_add(content.bytes);
        }
    }
    exclusions.sort_by(exclusion_order);
    if !ensure_unique_paths(&exclusions) {
        bail!("RAG exclusions contain duplicate source/path/rule entries");
    }

    flush_and_sync(documents_writer, &documents_path)?;
    flush_and_sync(chunks_writer, &chunks_path)?;
    metrics.write_us = duration_us(pipeline_started.elapsed())
        .saturating_sub(metrics.scan_us)
        .saturating_sub(metrics.stable_read_us)
        .saturating_sub(metrics.secret_scan_us);

    let validation_started = Instant::now();
    let validated = validate_data_files(&staging_dir, documents, chunks, None)?;
    metrics.validation_us = duration_us(validation_started.elapsed());
    let configuration = RagIndexConfiguration {
        policy_version: RAG_POLICY_VERSION.to_string(),
        chunk_chars,
        max_file_bytes: DEFAULT_RAG_MAX_FILE_BYTES,
        max_entries: MAX_RAG_ENTRIES,
        allowed_extensions: configuration_allowed_extensions(),
    };
    let configuration_hash = configuration_hash(&configuration)?;
    let manifest = RagIndexManifest {
        schema_version: RAG_INDEX_SCHEMA_VERSION,
        generation_id: generation_id.clone(),
        created_at_unix_ms: unix_ms(),
        opticcode_version: env!("CARGO_PKG_VERSION").to_string(),
        configuration_hash,
        configuration,
        collections: sources
            .iter()
            .map(|source| source.manifest.clone())
            .collect(),
        documents,
        chunks,
        indexed_bytes,
        excluded_entries: exclusions,
        exclusion_rules: exclusion_rules(),
        index_complete: true,
        documents_file: validated.documents_file,
        chunks_file: validated.chunks_file,
        metrics: metrics.clone(),
    };
    inject_fault(fault, RagFaultPoint::BeforeManifestFinalization)?;
    write_json_file(&staging_dir.join(RAG_MANIFEST_FILE), &manifest)?;
    validate_generation(&staging_dir, &manifest)?;

    let generations_dir = ensure_real_directory(&index_root.join(RAG_GENERATIONS_DIR))?;
    let generation_dir = generations_dir.join(&generation_id);
    if generation_dir.exists() {
        bail!("RAG generation already exists: {generation_id}");
    }
    fs::rename(&staging_dir, &generation_dir).with_context(|| {
        format!(
            "failed to finalize RAG generation {} into {}",
            staging_dir.display(),
            generation_dir.display()
        )
    })?;
    let publish_started = Instant::now();
    let pointer_temp = write_current_temp(&index_root, &generation_id)?;
    inject_fault(fault, RagFaultPoint::DuringPublication)?;
    atomic_replace_file(&pointer_temp, &index_root.join(RAG_CURRENT_FILE))?;
    let publish_us = duration_us(publish_started.elapsed());

    let active = resolve_active_generation(&index_root)?;
    if active.manifest.generation_id != generation_id {
        bail!("published RAG generation does not match the requested generation");
    }
    validate_generation(&active.generation_dir, &active.manifest)?;

    Ok(RagIndexReport {
        schema_version: RAG_INDEX_SCHEMA_VERSION,
        output_dir: index_root,
        generation_id,
        manifest_path: generation_dir.join(RAG_MANIFEST_FILE),
        sources: sources.len(),
        documents,
        chunks,
        excluded_entries: manifest.excluded_entries.len(),
        indexed_bytes,
        recovered_staging_directories,
        legacy_index_detected,
        metrics,
        publish_us,
    })
}

pub fn search_rag_index(index_dir: &Path, query: &str, limit: usize) -> Result<Vec<RagSearchHit>> {
    Ok(search_rag_index_report(index_dir, query, limit)?.hits)
}

pub fn load_active_rag_manifest(index_dir: &Path) -> Result<RagIndexManifest> {
    let index_root = resolve_existing_index_root(index_dir)?;
    let active = resolve_active_generation(&index_root)?;
    validate_generation(&active.generation_dir, &active.manifest)?;
    Ok(active.manifest)
}

pub fn search_rag_index_report(
    index_dir: &Path,
    query: &str,
    limit: usize,
) -> Result<RagSearchReport> {
    search_rag_index_queries(index_dir, &[query.to_string()], limit)?
        .pop()
        .context("single RAG search returned no report")
}

pub fn search_rag_index_queries(
    index_dir: &Path,
    queries: &[String],
    limit: usize,
) -> Result<Vec<RagSearchReport>> {
    let started = Instant::now();
    let index_root = resolve_existing_index_root(index_dir)?;
    let active = resolve_active_generation(&index_root)?;
    let chunks_path = active.generation_dir.join(RAG_CHUNKS_FILE);
    ensure_real_file(&chunks_path)?;
    let file = File::open(&chunks_path)
        .with_context(|| format!("failed to open RAG chunks: {}", chunks_path.display()))?;
    let mut reader = BufReader::new(file);
    let mut searches = queries
        .iter()
        .map(|query| {
            let query_lower = query.to_ascii_lowercase();
            let terms = query_lower
                .split_whitespace()
                .filter(|term| !term.is_empty())
                .map(str::to_string)
                .collect();
            PreparedSearch {
                query: query.clone(),
                query_lower,
                terms,
                hits: Vec::new(),
            }
        })
        .collect::<Vec<_>>();
    let mut hasher = blake3::Hasher::new();
    let mut bytes = 0u64;
    let mut records = 0usize;
    let mut raw = Vec::new();

    loop {
        raw.clear();
        let read = reader.read_until(b'\n', &mut raw)?;
        if read == 0 {
            break;
        }
        hasher.update(&raw);
        bytes = bytes.saturating_add(read as u64);
        let line = trim_json_line(&raw);
        if line.is_empty() {
            continue;
        }
        let chunk: RagChunkRecord = serde_json::from_slice(line)
            .context("active RAG index contains an invalid chunk record")?;
        validate_chunk_record(&chunk)?;
        if !active.manifest.collections.iter().any(|source| {
            source.collection == chunk.collection
                && source.profile == chunk.profile
                && source.source == chunk.source
                && source.source_kind == chunk.source_kind
        }) {
            bail!("active RAG chunk references unknown source provenance");
        }
        if detect_secret(&chunk.text).is_some() {
            bail!("active RAG index contains content rejected by the current secret policy");
        }
        records += 1;
        let text_lower = chunk.text.to_ascii_lowercase();
        let mut document_path = None;
        for search in &mut searches {
            if search.terms.len() > 1
                && search
                    .terms
                    .iter()
                    .any(|term| !text_lower.contains(term.as_str()))
            {
                continue;
            }
            let term_score = search
                .terms
                .iter()
                .map(|term| text_lower.matches(term.as_str()).count())
                .sum::<usize>();
            let phrase_score = if search.terms.len() > 1 {
                text_lower.matches(&search.query_lower).count() * search.terms.len() * 8
            } else {
                0
            };
            let score = term_score + phrase_score;
            if score == 0 {
                continue;
            }
            let document_path = document_path
                .get_or_insert_with(|| format!("{}:{}", chunk.source_kind, chunk.relative_path));
            search.hits.push(RagSearchHit {
                document_path: document_path.clone(),
                chunk_id: chunk.id.clone(),
                score,
                preview: make_preview(&chunk.text, &search.terms, 240),
            });
        }
    }
    if bytes != active.manifest.chunks_file.bytes
        || records != active.manifest.chunks_file.records
        || hasher.finalize().to_hex().as_str() != active.manifest.chunks_file.blake3
    {
        bail!("active RAG chunks do not match the published manifest");
    }
    let duration_us = duration_us(started.elapsed());
    Ok(searches
        .into_iter()
        .map(|mut search| {
            search.hits.sort_by(|left, right| {
                right
                    .score
                    .cmp(&left.score)
                    .then_with(|| left.document_path.cmp(&right.document_path))
                    .then_with(|| left.chunk_id.cmp(&right.chunk_id))
            });
            search.hits.truncate(limit);
            RagSearchReport {
                schema_version: RAG_SEARCH_SCHEMA_VERSION,
                generation_id: active.manifest.generation_id.clone(),
                query: search.query,
                hits: search.hits,
                duration_us,
            }
        })
        .collect())
}

fn validate_generation(directory: &Path, expected: &RagIndexManifest) -> Result<()> {
    ensure_real_directory_existing(directory)?;
    let manifest_path = directory.join(RAG_MANIFEST_FILE);
    ensure_real_file(&manifest_path)?;
    let manifest: RagIndexManifest =
        serde_json::from_slice(&fs::read(&manifest_path).with_context(|| {
            format!("failed to read RAG manifest: {}", manifest_path.display())
        })?)
        .context("failed to parse RAG manifest")?;
    validate_manifest(&manifest)?;
    if &manifest != expected {
        bail!("RAG manifest changed during validation");
    }
    let validated = validate_data_files(
        directory,
        manifest.documents,
        manifest.chunks,
        Some(&manifest),
    )?;
    if validated.documents_file != manifest.documents_file
        || validated.chunks_file != manifest.chunks_file
        || validated.indexed_bytes != manifest.indexed_bytes
    {
        bail!("RAG data files do not match their manifest hashes, counts, or byte total");
    }
    Ok(())
}

fn validate_manifest(manifest: &RagIndexManifest) -> Result<()> {
    if manifest.schema_version != RAG_INDEX_SCHEMA_VERSION {
        bail!(
            "unsupported RAG index schema {}; expected {}",
            manifest.schema_version,
            RAG_INDEX_SCHEMA_VERSION
        );
    }
    validate_generation_id(&manifest.generation_id)?;
    if !manifest.index_complete {
        bail!("RAG manifest is not marked complete");
    }
    if manifest.collections.is_empty() {
        bail!("RAG manifest has no authorized collection");
    }
    if manifest.created_at_unix_ms == 0
        || manifest.opticcode_version.is_empty()
        || manifest.configuration.policy_version != RAG_POLICY_VERSION
        || !(MIN_CHUNK_CHARS..=MAX_CHUNK_CHARS).contains(&manifest.configuration.chunk_chars)
        || manifest.configuration.max_file_bytes != DEFAULT_RAG_MAX_FILE_BYTES
        || manifest.configuration.max_entries != MAX_RAG_ENTRIES
        || manifest.configuration.allowed_extensions != configuration_allowed_extensions()
        || manifest.configuration_hash != configuration_hash(&manifest.configuration)?
    {
        bail!("RAG manifest policy or configuration hash is invalid");
    }
    if manifest.documents_file.name != RAG_DOCUMENTS_FILE
        || manifest.chunks_file.name != RAG_CHUNKS_FILE
    {
        bail!("RAG manifest references unsupported data filenames");
    }
    if manifest.excluded_entries.len() > MAX_RAG_EXCLUSIONS
        || !ensure_unique_paths(&manifest.excluded_entries)
    {
        bail!("RAG manifest exclusions are invalid or exceed their bound");
    }
    let rule_ids = manifest
        .exclusion_rules
        .iter()
        .map(|rule| rule.rule_id.as_str())
        .collect::<HashSet<_>>();
    if rule_ids.len() != manifest.exclusion_rules.len()
        || manifest.exclusion_rules.iter().any(|rule| {
            rule.rule_id.is_empty() || rule.category.is_empty() || rule.decision != "exclude"
        })
    {
        bail!("RAG manifest exclusion rules are invalid");
    }
    let source_keys = manifest
        .collections
        .iter()
        .map(|source| {
            (
                source.collection.as_str(),
                source.profile.as_str(),
                source.source.as_str(),
                source.source_kind.as_str(),
            )
        })
        .collect::<HashSet<_>>();
    if source_keys.len() != manifest.collections.len() {
        bail!("RAG manifest contains duplicate source provenance");
    }
    for exclusion in &manifest.excluded_entries {
        if exclusion.collection.is_empty()
            || exclusion.source.is_empty()
            || exclusion.entry_kind.is_empty()
            || exclusion.category.is_empty()
            || exclusion.decision != "excluded"
            || !rule_ids.contains(exclusion.rule_id.as_str())
            || exclusion
                .position
                .as_ref()
                .is_some_and(|position| position.line == 0 || position.column == 0)
        {
            bail!("RAG manifest contains an invalid exclusion record");
        }
        if !manifest.collections.iter().any(|source| {
            source.collection == exclusion.collection && source.source == exclusion.source
        }) {
            bail!("RAG exclusion references an unknown source");
        }
        if exclusion.relative_path != "<outside-root>" {
            validate_portable_relative_path(&exclusion.relative_path)?;
        }
    }
    for source in &manifest.collections {
        if source.root_fingerprint.len() != 64
            || !source
                .root_fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            bail!("RAG manifest contains an invalid source root fingerprint");
        }
        let expected_source = format!("source-{}", &source.root_fingerprint[..16]);
        let expected_collection =
            format!("{}-{}", source.source_kind, &source.root_fingerprint[..12]);
        if source.source.is_empty()
            || source.collection.is_empty()
            || source.profile != RAG_POLICY_VERSION
            || source.source != expected_source
            || source.collection != expected_collection
            || source.source_label.is_empty()
            || source.source_label.contains(['/', '\\', ':'])
        {
            bail!("RAG manifest contains invalid source provenance");
        }
    }
    Ok(())
}

fn validate_data_files(
    directory: &Path,
    expected_documents: usize,
    expected_chunks: usize,
    manifest: Option<&RagIndexManifest>,
) -> Result<ValidatedData> {
    let documents_path = directory.join(RAG_DOCUMENTS_FILE);
    let chunks_path = directory.join(RAG_CHUNKS_FILE);
    ensure_real_file(&documents_path)?;
    ensure_real_file(&chunks_path)?;

    let mut documents = HashMap::new();
    let mut reconstructed = HashMap::<String, String>::new();
    let mut document_records = 0usize;
    let mut indexed_bytes = 0u64;
    read_json_lines_hashed(&documents_path, |line| {
        let document: RagDocumentRecord =
            serde_json::from_slice(line).context("RAG documents file contains invalid JSON")?;
        validate_document_record(&document)?;
        if let Some(manifest) = manifest {
            if !manifest.collections.iter().any(|source| {
                source.collection == document.collection
                    && source.profile == document.profile
                    && source.source == document.source
                    && source.source_kind == document.source_kind
            }) {
                bail!("RAG document references unknown source provenance");
            }
            if document.bytes > manifest.configuration.max_file_bytes {
                bail!("RAG document exceeds the published per-file size limit");
            }
        }
        if documents
            .insert(document.id.clone(), document.clone())
            .is_some()
        {
            bail!("RAG documents file contains a duplicate document id");
        }
        reconstructed.insert(document.id.clone(), String::new());
        indexed_bytes = indexed_bytes
            .checked_add(document.bytes)
            .context("RAG indexed byte total overflowed")?;
        document_records += 1;
        Ok(())
    })?;
    if document_records != expected_documents {
        bail!("RAG document count does not match the expected count");
    }

    let mut chunk_records = 0usize;
    let mut chunk_ids = HashSet::new();
    let mut next_chunk_index = HashMap::<String, usize>::new();
    let chunks_file = read_json_lines_hashed(&chunks_path, |line| {
        let chunk: RagChunkRecord =
            serde_json::from_slice(line).context("RAG chunks file contains invalid JSON")?;
        validate_chunk_record(&chunk)?;
        if manifest
            .is_some_and(|manifest| chunk.text.chars().count() > manifest.configuration.chunk_chars)
        {
            bail!("RAG chunk exceeds the published character limit");
        }
        let document = documents
            .get(&chunk.document_id)
            .context("RAG chunk references an unknown document")?;
        if chunk.collection != document.collection
            || chunk.profile != document.profile
            || chunk.source != document.source
            || chunk.source_kind != document.source_kind
            || chunk.relative_path != document.relative_path
            || chunk.content_type != document.content_type
            || chunk.allow_rule != document.allow_rule
            || chunk.source_modified_unix_ms != document.source_modified_unix_ms
        {
            bail!("RAG chunk provenance does not match its document");
        }
        if !chunk_ids.insert(chunk.id.clone()) {
            bail!("RAG chunks file contains a duplicate chunk id");
        }
        let expected_index = next_chunk_index
            .entry(chunk.document_id.clone())
            .or_insert(0);
        if chunk.chunk_index != *expected_index {
            bail!("RAG chunk indexes are not contiguous and deterministic");
        }
        *expected_index += 1;
        reconstructed
            .get_mut(&chunk.document_id)
            .context("RAG reconstruction is missing a document")?
            .push_str(&chunk.text);
        chunk_records += 1;
        Ok(())
    })?;
    if chunk_records != expected_chunks {
        bail!("RAG chunk count does not match the expected count");
    }

    for (document_id, document) in &documents {
        let content = reconstructed
            .get(document_id)
            .context("RAG document reconstruction is missing")?;
        if content.len() as u64 != document.bytes
            || content.chars().count() != document.chars
            || blake3::hash(content.as_bytes()).to_hex().as_str() != document.blake3
        {
            bail!("RAG document content does not match its BLAKE3 provenance");
        }
    }
    let documents_file = describe_jsonl(&documents_path, document_records)?;
    Ok(ValidatedData {
        documents_file,
        chunks_file: RagIndexFileDescriptor {
            name: RAG_CHUNKS_FILE.to_string(),
            bytes: chunks_file.0,
            records: chunk_records,
            blake3: chunks_file.1,
        },
        indexed_bytes,
    })
}

fn validate_document_record(document: &RagDocumentRecord) -> Result<()> {
    if document.schema_version != RAG_INDEX_SCHEMA_VERSION
        || document.id.is_empty()
        || document.collection.is_empty()
        || document.profile != RAG_POLICY_VERSION
        || document.source.is_empty()
        || document.source_kind.is_empty()
        || document.content_type.is_empty()
        || document.inclusion_reason != "explicit_extension_allowlist"
        || !document.allow_rule.starts_with("allow.extension.")
    {
        bail!("RAG document has invalid schema or provenance");
    }
    validate_portable_relative_path(&document.relative_path)?;
    if !record_matches_allow_policy(
        &document.relative_path,
        &document.content_type,
        &document.allow_rule,
    ) {
        bail!("RAG document is not allowed by the published ingestion policy");
    }
    let expected_id = hash_parts(&[&document.source, &document.relative_path, &document.blake3]);
    if document.id != expected_id || document.blake3.len() != 64 {
        bail!("RAG document id or hash is invalid");
    }
    Ok(())
}

fn validate_chunk_record(chunk: &RagChunkRecord) -> Result<()> {
    if chunk.schema_version != RAG_INDEX_SCHEMA_VERSION
        || chunk.id.is_empty()
        || chunk.document_id.is_empty()
        || chunk.collection.is_empty()
        || chunk.profile != RAG_POLICY_VERSION
        || chunk.source.is_empty()
        || chunk.source_kind.is_empty()
        || chunk.content_type.is_empty()
        || chunk.inclusion_reason != "bounded_character_chunk"
        || !chunk.allow_rule.starts_with("allow.extension.")
    {
        bail!("RAG chunk has invalid schema or provenance");
    }
    validate_portable_relative_path(&chunk.relative_path)?;
    if !record_matches_allow_policy(&chunk.relative_path, &chunk.content_type, &chunk.allow_rule) {
        bail!("RAG chunk is not allowed by the published ingestion policy");
    }
    let actual_hash = blake3::hash(chunk.text.as_bytes()).to_hex().to_string();
    let expected_id = hash_parts(&[
        &chunk.document_id,
        &chunk.chunk_index.to_string(),
        &actual_hash,
    ]);
    if chunk.blake3 != actual_hash || chunk.id != expected_id {
        bail!("RAG chunk id or hash is invalid");
    }
    if detect_secret(&chunk.text).is_some() {
        bail!("RAG chunk contains content rejected by the current secret policy");
    }
    Ok(())
}

fn resolve_active_generation(index_root: &Path) -> Result<ActiveGeneration> {
    let current_path = index_root.join(RAG_CURRENT_FILE);
    if !current_path.exists() {
        if legacy_files_exist(index_root) {
            bail!(
                "legacy RAG index detected at {}; rebuild it with `opticcode rag-index` before searching",
                index_root.display()
            );
        }
        bail!(
            "no published RAG index found at {}; run `opticcode rag-index` first",
            index_root.display()
        );
    }
    ensure_real_file(&current_path)?;
    let generation_id = fs::read_to_string(&current_path).with_context(|| {
        format!(
            "failed to read RAG current pointer: {}",
            current_path.display()
        )
    })?;
    let generation_id = generation_id.trim();
    validate_generation_id(generation_id)?;
    let generation_dir = index_root.join(RAG_GENERATIONS_DIR).join(generation_id);
    ensure_real_directory_existing(&generation_dir)?;
    let canonical_generation = fs::canonicalize(&generation_dir)?;
    if canonical_generation.strip_prefix(index_root).is_err() {
        bail!("active RAG generation escaped the index root");
    }
    let manifest_path = generation_dir.join(RAG_MANIFEST_FILE);
    ensure_real_file(&manifest_path)?;
    let manifest: RagIndexManifest = serde_json::from_slice(&fs::read(&manifest_path)?)
        .context("failed to parse active RAG manifest")?;
    validate_manifest(&manifest)?;
    if manifest.generation_id != generation_id {
        bail!("active RAG pointer and manifest generation do not match");
    }
    Ok(ActiveGeneration {
        generation_dir,
        manifest,
    })
}

fn prepare_index_root(output_dir: &Path) -> Result<PathBuf> {
    let absolute = if output_dir.is_absolute() {
        output_dir.to_path_buf()
    } else {
        std::env::current_dir()?.join(output_dir)
    };
    let parent = absolute
        .parent()
        .context("RAG index output must have a parent directory")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create RAG index parent: {}", parent.display()))?;
    let parent_metadata = fs::symlink_metadata(parent)?;
    if metadata_is_link_or_reparse(&parent_metadata) || !parent_metadata.is_dir() {
        bail!("RAG index parent is a symlink, reparse point, or non-directory");
    }
    if absolute.exists() {
        ensure_real_directory_existing(&absolute)?;
    } else {
        fs::create_dir(&absolute)
            .with_context(|| format!("failed to create RAG index root: {}", absolute.display()))?;
    }
    resolve_existing_index_root(&absolute)
}

fn resolve_existing_index_root(index_dir: &Path) -> Result<PathBuf> {
    let original = fs::symlink_metadata(index_dir)
        .with_context(|| format!("failed to inspect RAG index root: {}", index_dir.display()))?;
    if metadata_is_link_or_reparse(&original) || !original.is_dir() {
        bail!("RAG index root must be a real directory, not a symlink or reparse point");
    }
    fs::canonicalize(index_dir)
        .with_context(|| format!("failed to resolve RAG index root: {}", index_dir.display()))
}

fn recover_incomplete_publications(index_root: &Path) -> Result<usize> {
    let mut recovered = 0usize;
    for entry in fs::read_dir(index_root)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();
        if name.starts_with(STAGING_PREFIX) {
            let generation_id = name
                .strip_prefix(STAGING_PREFIX)
                .context("RAG staging prefix parsing failed")?;
            validate_generation_id(generation_id).with_context(|| {
                format!(
                    "unrecognized RAG staging directory requires manual inspection: {}",
                    path.display()
                )
            })?;
            let metadata = fs::symlink_metadata(&path)?;
            if metadata_is_link_or_reparse(&metadata) || !metadata.is_dir() {
                bail!(
                    "unsafe RAG staging entry requires manual inspection: {}",
                    path.display()
                );
            }
            ensure_child_path(index_root, &path)?;
            fs::remove_dir_all(&path).with_context(|| {
                format!(
                    "failed to recover incomplete RAG staging directory: {}",
                    path.display()
                )
            })?;
            recovered += 1;
        } else if name.starts_with(CURRENT_TEMP_PREFIX) && name.ends_with(CURRENT_TEMP_SUFFIX) {
            let generation_id = name
                .strip_prefix(CURRENT_TEMP_PREFIX)
                .and_then(|value| value.strip_suffix(CURRENT_TEMP_SUFFIX))
                .context("RAG pointer temporary name parsing failed")?;
            validate_generation_id(generation_id).with_context(|| {
                format!(
                    "unrecognized RAG pointer temporary requires manual inspection: {}",
                    path.display()
                )
            })?;
            let metadata = fs::symlink_metadata(&path)?;
            if metadata_is_link_or_reparse(&metadata) || !metadata.is_file() {
                bail!(
                    "unsafe RAG pointer temporary requires manual inspection: {}",
                    path.display()
                );
            }
            ensure_child_path(index_root, &path)?;
            fs::remove_file(&path).with_context(|| {
                format!(
                    "failed to remove incomplete RAG pointer temporary: {}",
                    path.display()
                )
            })?;
        }
    }
    Ok(recovered)
}

fn write_current_temp(index_root: &Path, generation_id: &str) -> Result<PathBuf> {
    let path = index_root.join(format!(
        "{CURRENT_TEMP_PREFIX}{generation_id}{CURRENT_TEMP_SUFFIX}"
    ));
    let mut file = create_new_file(&path)?;
    file.write_all(generation_id.as_bytes())?;
    file.write_all(b"\n")?;
    file.flush()?;
    file.sync_all()?;
    Ok(path)
}

#[cfg(windows)]
fn atomic_replace_file(source: &Path, destination: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    ensure_real_file(source)?;
    if destination.exists() {
        ensure_real_file(destination)?;
    }
    let source_wide = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination_wide = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            source_wide.as_ptr(),
            destination_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error())
            .context("failed to atomically publish RAG CURRENT pointer");
    }
    Ok(())
}

#[cfg(not(windows))]
fn atomic_replace_file(source: &Path, destination: &Path) -> Result<()> {
    ensure_real_file(source)?;
    if destination.exists() {
        ensure_real_file(destination)?;
    }
    fs::rename(source, destination).context("failed to atomically publish RAG CURRENT pointer")
}

fn validate_generation_id(value: &str) -> Result<()> {
    let parts = value.split('-').collect::<Vec<_>>();
    if value.len() > 96
        || parts.len() != 4
        || parts[0] != "g"
        || parts[1..]
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        bail!("RAG generation id is invalid");
    }
    Ok(())
}

fn validate_portable_relative_path(value: &str) -> Result<()> {
    if value.is_empty() || value.contains('\\') {
        bail!("RAG record path is empty or not normalized");
    }
    let path = Path::new(value);
    if path.is_absolute()
        || !path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        bail!("RAG record path is not a portable relative path");
    }
    Ok(())
}

fn create_new_file(path: &Path) -> Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("failed to create RAG file: {}", path.display()))
}

fn flush_and_sync(mut writer: BufWriter<File>, path: &Path) -> Result<()> {
    writer
        .flush()
        .with_context(|| format!("failed to flush RAG file: {}", path.display()))?;
    let file = writer.into_inner().map_err(|error| error.into_error())?;
    file.sync_all()
        .with_context(|| format!("failed to sync RAG file: {}", path.display()))
}

fn write_json_line<T: serde::Serialize>(writer: &mut BufWriter<File>, value: &T) -> Result<()> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n")?;
    Ok(())
}

fn write_json_file<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    let mut file = create_new_file(path)?;
    file.write_all(&bytes)?;
    file.flush()?;
    file.sync_all()?;
    Ok(())
}

fn read_json_lines_hashed<F>(path: &Path, mut visit: F) -> Result<(u64, String)>
where
    F: FnMut(&[u8]) -> Result<()>,
{
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut raw = Vec::new();
    let mut bytes = 0u64;
    let mut hasher = blake3::Hasher::new();
    loop {
        raw.clear();
        let read = reader.read_until(b'\n', &mut raw)?;
        if read == 0 {
            break;
        }
        hasher.update(&raw);
        bytes = bytes.saturating_add(read as u64);
        let line = trim_json_line(&raw);
        if !line.is_empty() {
            visit(line)?;
        }
    }
    Ok((bytes, hasher.finalize().to_hex().to_string()))
}

fn describe_jsonl(path: &Path, records: usize) -> Result<RagIndexFileDescriptor> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let bytes = std::io::copy(&mut file, &mut hasher)?;
    Ok(RagIndexFileDescriptor {
        name: path
            .file_name()
            .and_then(|value| value.to_str())
            .context("RAG data filename is not UTF-8")?
            .to_string(),
        bytes,
        records,
        blake3: hasher.finalize().to_hex().to_string(),
    })
}

fn trim_json_line(value: &[u8]) -> &[u8] {
    let value = value.strip_suffix(b"\n").unwrap_or(value);
    value.strip_suffix(b"\r").unwrap_or(value)
}

fn configuration_hash(configuration: &RagIndexConfiguration) -> Result<String> {
    Ok(blake3::hash(&serde_json::to_vec(configuration)?)
        .to_hex()
        .to_string())
}

fn ensure_real_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect RAG file: {}", path.display()))?;
    if metadata_is_link_or_reparse(&metadata) || !metadata.is_file() {
        bail!(
            "RAG path is a symlink, reparse point, or non-file: {}",
            path.display()
        );
    }
    Ok(())
}

fn ensure_real_directory(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        ensure_real_directory_existing(path)?;
    } else {
        fs::create_dir(path)
            .with_context(|| format!("failed to create RAG directory: {}", path.display()))?;
    }
    fs::canonicalize(path)
        .with_context(|| format!("failed to resolve RAG directory: {}", path.display()))
}

fn ensure_real_directory_existing(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect RAG directory: {}", path.display()))?;
    if metadata_is_link_or_reparse(&metadata) || !metadata.is_dir() {
        bail!(
            "RAG path is a symlink, reparse point, or non-directory: {}",
            path.display()
        );
    }
    Ok(())
}

fn ensure_child_path(root: &Path, path: &Path) -> Result<()> {
    if path.parent() != Some(root) {
        bail!("RAG recovery path escaped the index root");
    }
    Ok(())
}

fn legacy_files_exist(index_root: &Path) -> bool {
    index_root.join(RAG_DOCUMENTS_FILE).is_file() || index_root.join(RAG_CHUNKS_FILE).is_file()
}

fn inject_fault(actual: Option<RagFaultPoint>, expected: RagFaultPoint) -> Result<()> {
    if actual == Some(expected) {
        bail!("injected RAG fault at {expected:?}");
    }
    Ok(())
}

fn push_global_exclusion(
    exclusions: &mut Vec<RagExclusionRecord>,
    exclusion: RagExclusionRecord,
) -> Result<()> {
    if exclusions.len() >= MAX_RAG_EXCLUSIONS {
        bail!("RAG index exceeds the bounded exclusion limit of {MAX_RAG_EXCLUSIONS}");
    }
    exclusions.push(exclusion);
    Ok(())
}

fn extend_global_exclusions(
    exclusions: &mut Vec<RagExclusionRecord>,
    additional: Vec<RagExclusionRecord>,
) -> Result<()> {
    if exclusions.len().saturating_add(additional.len()) > MAX_RAG_EXCLUSIONS {
        bail!("RAG index exceeds the bounded exclusion limit of {MAX_RAG_EXCLUSIONS}");
    }
    exclusions.extend(additional);
    Ok(())
}

fn exclusion_order(left: &RagExclusionRecord, right: &RagExclusionRecord) -> std::cmp::Ordering {
    left.source
        .cmp(&right.source)
        .then_with(|| left.relative_path.cmp(&right.relative_path))
        .then_with(|| left.rule_id.cmp(&right.rule_id))
}

fn hash_parts(parts: &[&str]) -> String {
    let mut hasher = blake3::Hasher::new();
    for part in parts {
        hasher.update(&(part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

fn chunk_text(content: &str, chunk_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::with_capacity(chunk_chars);
    let mut current_chars = 0usize;
    for character in content.chars() {
        current.push(character);
        current_chars += 1;
        if current_chars >= chunk_chars {
            chunks.push(current);
            current = String::with_capacity(chunk_chars);
            current_chars = 0;
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn make_preview(text: &str, terms: &[String], max_chars: usize) -> String {
    let lower = text.to_ascii_lowercase();
    let phrase = terms.join(" ");
    let first_match = lower
        .find(&phrase)
        .or_else(|| {
            terms
                .iter()
                .filter_map(|term| lower.find(term.as_str()))
                .min()
        })
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

fn new_generation_id() -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = GENERATION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("g-{stamp:x}-{:x}-{sequence:x}", std::process::id())
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn duration_us(value: Duration) -> u64 {
    value.as_micros().min(u128::from(u64::MAX)) as u64
}

fn normalized_relative(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
