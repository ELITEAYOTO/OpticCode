use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::index::{build_rag_index_with_fault, RagFaultPoint};
use super::policy::{
    collect_inventory, prepare_sources, read_stable_candidate, read_stable_candidate_with_hook,
    RagCandidate, DEFAULT_RAG_MAX_FILE_BYTES,
};
use super::schema::{
    RagIndexManifest, RAG_CHUNKS_FILE, RAG_CURRENT_FILE, RAG_DOCUMENTS_FILE, RAG_GENERATIONS_DIR,
    RAG_INDEX_SCHEMA_VERSION, RAG_MANIFEST_FILE, RAG_POLICY_VERSION,
};
use super::secrets::detect_secret;
use super::{build_rag_index, inspect_rag_source, search_rag_index, search_rag_index_queries};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TemporaryFixture {
    root: PathBuf,
}

impl TemporaryFixture {
    fn new(label: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after Unix epoch")
            .as_nanos();
        let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "opticcode-rag-safe-{label}-{}-{stamp}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("temporary RAG fixture should be created");
        Self { root }
    }

    fn directory(&self, relative: &str) -> PathBuf {
        let path = self.root.join(relative);
        fs::create_dir_all(&path).expect("fixture directory should be created");
        path
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("fixture parent should be created");
        }
        fs::write(&path, content).expect("fixture file should be written");
        path
    }
}

impl Drop for TemporaryFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn secret_detector_blocks_explicit_patterns_without_needing_values_in_reports() {
    let cases = [
        (
            "-----BEGIN PRIVATE KEY-----\ninvalid-test-material",
            "secret.private_key",
        ),
        (
            "token=ghp_000000000000000000000000000000000000",
            "secret.github_token",
        ),
        ("key=AKIA0000000000000000", "secret.aws_access_key"),
        (
            "api_key=sk-test_only_000000000000000000000000",
            "secret.openai_token",
        ),
        (
            "remote=https://test-user:not-a-real-password@example.invalid/repo",
            "secret.uri_credentials",
        ),
        (
            "password: fake-test-only-value",
            "secret.credential_assignment",
        ),
    ];
    for (content, expected_rule) in cases {
        let detection = detect_secret(content).expect("fake secret should be rejected");
        assert_eq!(detection.rule_id, expected_rule);
        assert!(detection.line >= 1);
        assert!(detection.column >= 1);
    }

    assert!(detect_secret("The password field is documented but no value is present.").is_none());
    assert!(detect_secret("password: ${DB_PASSWORD}\nsecret: <injected-at-runtime>").is_none());
}

#[test]
fn scan_is_fail_closed_for_names_extensions_sizes_and_secret_content() {
    let fixture = TemporaryFixture::new("policy");
    let source = fixture.directory("source");
    fs::write(source.join("Safe.java"), "class Safe {}\n").unwrap();
    fs::write(source.join("Guide.md"), "Safe legacy notes.\n").unwrap();
    fs::write(
        source.join("safe.yml"),
        "message: The password field is documented.\n",
    )
    .unwrap();
    fs::write(source.join("README"), "extensionless\n").unwrap();
    fs::write(source.join(".env"), "VALUE=invalid-test-only\n").unwrap();
    fs::write(source.join(".env.local"), "VALUE=invalid-test-only\n").unwrap();
    fs::write(source.join("credentials.yml"), "value: invalid-test-only\n").unwrap();
    fs::write(source.join("private.pem"), "invalid test material\n").unwrap();
    fs::write(
        source.join("config.yml"),
        "password: fake-test-only-value\n",
    )
    .unwrap();
    fs::write(
        source.join("application.properties"),
        "database.password=fake-test-only-value\n",
    )
    .unwrap();
    fs::write(
        source.join("auth-guide.md"),
        "ghp_000000000000000000000000000000000000\n",
    )
    .unwrap();
    fs::create_dir_all(source.join("benchmarks").join("runs")).unwrap();
    fs::write(
        source.join("benchmarks").join("runs").join("generated.md"),
        "generated benchmark output\n",
    )
    .unwrap();
    fs::create_dir(source.join("Idées-Vrac")).unwrap();
    fs::write(
        source.join("Idées-Vrac").join("future-pentesting.md"),
        "private future notes\n",
    )
    .unwrap();
    fs::write(
        source.join("TooLarge.java"),
        vec![b'a'; DEFAULT_RAG_MAX_FILE_BYTES as usize + 1],
    )
    .unwrap();

    let report = inspect_rag_source(&source, 100).expect("safe scan should complete");
    assert_eq!(report.indexable_files, 3);
    assert_eq!(report.skipped_large_files, 1);
    let rules = report
        .exclusions
        .iter()
        .map(|entry| (entry.relative_path.as_str(), entry.rule_id.as_str()))
        .collect::<Vec<_>>();
    assert!(rules.contains(&("README", "type.extensionless")));
    assert!(rules.contains(&(".env", "path.file.environment")));
    assert!(rules.contains(&(".env.local", "path.file.environment")));
    assert!(rules.contains(&("credentials.yml", "path.file.credential")));
    assert!(rules.contains(&("private.pem", "path.file.private_key")));
    assert!(rules.contains(&("config.yml", "secret.credential_assignment")));
    assert!(rules.contains(&("application.properties", "secret.credential_assignment")));
    assert!(rules.contains(&("auth-guide.md", "secret.github_token")));
    assert!(rules.contains(&("benchmarks/runs", "path.directory.benchmark_runs")));
    assert!(rules.contains(&("TooLarge.java", "size.too_large")));
    assert!(rules.contains(&("Idées-Vrac", "path.directory.private_notes")));
}

#[test]
fn stable_read_rejects_a_file_changed_after_reading() {
    let fixture = TemporaryFixture::new("changed");
    let source_root = fixture.directory("source");
    fs::write(source_root.join("Safe.java"), "class Safe {}\n").unwrap();
    let source = prepare_sources(&[source_root])
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let candidate = collect_inventory(&source).unwrap().candidates.remove(0);
    let outcome = read_stable_candidate_with_hook(&source, &candidate, |path| {
        fs::write(path, "class Safe { int changed = 1; }\n").unwrap();
    });
    assert!(outcome.content.is_none());
    assert_eq!(
        outcome.exclusion.unwrap().rule_id,
        "content.changed_during_read"
    );
}

#[test]
fn canonical_root_boundary_rejects_an_outside_candidate() {
    let fixture = TemporaryFixture::new("escape");
    let source_root = fixture.directory("source");
    let outside = fixture.write("Outside.java", "class Outside {}\n");
    let source = prepare_sources(&[source_root])
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let candidate = RagCandidate {
        absolute_path: outside,
        relative_path: "../Outside.java".to_string(),
        content_type: "java".to_string(),
        allow_rule: "allow.extension.java".to_string(),
    };
    let outcome = read_stable_candidate(&source, &candidate);
    assert!(outcome.content.is_none());
    assert_eq!(outcome.exclusion.unwrap().rule_id, "path.escape");
}

#[test]
fn versioned_index_is_portable_deterministic_and_secret_free() {
    let fixture = TemporaryFixture::new("portable");
    let source = fixture.directory("source");
    fs::write(source.join("B.md"), "legacy spawner notes\n").unwrap();
    fs::write(
        source.join("A.java"),
        "class A { String value = \"spawner\"; }\n",
    )
    .unwrap();
    fs::write(
        source.join("config.yml"),
        "password: fake-test-only-value\n",
    )
    .unwrap();
    let first_index = fixture.root.join("index-one");
    let second_index = fixture.root.join("index-two");
    let first = build_rag_index(std::slice::from_ref(&source), &first_index, 512).unwrap();
    build_rag_index(std::slice::from_ref(&source), &second_index, 512).unwrap();
    assert_eq!(first.schema_version, RAG_INDEX_SCHEMA_VERSION);
    assert_eq!(first.documents, 2);
    assert_eq!(first.chunks, 2);
    assert_eq!(first.excluded_entries, 1);

    let first_generation = active_generation_dir(&first_index);
    let second_generation = active_generation_dir(&second_index);
    assert_eq!(
        fs::read(first_generation.join(RAG_DOCUMENTS_FILE)).unwrap(),
        fs::read(second_generation.join(RAG_DOCUMENTS_FILE)).unwrap()
    );
    assert_eq!(
        fs::read(first_generation.join(RAG_CHUNKS_FILE)).unwrap(),
        fs::read(second_generation.join(RAG_CHUNKS_FILE)).unwrap()
    );
    let manifest = read_manifest(&first_index);
    assert!(manifest.index_complete);
    assert_eq!(manifest.configuration.policy_version, RAG_POLICY_VERSION);
    assert_eq!(manifest.documents, 2);
    assert_eq!(manifest.chunks, 2);
    assert_eq!(manifest.excluded_entries[0].relative_path, "config.yml");
    let artifacts = [RAG_DOCUMENTS_FILE, RAG_CHUNKS_FILE, RAG_MANIFEST_FILE]
        .into_iter()
        .flat_map(|name| fs::read(first_generation.join(name)).unwrap())
        .collect::<Vec<_>>();
    let artifacts = String::from_utf8(artifacts).unwrap();
    assert!(!artifacts.contains("fake-test-only-value"));
    assert!(!artifacts.contains(&source.to_string_lossy().to_string()));
    assert_eq!(
        search_rag_index(&first_index, "spawner", 10).unwrap().len(),
        2
    );
    let queries = vec!["spawner".to_string(), "legacy".to_string()];
    let reports = search_rag_index_queries(&first_index, &queries, 10).unwrap();
    assert_eq!(reports.len(), queries.len());
    for (query, report) in queries.iter().zip(reports) {
        assert_eq!(report.query, *query);
        assert_eq!(
            report.hits,
            search_rag_index(&first_index, query, 10).unwrap()
        );
    }
}

#[test]
fn legacy_index_is_rejected_with_an_explicit_rebuild_error() {
    let fixture = TemporaryFixture::new("legacy");
    let index = fixture.directory("legacy-index");
    fs::write(index.join(RAG_DOCUMENTS_FILE), "{}\n").unwrap();
    fs::write(index.join(RAG_CHUNKS_FILE), "{}\n").unwrap();
    let error = search_rag_index(&index, "test", 1).unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("legacy RAG index"));
    assert!(message.contains("rebuild"));
}

#[test]
fn all_fault_points_preserve_the_previous_active_generation() {
    let fixture = TemporaryFixture::new("faults");
    let source = fixture.directory("source");
    let source_file = source.join("Marker.java");
    fs::write(
        &source_file,
        "class Marker { String value = \"baseline_marker\"; }\n",
    )
    .unwrap();
    let index = fixture.root.join("index");
    build_rag_index(std::slice::from_ref(&source), &index, 512).unwrap();
    let baseline_generation = fs::read_to_string(index.join(RAG_CURRENT_FILE)).unwrap();
    fs::write(
        &source_file,
        "class Marker { String value = \"replacement_marker\"; }\n",
    )
    .unwrap();

    for fault in [
        RagFaultPoint::DuringScan,
        RagFaultPoint::DuringDocumentsWrite,
        RagFaultPoint::DuringChunksWrite,
        RagFaultPoint::BeforeManifestFinalization,
        RagFaultPoint::DuringPublication,
    ] {
        let error = build_rag_index_with_fault(std::slice::from_ref(&source), &index, 512, fault)
            .expect_err("fault injection should interrupt index construction");
        assert!(format!("{error:#}").contains("injected RAG fault"));
        assert_eq!(
            fs::read_to_string(index.join(RAG_CURRENT_FILE)).unwrap(),
            baseline_generation
        );
        assert_eq!(
            search_rag_index(&index, "baseline_marker", 10)
                .unwrap()
                .len(),
            1
        );
        assert!(search_rag_index(&index, "replacement_marker", 10)
            .unwrap()
            .is_empty());
    }

    let report = build_rag_index(std::slice::from_ref(&source), &index, 512).unwrap();
    assert_ne!(report.generation_id.trim(), baseline_generation.trim());
    assert_eq!(
        search_rag_index(&index, "replacement_marker", 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn truncated_staging_is_recovered_without_touching_the_active_index() {
    let fixture = TemporaryFixture::new("recovery");
    let source = fixture.directory("source");
    fs::write(source.join("Safe.java"), "class Safe {}\n").unwrap();
    let index = fixture.root.join("index");
    build_rag_index(std::slice::from_ref(&source), &index, 512).unwrap();
    let old_current = fs::read_to_string(index.join(RAG_CURRENT_FILE)).unwrap();
    let truncated = index.join(".staging-g-dead-beef-0");
    fs::create_dir(&truncated).unwrap();
    fs::write(truncated.join(RAG_DOCUMENTS_FILE), b"{\"truncated\":").unwrap();
    assert_eq!(search_rag_index(&index, "Safe", 1).unwrap().len(), 1);

    let report = build_rag_index(std::slice::from_ref(&source), &index, 512).unwrap();
    assert_eq!(report.recovered_staging_directories, 1);
    assert!(!truncated.exists());
    assert_ne!(
        fs::read_to_string(index.join(RAG_CURRENT_FILE)).unwrap(),
        old_current
    );
    assert_eq!(search_rag_index(&index, "Safe", 1).unwrap().len(), 1);
}

#[test]
fn recovery_never_deletes_an_unrecognized_staging_directory() {
    let fixture = TemporaryFixture::new("recovery-ownership");
    let source = fixture.directory("source");
    fs::write(source.join("Safe.java"), "class Safe {}\n").unwrap();
    let index = fixture.root.join("index");
    build_rag_index(std::slice::from_ref(&source), &index, 512).unwrap();

    let unrelated = index.join(".staging-user-notes");
    fs::create_dir(&unrelated).unwrap();
    fs::write(unrelated.join("keep.txt"), "must remain\n").unwrap();

    let error = build_rag_index(std::slice::from_ref(&source), &index, 512)
        .expect_err("an ambiguous staging directory must require manual inspection");
    assert!(error.to_string().contains("manual inspection"));
    assert!(unrelated.join("keep.txt").is_file());
    assert_eq!(search_rag_index(&index, "Safe", 1).unwrap().len(), 1);
}

#[cfg(windows)]
#[test]
fn scan_refuses_windows_junctions_and_a_reparse_root() {
    use std::process::Command;

    let fixture = TemporaryFixture::new("junction");
    let source = fixture.directory("source");
    let external = fixture.directory("external");
    fs::write(source.join("Safe.java"), "class Safe {}\n").unwrap();
    fs::write(external.join("Outside.java"), "class Outside {}\n").unwrap();
    let junction = source.join("linked");
    let output = Command::new("cmd")
        .args(["/D", "/C", "mklink", "/J"])
        .arg(&junction)
        .arg(&external)
        .output()
        .expect("junction command should start");
    assert!(
        output.status.success(),
        "junction should be created: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let report = inspect_rag_source(&source, 20).unwrap();
    assert_eq!(report.indexable_files, 1);
    assert!(report.exclusions.iter().any(|entry| {
        entry.relative_path == "linked" && entry.rule_id == "path.symlink_or_reparse"
    }));
    let root_error = inspect_rag_source(&junction, 20).unwrap_err();
    assert!(format!("{root_error:#}").contains("symlink or reparse point"));

    fs::remove_dir(&junction).expect("junction should be removed without touching its target");
}

fn active_generation_dir(index: &Path) -> PathBuf {
    let generation = fs::read_to_string(index.join(RAG_CURRENT_FILE)).unwrap();
    index.join(RAG_GENERATIONS_DIR).join(generation.trim())
}

fn read_manifest(index: &Path) -> RagIndexManifest {
    serde_json::from_slice(&fs::read(active_generation_dir(index).join(RAG_MANIFEST_FILE)).unwrap())
        .unwrap()
}
