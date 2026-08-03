use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::*;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[test]
fn retrieval_metrics_cover_hit_recall_mrr_ndcg_and_duplicates() {
    let expected = EvalExpected {
        relevant_files: vec!["src/A.java".to_string(), "src/B.java".to_string()],
        ..EvalExpected::default()
    };
    let retrieved = vec![
        item(1, "src/Noise.java"),
        item(2, "src/A.java"),
        item(3, "src/A.java"),
        item(4, "src/B.java"),
    ];

    let metrics = calculate_retrieval_metrics(&expected, &retrieved);

    assert_eq!(metrics.hit_at_1, Some(false));
    assert_eq!(metrics.hit_at_3, Some(true));
    assert_eq!(metrics.hit_at_5, Some(true));
    assert_eq!(metrics.recall_at_1, Some(0.0));
    assert_eq!(metrics.recall_at_3, Some(0.5));
    assert_eq!(metrics.recall_at_k, Some(1.0));
    assert_eq!(metrics.reciprocal_rank, Some(0.5));
    assert_eq!(metrics.first_relevant_rank, Some(2));
    assert_eq!(metrics.duplicates, 1);
    assert_eq!(metrics.unique_files, 3);
    assert_eq!(metrics.out_of_scope_results, 1);
    assert!(metrics
        .ndcg_at_5
        .is_some_and(|value| value > 0.5 && value < 1.0));
}

#[test]
fn retrieval_metrics_handle_multiple_units_and_no_relevant_results() {
    let expected = EvalExpected {
        relevant_files: vec!["Plugin.java".to_string()],
        relevant_symbols: vec!["dev.example.Plugin#onEnable()".to_string()],
        ..EvalExpected::default()
    };
    let retrieved = vec![EvalRetrievedItem {
        symbol: Some("dev.example.Plugin#onEnable()".to_string()),
        ..item(1, "src/main/java/dev/example/Plugin.java")
    }];
    let metrics = calculate_retrieval_metrics(&expected, &retrieved);
    assert_eq!(metrics.relevant_expected, 1);
    assert_eq!(metrics.relevant_found_at_k, 1);
    assert_eq!(metrics.recall_at_k, Some(1.0));
    assert_eq!(metrics.ndcg_at_5, Some(1.0));

    let no_expectation = calculate_retrieval_metrics(&EvalExpected::default(), &retrieved);
    assert_eq!(no_expectation.hit_at_1, None);
    assert_eq!(no_expectation.recall_at_k, None);
    assert_eq!(no_expectation.reciprocal_rank, None);
    assert_eq!(no_expectation.ndcg_at_5, None);
}

#[test]
fn percentile_uses_nearest_rank_for_p50_and_p95() {
    let mut values = vec![100, 20, 40, 80, 60];
    assert_eq!(percentile(&mut values, 0.50), 60);
    assert_eq!(percentile(&mut values, 0.95), 100);
    assert_eq!(percentile(&mut [], 0.95), 0);
}

#[test]
fn invalid_corpus_is_rejected() {
    let mut suite = tiny_suite("fixture");
    suite.schema_version = 999;
    assert!(validate_eval_suite(&suite)
        .unwrap_err()
        .to_string()
        .contains("unsupported evaluation suite schema"));

    let mut suite = tiny_suite("fixture");
    suite.cases.push(suite.cases[0].clone());
    assert!(validate_eval_suite(&suite)
        .unwrap_err()
        .to_string()
        .contains("duplicate evaluation case id"));

    let mut suite = tiny_suite("fixture");
    suite.fixtures.insert(
        "fixture".to_string(),
        EvalFixture::Versioned {
            path: std::env::current_dir()
                .unwrap()
                .to_string_lossy()
                .to_string(),
        },
    );
    assert!(validate_eval_suite(&suite)
        .unwrap_err()
        .to_string()
        .contains("must use a non-empty relative path"));
}

#[test]
fn versioned_corpus_has_45_balanced_cases_and_no_personal_paths() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../benchmarks/eval/context-retrieval-v1.json");
    let suite = load_eval_suite(&path).unwrap();
    assert_eq!(suite.cases.len(), 45);
    let mut categories = BTreeMap::new();
    for case in &suite.cases {
        *categories.entry(case.category).or_insert(0usize) += 1;
    }
    for category in [
        EvalCategory::ExactSymbol,
        EvalCategory::InterFileArchitecture,
        EvalCategory::ChangeImpactCallers,
        EvalCategory::ProjectConfiguration,
        EvalCategory::LegacyAndNegative,
    ] {
        assert_eq!(categories.get(&category), Some(&9));
    }
    let encoded = serde_json::to_string(&suite).unwrap().to_ascii_lowercase();
    assert!(!encoded.contains("c:\\users\\"));
    assert!(!encoded.contains("desktop\\minecraft"));
}

#[test]
fn runner_is_read_only_deterministic_and_supports_unicode_space_paths() {
    let temp = TempDirectory::new("eval unicode space");
    let fixture = temp.path.join("fixture espace é");
    fs::create_dir_all(fixture.join("src/main/java/dev/example")).unwrap();
    let source = fixture.join("src/main/java/dev/example/Plugin.java");
    fs::write(
        &source,
        "package dev.example;\npublic final class Plugin { void onEnable() {} }\n",
    )
    .unwrap();
    let suite_path = temp.path.join("suite.json");
    write_suite(&suite_path, &tiny_suite("fixture espace é"));
    let original = fs::read(&source).unwrap();
    let options = EvalRunOptions {
        strategies: vec![EvalStrategy::Exact],
        ..EvalRunOptions::default()
    };

    let first = run_evaluation(&suite_path, options.clone()).unwrap();
    let second = run_evaluation(&suite_path, options).unwrap();

    assert_eq!(first.results.len(), 1);
    assert_eq!(first.results[0].status, EvalCaseStatus::Completed);
    assert_eq!(first.results[0].observed, second.results[0].observed);
    assert_eq!(fs::read(&source).unwrap(), original);
}

#[test]
fn absent_external_fixture_is_skipped_cleanly() {
    let temp = TempDirectory::new("eval external");
    let suite = EvalSuite {
        schema_version: EVAL_SUITE_SCHEMA_VERSION,
        id: "external-suite".to_string(),
        version: "1".to_string(),
        description: "External fixture skip test".to_string(),
        fixtures: BTreeMap::from([(
            "panda".to_string(),
            EvalFixture::External {
                external_id: "pandaspigot".to_string(),
                description: "Read-only PandaSpigot checkout".to_string(),
            },
        )]),
        cases: vec![EvalCase {
            id: "external-missing".to_string(),
            category: EvalCategory::InterFileArchitecture,
            prompt: "Find the server bootstrap".to_string(),
            fixture: "panda".to_string(),
            exact_query: Some("MinecraftServer".to_string()),
            expected: EvalExpected::default(),
            validation: None,
            tags: vec!["external".to_string()],
            context_budget: EvalContextBudget::default(),
            reason: "Exercises optional checkout behavior".to_string(),
        }],
    };
    let suite_path = temp.path.join("suite.json");
    write_suite(&suite_path, &suite);

    let report = run_evaluation(
        &suite_path,
        EvalRunOptions {
            strategies: vec![EvalStrategy::Symbol],
            ..EvalRunOptions::default()
        },
    )
    .unwrap();

    assert_eq!(report.summary.skipped, 1);
    assert_eq!(report.results[0].status, EvalCaseStatus::Skipped);
    assert!(report.results[0]
        .skip_reason
        .as_deref()
        .is_some_and(|reason| reason.contains("was not provided")));
}

#[test]
fn case_limit_is_explicitly_reported_as_suite_truncation() {
    let temp = TempDirectory::new("eval truncation");
    let fixture = temp.path.join("fixture");
    fs::create_dir_all(&fixture).unwrap();
    fs::write(fixture.join("A.java"), "class A {}\n").unwrap();
    let mut suite = tiny_suite("fixture");
    let mut second = suite.cases[0].clone();
    second.id = "tiny-2".to_string();
    suite.cases.push(second);
    let suite_path = temp.path.join("suite.json");
    write_suite(&suite_path, &suite);

    let report = run_evaluation(
        &suite_path,
        EvalRunOptions {
            strategies: vec![EvalStrategy::Exact],
            case_limit: Some(1),
            ..EvalRunOptions::default()
        },
    )
    .unwrap();

    assert!(report.configuration.suite_truncated);
    assert_eq!(report.summary.case_count, 1);
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.contains("suite truncated")));
}

#[test]
fn baseline_comparison_detects_quality_regression() {
    let mut baseline = empty_report("base");
    baseline.summary.strategies = vec![strategy_summary(0.9, 100.0, 1_000)];
    let mut candidate = empty_report("candidate");
    candidate.summary.strategies = vec![strategy_summary(0.6, 110.0, 1_100)];

    let comparison = compare_reports(&baseline, &candidate);

    assert!(comparison.comparable);
    assert!(comparison
        .regressions
        .iter()
        .any(|regression| regression.metric == "hit_at_5"));
}

#[test]
fn reports_round_trip_as_json_and_markdown() {
    let temp = TempDirectory::new("eval reports");
    let report = empty_report("roundtrip");

    let paths = write_eval_reports(&report, &temp.path).unwrap();
    let loaded = load_eval_report(&paths.json).unwrap();
    let markdown = fs::read_to_string(&paths.markdown).unwrap();

    assert_eq!(loaded.run_id, report.run_id);
    assert!(markdown.contains("# OpticCode evaluation `roundtrip`"));
    assert!(markdown.contains("## Exact configuration"));
}

fn item(rank: usize, path: &str) -> EvalRetrievedItem {
    EvalRetrievedItem {
        rank,
        path: path.to_string(),
        symbol: None,
        score: None,
        source: "test".to_string(),
        bytes: 1,
        chars: 1,
        estimated_tokens: 1,
        truncated: false,
        content_hash: None,
    }
}

fn tiny_suite(fixture_path: &str) -> EvalSuite {
    EvalSuite {
        schema_version: EVAL_SUITE_SCHEMA_VERSION,
        id: "tiny-suite".to_string(),
        version: "1".to_string(),
        description: "Small deterministic evaluation suite".to_string(),
        fixtures: BTreeMap::from([(
            "fixture".to_string(),
            EvalFixture::Versioned {
                path: fixture_path.to_string(),
            },
        )]),
        cases: vec![EvalCase {
            id: "tiny-1".to_string(),
            category: EvalCategory::ExactSymbol,
            prompt: "Find Plugin".to_string(),
            fixture: "fixture".to_string(),
            exact_query: Some("Plugin".to_string()),
            expected: EvalExpected {
                relevant_files: vec!["src/main/java/dev/example/Plugin.java".to_string()],
                relevant_symbols: Vec::new(),
                irrelevant_files: Vec::new(),
                facts: vec!["Plugin exists".to_string()],
                forbidden_claims: Vec::new(),
            },
            validation: Some(EvalValidation {
                kind: EvalValidationKind::ReadOnly,
                command: None,
                expected_files_unchanged: true,
            }),
            tags: vec!["tiny".to_string()],
            context_budget: EvalContextBudget::default(),
            reason: "Exercises exact retrieval".to_string(),
        }],
    }
}

fn write_suite(path: &Path, suite: &EvalSuite) {
    fs::write(path, serde_json::to_vec_pretty(suite).unwrap()).unwrap();
}

fn empty_report(run_id: &str) -> EvalRunReport {
    EvalRunReport {
        schema_version: EVAL_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        opticcode_version: "0.1.0".to_string(),
        git_commit: None,
        suite_id: "suite".to_string(),
        suite_version: "1".to_string(),
        configuration: EvalConfiguration {
            strategies: vec![EvalStrategy::Symbol],
            repetitions: 1,
            case_limit: None,
            suite_truncated: false,
            llm_mode: EvalLlmMode::Disabled,
            context: EvalContextConfiguration {
                legacy_max_files: 8,
                legacy_max_bytes_per_file: 4096,
                symbol_max_files: 12,
                symbol_max_snippets: 12,
                symbol_max_chars: 24_576,
                symbol_max_estimated_tokens: 6_144,
                exact_limit: 12,
            },
            rag: EvalRagConfiguration {
                enabled: false,
                index_label: "none".to_string(),
                limit: 8,
            },
            generation: None,
        },
        configuration_hash: "hash".to_string(),
        rag_identity: None,
        started_at_unix_ms: 1,
        duration_us: 1,
        environment: EvalEnvironment {
            os: "windows".to_string(),
            arch: "x86_64".to_string(),
            family: "windows".to_string(),
            logical_cpus: 1,
        },
        results: Vec::new(),
        summary: EvalSummary::default(),
        baseline: None,
        warnings: Vec::new(),
    }
}

fn strategy_summary(hit_at_5: f64, tokens: f64, latency: u64) -> EvalStrategySummary {
    EvalStrategySummary {
        strategy: EvalStrategy::Symbol,
        completed: 1,
        skipped: 0,
        failed: 0,
        hit_at_1: Some(hit_at_5),
        hit_at_3: Some(hit_at_5),
        hit_at_5: Some(hit_at_5),
        mean_recall_at_k: Some(hit_at_5),
        mean_reciprocal_rank: Some(hit_at_5),
        mean_ndcg_at_5: Some(hit_at_5),
        mean_estimated_tokens: tokens,
        latency_p50_us: latency,
        latency_p95_us: latency,
        analysis_complete_rate: 1.0,
    }
}

struct TempDirectory {
    path: PathBuf,
}

impl TempDirectory {
    fn new(label: &str) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "opticcode-{label}-{}-{timestamp}-{counter}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let temp = std::env::temp_dir();
        assert!(self.path.starts_with(&temp));
        let _ = fs::remove_dir_all(&self.path);
    }
}
