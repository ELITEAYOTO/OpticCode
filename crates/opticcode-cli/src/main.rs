use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use opticcode_core::{
    load_memory_for_workspace, load_profile_for_workspace, load_rag_context, parse_keep_alive,
    AskOptions, GenerateMetrics, OpticCode, PlanOptions, RagContext, DEFAULT_PROFILE,
};
use opticcode_tools::apply_transaction::{
    apply_transaction_error_kind, inspect_apply_transaction, list_apply_transactions,
    recover_apply_transaction, ApplyTransactionErrorKind, ApplyTransactionInspection,
    ApplyTransactionResult,
};
use opticcode_tools::eval::{
    compare_reports, load_eval_report, run_evaluation, write_eval_reports,
    EvalGenerationConfiguration, EvalLlmMode, EvalRunOptions, EvalStrategy,
};
use opticcode_tools::git_state::capture_git_state;
use opticcode_tools::java_context::{
    build_java_task_context, JavaTaskContextOptions, DEFAULT_JAVA_CONTEXT_BYTES,
    DEFAULT_JAVA_CONTEXT_CALLER_LIMIT, DEFAULT_JAVA_CONTEXT_CANDIDATE_LIMIT,
    DEFAULT_JAVA_CONTEXT_CHARS, DEFAULT_JAVA_CONTEXT_IDENTIFIER_LIMIT,
    DEFAULT_JAVA_CONTEXT_PRIMARY_SYMBOL_LIMIT, DEFAULT_JAVA_CONTEXT_RELATED_LIMIT,
    DEFAULT_JAVA_CONTEXT_RELATION_DEPTH, DEFAULT_JAVA_CONTEXT_RELATION_LIMIT,
    DEFAULT_JAVA_CONTEXT_SNIPPET_BYTES, DEFAULT_JAVA_CONTEXT_SNIPPET_LIMIT,
    DEFAULT_JAVA_CONTEXT_SYMBOL_VISIT_LIMIT, DEFAULT_JAVA_CONTEXT_TERM_LIMIT,
    DEFAULT_JAVA_CONTEXT_TOKEN_LIMIT, DEFAULT_JAVA_CONTEXT_WARNING_LIMIT,
};
use opticcode_tools::java_edit_worktree::{
    java_edit_worktree_error_kind, verify_java_edits_in_worktree, JavaEditWorktreeErrorKind,
    JavaEditWorktreeOptions, JavaEditWorktreeReport,
};
use opticcode_tools::java_edits::{
    legacy_rule_catalog, propose_java_edits, JavaEditOptions, DEFAULT_JAVA_EDIT_PROPOSAL_LIMIT,
};
use opticcode_tools::java_index::{
    analyze_java_index, JavaIndexOptions, DEFAULT_JAVA_INDEX_CANDIDATE_LIMIT,
    DEFAULT_JAVA_INDEX_REFERENCE_LIMIT, DEFAULT_JAVA_INDEX_SYMBOL_LIMIT,
};
use opticcode_tools::java_syntax::{
    analyze_java_syntax, JavaSyntaxOptions, DEFAULT_JAVA_SYNTAX_FILE_BYTES,
    DEFAULT_JAVA_SYNTAX_FILE_LIMIT, DEFAULT_JAVA_SYNTAX_ITEM_LIMIT,
};
use opticcode_tools::process_runner::{
    CancellationToken, ProcessOutputStats, ProcessStatus, ProcessTermination,
    DEFAULT_PROCESS_OUTPUT_LIMIT_BYTES, DEFAULT_PROCESS_TIMEOUT_SECONDS,
};
use opticcode_tools::worktree::{
    cleanup_disposable_worktree, list_disposable_worktrees, verify_java_legacy_patch_in_worktree,
    worktree_operation_error_kind, WorktreeCleanupReport, WorktreeLeaseInspection,
    WorktreeOperationErrorKind, WorktreeVerificationOptions, WorktreeVerificationReport,
    WorktreeVerificationStatus, DEFAULT_WORKTREE_GIT_TIMEOUT_SECONDS,
};
use opticcode_tools::{
    analyze_java_project, apply_java_legacy_patch_in_place_with_options,
    apply_java_legacy_patch_to_copy, build_java_project_with_cancellation, build_project_context,
    build_rag_index, check_patch_with_git, inspect_rag_source, inspect_resource_pack,
    inspect_workspace, prepare_java_legacy_apply_plan, propose_java_legacy_patch,
    search_rag_index_report, search_workspace, undo_apply_run, ApplyLogEntry, ApplyPlan,
    ApplyUndoResult, BuildOptions, BuildResult, PatchCheckResult, RagSourceReport,
};
use serde::Serialize;
use std::io::{self, Write};

#[derive(Debug, Parser)]
#[command(name = "opticcode")]
#[command(about = "Local code assistant focused on Java 8 / Bukkit 1.8.8 projects.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Parser)]
#[command(name = "opticcode java-context")]
#[command(about = "Select bounded Java context for a concrete coding task.")]
struct JavaContextArgs {
    task: String,
    #[arg(long, default_value = ".")]
    path: PathBuf,
    #[arg(long, default_value_t = DEFAULT_JAVA_SYNTAX_FILE_LIMIT)]
    limit: usize,
    #[arg(long, default_value_t = DEFAULT_JAVA_SYNTAX_FILE_BYTES)]
    max_file_bytes: u64,
    #[arg(long, default_value_t = DEFAULT_JAVA_SYNTAX_ITEM_LIMIT)]
    item_limit: usize,
    #[arg(long, default_value_t = DEFAULT_JAVA_INDEX_SYMBOL_LIMIT)]
    symbol_limit: usize,
    #[arg(long, default_value_t = DEFAULT_JAVA_INDEX_REFERENCE_LIMIT)]
    reference_limit: usize,
    #[arg(long, default_value_t = DEFAULT_JAVA_INDEX_CANDIDATE_LIMIT)]
    candidate_limit: usize,
    #[arg(long, default_value_t = DEFAULT_JAVA_CONTEXT_IDENTIFIER_LIMIT)]
    identifier_limit: usize,
    #[arg(long, default_value_t = DEFAULT_JAVA_CONTEXT_TERM_LIMIT)]
    term_limit: usize,
    #[arg(long, default_value_t = DEFAULT_JAVA_CONTEXT_SYMBOL_VISIT_LIMIT)]
    visited_symbol_limit: usize,
    #[arg(long, default_value_t = DEFAULT_JAVA_CONTEXT_PRIMARY_SYMBOL_LIMIT)]
    primary_symbol_limit: usize,
    #[arg(long, default_value_t = DEFAULT_JAVA_CONTEXT_CANDIDATE_LIMIT)]
    selection_limit: usize,
    #[arg(long, default_value_t = DEFAULT_JAVA_CONTEXT_RELATION_LIMIT)]
    relation_limit: usize,
    #[arg(long, default_value_t = DEFAULT_JAVA_CONTEXT_RELATION_DEPTH)]
    relation_depth: usize,
    #[arg(long, default_value_t = DEFAULT_JAVA_CONTEXT_SNIPPET_LIMIT)]
    snippet_limit: usize,
    #[arg(long, default_value_t = DEFAULT_JAVA_CONTEXT_SNIPPET_BYTES)]
    snippet_bytes: usize,
    #[arg(long, default_value_t = DEFAULT_JAVA_CONTEXT_BYTES)]
    context_bytes: usize,
    #[arg(long, default_value_t = DEFAULT_JAVA_CONTEXT_CHARS)]
    context_chars: usize,
    #[arg(long, default_value_t = DEFAULT_JAVA_CONTEXT_TOKEN_LIMIT)]
    context_tokens: usize,
    #[arg(long, default_value_t = DEFAULT_JAVA_CONTEXT_WARNING_LIMIT)]
    warning_limit: usize,
    #[arg(long, default_value_t = DEFAULT_JAVA_CONTEXT_CALLER_LIMIT)]
    caller_limit: usize,
    #[arg(long, default_value_t = DEFAULT_JAVA_CONTEXT_RELATED_LIMIT)]
    related_limit: usize,
    #[arg(long)]
    compare_baseline: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
#[command(name = "opticcode eval")]
#[command(about = "Run reproducible read-only context and retrieval evaluation.")]
struct EvalArgs {
    #[arg(long, default_value = "benchmarks/eval/context-retrieval-v1.json")]
    suite: PathBuf,
    #[arg(
        long = "strategy",
        value_delimiter = ',',
        default_value = "legacy,symbol,exact"
    )]
    strategies: Vec<EvalStrategy>,
    #[arg(long, default_value_t = 1)]
    repetitions: u32,
    #[arg(long)]
    case_limit: Option<usize>,
    #[arg(long)]
    baseline: Option<PathBuf>,
    #[arg(long)]
    compare: Option<PathBuf>,
    #[arg(long, requires = "compare")]
    candidate: Option<PathBuf>,
    #[arg(long, default_value = "data/index")]
    rag_index: PathBuf,
    #[arg(long, default_value_t = 8)]
    rag_limit: usize,
    #[arg(long)]
    no_rag: bool,
    #[arg(long = "external", value_name = "ID=PATH")]
    external_fixtures: Vec<String>,
    #[arg(long)]
    with_llm: bool,
    #[arg(long, default_value = "qwen2.5-coder:14b")]
    model: String,
    #[arg(long)]
    temperature: Option<f32>,
    #[arg(long)]
    seed: Option<i64>,
    #[arg(long, default_value_t = 256)]
    max_generated_tokens: u32,
    #[arg(long, default_value_t = 120_000)]
    http_timeout_ms: u64,
    #[arg(long, default_value_t = 0)]
    warmup_runs: u32,
    #[arg(long, default_value = "benchmarks/runs/eval")]
    reports_dir: PathBuf,
    #[arg(long)]
    no_write_reports: bool,
    #[arg(long)]
    fail_on_regression: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum Command {
    Inspect {
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
    GitState {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    Search {
        pattern: String,
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    Context {
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
    /// Select bounded Java context for a concrete coding task.
    JavaContext,
    /// Run reproducible context and retrieval evaluation.
    Eval,
    AnalyzeJava {
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
    JavaSyntax {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long, default_value_t = DEFAULT_JAVA_SYNTAX_FILE_LIMIT)]
        limit: usize,
        #[arg(long, default_value_t = DEFAULT_JAVA_SYNTAX_FILE_BYTES)]
        max_file_bytes: u64,
        #[arg(long, default_value_t = DEFAULT_JAVA_SYNTAX_ITEM_LIMIT)]
        item_limit: usize,
        #[arg(long)]
        json: bool,
    },
    JavaIndex {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long, default_value_t = DEFAULT_JAVA_SYNTAX_FILE_LIMIT)]
        limit: usize,
        #[arg(long, default_value_t = DEFAULT_JAVA_SYNTAX_FILE_BYTES)]
        max_file_bytes: u64,
        #[arg(long, default_value_t = DEFAULT_JAVA_SYNTAX_ITEM_LIMIT)]
        item_limit: usize,
        #[arg(long, default_value_t = DEFAULT_JAVA_INDEX_SYMBOL_LIMIT)]
        symbol_limit: usize,
        #[arg(long, default_value_t = DEFAULT_JAVA_INDEX_REFERENCE_LIMIT)]
        reference_limit: usize,
        #[arg(long, default_value_t = DEFAULT_JAVA_INDEX_CANDIDATE_LIMIT)]
        candidate_limit: usize,
        #[arg(long)]
        json: bool,
    },
    JavaEdits {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long, default_value_t = DEFAULT_JAVA_SYNTAX_FILE_LIMIT)]
        limit: usize,
        #[arg(long, default_value_t = DEFAULT_JAVA_SYNTAX_FILE_BYTES)]
        max_file_bytes: u64,
        #[arg(long, default_value_t = DEFAULT_JAVA_SYNTAX_ITEM_LIMIT)]
        item_limit: usize,
        #[arg(long, default_value_t = DEFAULT_JAVA_INDEX_SYMBOL_LIMIT)]
        symbol_limit: usize,
        #[arg(long, default_value_t = DEFAULT_JAVA_INDEX_REFERENCE_LIMIT)]
        reference_limit: usize,
        #[arg(long, default_value_t = DEFAULT_JAVA_INDEX_CANDIDATE_LIMIT)]
        candidate_limit: usize,
        #[arg(long, default_value_t = DEFAULT_JAVA_EDIT_PROPOSAL_LIMIT)]
        proposal_limit: usize,
        #[arg(long)]
        json: bool,
    },
    JavaLegacyRules {
        #[arg(long)]
        json: bool,
    },
    JavaEditsVerify {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long, default_value_t = DEFAULT_JAVA_SYNTAX_FILE_LIMIT)]
        limit: usize,
        #[arg(long, default_value_t = DEFAULT_JAVA_SYNTAX_FILE_BYTES)]
        max_file_bytes: u64,
        #[arg(long, default_value_t = DEFAULT_JAVA_SYNTAX_ITEM_LIMIT)]
        item_limit: usize,
        #[arg(long, default_value_t = DEFAULT_JAVA_INDEX_SYMBOL_LIMIT)]
        symbol_limit: usize,
        #[arg(long, default_value_t = DEFAULT_JAVA_INDEX_REFERENCE_LIMIT)]
        reference_limit: usize,
        #[arg(long, default_value_t = DEFAULT_JAVA_INDEX_CANDIDATE_LIMIT)]
        candidate_limit: usize,
        #[arg(long, default_value_t = DEFAULT_JAVA_EDIT_PROPOSAL_LIMIT)]
        proposal_limit: usize,
        #[arg(long, default_value_t = DEFAULT_PROCESS_TIMEOUT_SECONDS)]
        timeout_seconds: u64,
        #[arg(long, default_value_t = DEFAULT_WORKTREE_GIT_TIMEOUT_SECONDS)]
        git_timeout_seconds: u64,
        #[arg(long, default_value_t = DEFAULT_PROCESS_OUTPUT_LIMIT_BYTES)]
        output_limit_bytes: usize,
        #[arg(long)]
        json: bool,
    },
    Build {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        fail_on_worktree_change: bool,
        #[arg(long, default_value_t = DEFAULT_PROCESS_TIMEOUT_SECONDS)]
        timeout_seconds: u64,
        #[arg(long, default_value_t = DEFAULT_PROCESS_OUTPUT_LIMIT_BYTES)]
        output_limit_bytes: usize,
        #[arg(long)]
        json: bool,
    },
    Profile {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long, default_value = DEFAULT_PROFILE)]
        profile: String,
    },
    Memory {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long, default_value = DEFAULT_PROFILE)]
        profile: String,
    },
    PackScan {
        #[arg(long)]
        path: PathBuf,
        #[arg(long, default_value_t = 60)]
        limit: usize,
    },
    RagScan {
        #[arg(long = "path", required = true)]
        paths: Vec<PathBuf>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    RagIndex {
        #[arg(long = "path", required = true)]
        paths: Vec<PathBuf>,
        #[arg(long, default_value = "data/index")]
        output: PathBuf,
        #[arg(long, default_value_t = 4000)]
        chunk_chars: usize,
        #[arg(long)]
        json: bool,
    },
    RagSearch {
        query: String,
        #[arg(long, default_value = "data/index")]
        index: PathBuf,
        #[arg(long, default_value_t = 8)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    RagDebug {
        query: String,
        #[arg(long, default_value = "data/index")]
        index: PathBuf,
        #[arg(long, default_value_t = 4)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    Patch {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        check: bool,
    },
    Apply {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        copy_to: Option<PathBuf>,
        #[arg(long)]
        undo: Option<String>,
        #[arg(long)]
        allow_external: bool,
        #[arg(long)]
        allow_dirty: bool,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        json: bool,
    },
    Transactions {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        inspect: Option<String>,
        #[arg(long)]
        recover: Option<String>,
        #[arg(long)]
        allow_external: bool,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        json: bool,
    },
    WorktreeVerify {
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long, default_value_t = DEFAULT_PROCESS_TIMEOUT_SECONDS)]
        timeout_seconds: u64,
        #[arg(long, default_value_t = DEFAULT_WORKTREE_GIT_TIMEOUT_SECONDS)]
        git_timeout_seconds: u64,
        #[arg(long, default_value_t = DEFAULT_PROCESS_OUTPUT_LIMIT_BYTES)]
        output_limit_bytes: usize,
        #[arg(long)]
        json: bool,
    },
    Worktrees {
        #[arg(long)]
        cleanup: Option<String>,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        json: bool,
    },
    Ask {
        prompt: String,
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long, default_value = "qwen2.5-coder:14b")]
        model: String,
        #[arg(long, default_value = "http://localhost:11434")]
        ollama_url: String,
        #[arg(long, default_value = "15m")]
        keep_alive: String,
        #[arg(long, default_value = DEFAULT_PROFILE)]
        profile: String,
        #[arg(long)]
        no_memory: bool,
        #[arg(long)]
        no_rag: bool,
        #[arg(long, default_value = "data/index")]
        rag_index: PathBuf,
        #[arg(long, default_value_t = 4)]
        rag_limit: usize,
        #[arg(long)]
        rag_debug: bool,
        #[arg(long)]
        brief: bool,
        #[arg(long)]
        max_tokens: Option<u32>,
        #[arg(long)]
        metrics: bool,
        #[arg(long)]
        metrics_json: bool,
    },
    Plan {
        goal: String,
        #[arg(long, default_value = ".")]
        path: PathBuf,
        #[arg(long, default_value = "qwen2.5-coder:14b")]
        model: String,
        #[arg(long, default_value = "http://localhost:11434")]
        ollama_url: String,
        #[arg(long, default_value = "15m")]
        keep_alive: String,
        #[arg(long, default_value = DEFAULT_PROFILE)]
        profile: String,
        #[arg(long)]
        no_memory: bool,
        #[arg(long)]
        no_rag: bool,
        #[arg(long, default_value = "data/index")]
        rag_index: PathBuf,
        #[arg(long, default_value_t = 4)]
        rag_limit: usize,
        #[arg(long)]
        rag_debug: bool,
        #[arg(long)]
        brief: bool,
        #[arg(long)]
        max_tokens: Option<u32>,
        #[arg(long)]
        metrics: bool,
        #[arg(long)]
        metrics_json: bool,
    },
}

fn main() -> Result<()> {
    if let Some(args) = parse_isolated_java_context() {
        return run_java_context(args);
    }
    if let Some(args) = parse_isolated_eval() {
        return run_eval(args);
    }
    let cli = Cli::parse();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(Box::pin(run_command(cli.command)))
}

fn parse_isolated_eval() -> Option<EvalArgs> {
    let mut args = std::env::args_os();
    args.next()?;
    let first = args.next()?;
    let mut parser_args = vec![std::ffi::OsString::from("opticcode eval")];
    if first == std::ffi::OsStr::new("eval") {
        parser_args.extend(args);
    } else if first == std::ffi::OsStr::new("help")
        && args.next().as_deref() == Some(std::ffi::OsStr::new("eval"))
    {
        parser_args.push(std::ffi::OsString::from("--help"));
        parser_args.extend(args);
    } else {
        return None;
    }
    Some(EvalArgs::parse_from(parser_args))
}

fn run_eval(args: EvalArgs) -> Result<()> {
    if let Some(baseline_path) = args.compare.as_deref() {
        let candidate_path = args
            .candidate
            .as_deref()
            .context("eval --compare requires --candidate")?;
        let baseline = load_eval_report(baseline_path)?;
        let candidate = load_eval_report(candidate_path)?;
        let comparison = compare_reports(&baseline, &candidate);
        if args.json {
            println!("{}", serde_json::to_string_pretty(&comparison)?);
        } else {
            println!("Evaluation comparison");
            println!("- baseline: {}", comparison.baseline_run_id);
            println!("- candidate: {}", comparison.candidate_run_id);
            println!("- comparable: {}", comparison.comparable);
            println!("- regressions: {}", comparison.regressions.len());
            println!("- improvements: {}", comparison.improvements.len());
        }
        io::stdout().flush()?;
        if !comparison.comparable || (args.fail_on_regression && !comparison.regressions.is_empty())
        {
            std::process::exit(2);
        }
        return Ok(());
    }

    let external_fixtures = parse_external_fixtures(&args.external_fixtures)?;
    let baseline = args.baseline.as_deref().map(load_eval_report).transpose()?;
    let generation = args.with_llm.then(|| EvalGenerationConfiguration {
        provider: "ollama".to_string(),
        model: args.model,
        temperature: args.temperature,
        seed: args.seed,
        max_generated_tokens: args.max_generated_tokens,
        http_timeout_ms: args.http_timeout_ms,
        warmup_runs: args.warmup_runs,
    });
    let rag_requested = !args.no_rag && args.strategies.contains(&EvalStrategy::Rag);
    let rag_index_label = if !rag_requested {
        "disabled".to_string()
    } else {
        sanitized_eval_path_label(&args.rag_index)
    };
    let report = run_evaluation(
        &args.suite,
        EvalRunOptions {
            strategies: args.strategies,
            repetitions: args.repetitions,
            case_limit: args.case_limit,
            rag_index: rag_requested.then_some(args.rag_index),
            rag_index_label,
            rag_limit: args.rag_limit,
            external_fixtures,
            llm_mode: if args.with_llm {
                EvalLlmMode::Enabled
            } else {
                EvalLlmMode::Disabled
            },
            generation,
            baseline,
        },
    )?;
    let artifacts = if args.no_write_reports {
        None
    } else {
        Some(write_eval_reports(&report, &args.reports_dir)?)
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Evaluation {}", report.run_id);
        println!(
            "- suite: {} v{} ({} cases)",
            report.suite_id, report.suite_version, report.summary.case_count
        );
        println!(
            "- executions: {} completed, {} skipped, {} failed",
            report.summary.completed, report.summary.skipped, report.summary.failed
        );
        for summary in &report.summary.strategies {
            println!(
                "- {}: Hit@5 {}, Recall@k {}, MRR {}, ~{:.0} tokens, p95 {:.2} ms",
                summary.strategy,
                format_optional_rate(summary.hit_at_5),
                format_optional_rate(summary.mean_recall_at_k),
                format_optional_rate(summary.mean_reciprocal_rank),
                summary.mean_estimated_tokens,
                summary.latency_p95_us as f64 / 1_000.0,
            );
        }
        if let Some(baseline) = &report.baseline {
            println!("- baseline regressions: {}", baseline.regressions.len());
        }
        if let Some(artifacts) = &artifacts {
            println!("- JSON: {}", artifacts.json.display());
            println!("- Markdown: {}", artifacts.markdown.display());
        }
    }
    io::stdout().flush()?;
    let has_regressions = report
        .baseline
        .as_ref()
        .is_some_and(|baseline| !baseline.regressions.is_empty());
    if report.summary.failed > 0 || (args.fail_on_regression && has_regressions) {
        std::process::exit(2);
    }
    Ok(())
}

fn parse_external_fixtures(
    values: &[String],
) -> Result<std::collections::BTreeMap<String, PathBuf>> {
    let mut fixtures = std::collections::BTreeMap::new();
    for value in values {
        let (id, path) = value
            .split_once('=')
            .with_context(|| format!("invalid external fixture `{value}`; expected ID=PATH"))?;
        let id = id.trim();
        let path = path.trim();
        if id.is_empty() || path.is_empty() {
            bail!("invalid external fixture `{value}`; ID and PATH must not be empty");
        }
        if fixtures
            .insert(id.to_string(), PathBuf::from(path))
            .is_some()
        {
            bail!("duplicate external fixture id `{id}`");
        }
    }
    Ok(fixtures)
}

fn format_optional_rate(value: Option<f64>) -> String {
    value.map_or_else(|| "n/a".to_string(), |value| format!("{value:.3}"))
}

fn sanitized_eval_path_label(path: &Path) -> String {
    if path.is_relative() {
        return path.to_string_lossy().replace('\\', "/");
    }
    path.file_name().map_or_else(
        || "<absolute>".to_string(),
        |name| format!("<absolute>/{}", name.to_string_lossy()),
    )
}

fn parse_isolated_java_context() -> Option<JavaContextArgs> {
    let mut args = std::env::args_os();
    args.next()?;
    let first = args.next()?;
    let mut parser_args = vec![std::ffi::OsString::from("opticcode java-context")];
    if first == std::ffi::OsStr::new("java-context") {
        parser_args.extend(args);
    } else if first == std::ffi::OsStr::new("help")
        && args.next().as_deref() == Some(std::ffi::OsStr::new("java-context"))
    {
        parser_args.push(std::ffi::OsString::from("--help"));
        parser_args.extend(args);
    } else {
        return None;
    }
    Some(JavaContextArgs::parse_from(parser_args))
}

fn run_java_context(
    JavaContextArgs {
        task,
        path,
        limit,
        max_file_bytes,
        item_limit,
        symbol_limit,
        reference_limit,
        candidate_limit,
        identifier_limit,
        term_limit,
        visited_symbol_limit,
        primary_symbol_limit,
        selection_limit,
        relation_limit,
        relation_depth,
        snippet_limit,
        snippet_bytes,
        context_bytes,
        context_chars,
        context_tokens,
        warning_limit,
        caller_limit,
        related_limit,
        compare_baseline,
        json,
    }: JavaContextArgs,
) -> Result<()> {
    let mut report = build_java_task_context(
        &path,
        &task,
        JavaTaskContextOptions {
            index: JavaIndexOptions {
                syntax: JavaSyntaxOptions {
                    max_files: limit,
                    max_file_bytes,
                    max_items_per_kind: item_limit,
                },
                max_symbols: symbol_limit,
                max_references: reference_limit,
                max_candidates_per_reference: candidate_limit,
            },
            max_query_identifiers: identifier_limit,
            max_query_terms: term_limit,
            max_symbols_visited: visited_symbol_limit,
            max_primary_symbols: primary_symbol_limit,
            max_candidates: selection_limit,
            max_relations: relation_limit,
            max_relation_depth: relation_depth,
            max_snippets: snippet_limit,
            max_snippet_bytes: snippet_bytes,
            max_context_bytes: context_bytes,
            max_context_chars: context_chars,
            max_estimated_tokens: context_tokens,
            max_warnings: warning_limit,
            max_callers_per_symbol: caller_limit,
            max_related_symbols: related_limit,
            compare_baseline,
        },
    )?;
    if json {
        let serialization_started = Instant::now();
        let _ = serde_json::to_vec(&report)?;
        report.timings.serialization_us = Some(
            serialization_started
                .elapsed()
                .as_micros()
                .min(u128::from(u64::MAX)) as u64,
        );
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{}", report.to_display_string());
    }
    Ok(())
}

async fn run_command(command: Command) -> Result<()> {
    match command {
        Command::Inspect { path } => {
            let report = inspect_workspace(&path)?;
            println!("{}", report.to_display_string());
        }
        Command::GitState { path, json } => {
            let snapshot = capture_git_state(&path)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&snapshot)?);
            } else {
                println!("{}", snapshot.to_display_string());
            }
        }
        Command::Search {
            pattern,
            path,
            limit,
        } => {
            let matches = search_workspace(&path, &pattern, limit)?;
            if matches.is_empty() {
                println!("No matches found.");
            } else {
                for hit in matches {
                    println!(
                        "{}:{}: {}",
                        hit.path.display(),
                        hit.line_number,
                        hit.line.trim()
                    );
                }
            }
        }
        Command::Context { path } => {
            let context = build_project_context(&path)?;
            println!("{}", context.to_display_string());
        }
        Command::JavaContext => bail!("java-context must be dispatched by its isolated parser"),
        Command::Eval => bail!("eval must be dispatched by its isolated parser"),
        Command::AnalyzeJava { path } => {
            let analysis = analyze_java_project(&path)?;
            println!("{}", analysis.to_display_string());
        }
        Command::JavaSyntax {
            path,
            limit,
            max_file_bytes,
            item_limit,
            json,
        } => {
            let report = analyze_java_syntax(
                &path,
                JavaSyntaxOptions {
                    max_files: limit,
                    max_file_bytes,
                    max_items_per_kind: item_limit,
                },
            )?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("{}", report.to_display_string());
            }
        }
        Command::JavaIndex {
            path,
            limit,
            max_file_bytes,
            item_limit,
            symbol_limit,
            reference_limit,
            candidate_limit,
            json,
        } => {
            let mut report = analyze_java_index(
                &path,
                JavaIndexOptions {
                    syntax: JavaSyntaxOptions {
                        max_files: limit,
                        max_file_bytes,
                        max_items_per_kind: item_limit,
                    },
                    max_symbols: symbol_limit,
                    max_references: reference_limit,
                    max_candidates_per_reference: candidate_limit,
                },
            )?;
            if json {
                let serialization_started = Instant::now();
                let _ = serde_json::to_vec(&report)?;
                report.timings.serialization_us = Some(
                    serialization_started
                        .elapsed()
                        .as_micros()
                        .min(u128::from(u64::MAX)) as u64,
                );
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("{}", report.to_display_string());
            }
        }
        Command::JavaEdits {
            path,
            limit,
            max_file_bytes,
            item_limit,
            symbol_limit,
            reference_limit,
            candidate_limit,
            proposal_limit,
            json,
        } => {
            let mut report = propose_java_edits(
                &path,
                JavaEditOptions {
                    index: JavaIndexOptions {
                        syntax: JavaSyntaxOptions {
                            max_files: limit,
                            max_file_bytes,
                            max_items_per_kind: item_limit,
                        },
                        max_symbols: symbol_limit,
                        max_references: reference_limit,
                        max_candidates_per_reference: candidate_limit,
                    },
                    max_proposals: proposal_limit,
                },
            )?;
            if json {
                let serialization_started = Instant::now();
                let _ = serde_json::to_vec(&report)?;
                report.timings.serialization_us = Some(
                    serialization_started
                        .elapsed()
                        .as_micros()
                        .min(u128::from(u64::MAX)) as u64,
                );
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("{}", report.to_display_string());
            }
        }
        Command::JavaLegacyRules { json } => {
            let catalog = legacy_rule_catalog();
            if json {
                println!("{}", serde_json::to_string_pretty(&catalog)?);
            } else {
                println!("{}", catalog.to_display_string());
            }
        }
        Command::JavaEditsVerify {
            path,
            limit,
            max_file_bytes,
            item_limit,
            symbol_limit,
            reference_limit,
            candidate_limit,
            proposal_limit,
            timeout_seconds,
            git_timeout_seconds,
            output_limit_bytes,
            json,
        } => {
            let cancellation = CancellationToken::new();
            let signal_cancellation = cancellation.clone();
            let signal_task = tokio::spawn(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    signal_cancellation.cancel();
                }
            });
            tokio::task::yield_now().await;
            let result = verify_java_edits_in_worktree(
                &path,
                JavaEditWorktreeOptions {
                    edits: JavaEditOptions {
                        index: JavaIndexOptions {
                            syntax: JavaSyntaxOptions {
                                max_files: limit,
                                max_file_bytes,
                                max_items_per_kind: item_limit,
                            },
                            max_symbols: symbol_limit,
                            max_references: reference_limit,
                            max_candidates_per_reference: candidate_limit,
                        },
                        max_proposals: proposal_limit,
                    },
                    worktree: WorktreeVerificationOptions {
                        build_timeout: Duration::from_secs(timeout_seconds),
                        git_timeout: Duration::from_secs(git_timeout_seconds),
                        output_limit_bytes,
                    },
                },
                &cancellation,
            );
            signal_task.abort();
            match result {
                Ok(report) => {
                    if json {
                        println!("{}", serde_json::to_string_pretty(&report)?);
                    } else {
                        println!("{}", report.to_display_string());
                    }
                    let exit_code = java_edit_worktree_exit_code(&report);
                    if exit_code != 0 {
                        std::process::exit(exit_code);
                    }
                }
                Err(error) => {
                    print_java_edit_worktree_error(&error, json)?;
                    std::process::exit(java_edit_worktree_error_exit_code(&error));
                }
            }
        }
        Command::Build {
            path,
            fail_on_worktree_change,
            timeout_seconds,
            output_limit_bytes,
            json,
        } => {
            let cancellation = CancellationToken::new();
            let signal_cancellation = cancellation.clone();
            let signal_task = tokio::spawn(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    signal_cancellation.cancel();
                }
            });
            tokio::task::yield_now().await;
            let result = build_java_project_with_cancellation(
                &path,
                BuildOptions {
                    fail_on_worktree_change,
                    timeout: Duration::from_secs(timeout_seconds),
                    output_limit_bytes,
                },
                &cancellation,
            );
            signal_task.abort();
            let result = result?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&BuildJson::from(&result))?
                );
            } else {
                println!("{}", result.to_display_string());
            }
            if !result.command_succeeded() {
                std::process::exit(1);
            }
        }
        Command::Profile { path, profile } => {
            match load_profile_for_workspace(&path, Some(&profile))? {
                Some(profile) => println!("{}", profile.to_display_string()),
                None => println!("Profile disabled."),
            }
        }
        Command::Memory { path, profile } => {
            let memory = load_memory_for_workspace(&path, Some(&profile))?;
            println!("{}", memory.to_display_string());
        }
        Command::PackScan { path, limit } => {
            let report = inspect_resource_pack(&path, limit)?;
            println!("{}", report.to_display_string());
        }
        Command::RagScan { paths, limit, json } => {
            let reports = paths
                .iter()
                .map(|path| inspect_rag_source(path, limit))
                .collect::<Result<Vec<_>>>()?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&RagScanJson {
                        schema_version: 1,
                        sources: &reports,
                    })?
                );
            } else {
                for (index, report) in reports.iter().enumerate() {
                    if index > 0 {
                        println!("\n---\n");
                    }
                    println!("{}", report.to_display_string());
                }
            }
        }
        Command::RagIndex {
            paths,
            output,
            chunk_chars,
            json,
        } => {
            let report = build_rag_index(&paths, &output, chunk_chars)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("{}", report.to_display_string());
            }
        }
        Command::RagSearch {
            query,
            index,
            limit,
            json,
        } => {
            let report = search_rag_index_report(&index, &query, limit)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else if report.hits.is_empty() {
                println!("No matches found.");
            } else {
                for hit in report.hits {
                    println!("{}\n", hit.to_display_string());
                }
            }
        }
        Command::RagDebug {
            query,
            index,
            limit,
            json,
        } => {
            let rag = load_rag_context(&index, &query, limit)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&RagDebugJson {
                        schema_version: 1,
                        context: &rag,
                    })?
                );
            } else {
                println!("{}", rag.to_display_string());
            }
        }
        Command::Patch { path, check } => {
            let proposal = propose_java_legacy_patch(&path)?;
            println!("{}", proposal.to_display_string());
            if check {
                match check_patch_with_git(&proposal)? {
                    Some(result) => {
                        println!("{}", result.to_display_string());
                        if !result.success {
                            std::process::exit(1);
                        }
                    }
                    None => println!("Patch check: skipped, no changes."),
                }
            }
        }
        Command::Apply {
            path,
            dry_run,
            copy_to,
            undo,
            allow_external,
            allow_dirty,
            yes,
            json,
        } => {
            match execute_apply_command(
                path,
                dry_run,
                copy_to,
                undo,
                allow_external,
                allow_dirty,
                yes,
            ) {
                Ok(output) => {
                    print_apply_command_output(&output, json)?;
                    let exit_code = output.exit_code();
                    if exit_code != 0 {
                        std::process::exit(exit_code);
                    }
                }
                Err(error) => {
                    print_apply_error("apply", &error, json)?;
                    std::process::exit(apply_error_exit_code(&error));
                }
            }
        }
        Command::Transactions {
            path,
            inspect,
            recover,
            allow_external,
            yes,
            json,
        } => match execute_transactions_command(path, inspect, recover, allow_external, yes) {
            Ok(output) => {
                print_transactions_command_output(&output, json)?;
                let exit_code = output.exit_code();
                if exit_code != 0 {
                    std::process::exit(exit_code);
                }
            }
            Err(error) => {
                print_apply_error("transactions", &error, json)?;
                std::process::exit(apply_error_exit_code(&error));
            }
        },
        Command::WorktreeVerify {
            path,
            timeout_seconds,
            git_timeout_seconds,
            output_limit_bytes,
            json,
        } => {
            let cancellation = CancellationToken::new();
            let signal_cancellation = cancellation.clone();
            let signal_task = tokio::spawn(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    signal_cancellation.cancel();
                }
            });
            tokio::task::yield_now().await;
            let result = verify_java_legacy_patch_in_worktree(
                &path,
                WorktreeVerificationOptions {
                    build_timeout: Duration::from_secs(timeout_seconds),
                    git_timeout: Duration::from_secs(git_timeout_seconds),
                    output_limit_bytes,
                },
                &cancellation,
            );
            signal_task.abort();
            match result {
                Ok(report) => {
                    if json {
                        println!("{}", serde_json::to_string_pretty(&report)?);
                    } else {
                        println!("{}", report.to_display_string());
                    }
                    let exit_code = worktree_verification_exit_code(&report);
                    if exit_code != 0 {
                        std::process::exit(exit_code);
                    }
                }
                Err(error) => {
                    print_worktree_error("worktree_verify", &error, json)?;
                    std::process::exit(worktree_error_exit_code(&error));
                }
            }
        }
        Command::Worktrees { cleanup, yes, json } => {
            if let Some(run_id) = cleanup {
                if !yes {
                    let error = anyhow::anyhow!("worktrees --cleanup requires --yes");
                    print_worktree_error("worktree_cleanup", &error, json)?;
                    std::process::exit(WORKTREE_EXIT_PRECONDITION);
                }
                match cleanup_disposable_worktree(&run_id) {
                    Ok(report) => {
                        print_worktree_cleanup(&report, json)?;
                        if !report.success {
                            std::process::exit(WORKTREE_EXIT_CLEANUP_FAILED);
                        }
                    }
                    Err(error) => {
                        print_worktree_error("worktree_cleanup", &error, json)?;
                        std::process::exit(worktree_error_exit_code(&error));
                    }
                }
            } else {
                let leases = list_disposable_worktrees()?;
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&WorktreeListJson {
                            schema_version: 1,
                            leases: &leases,
                        })?
                    );
                } else {
                    println!("Disposable worktrees: {}", leases.len());
                    for lease in &leases {
                        println!("- {}", lease.to_display_string());
                    }
                }
            }
        }
        Command::Ask {
            prompt,
            path,
            model,
            ollama_url,
            keep_alive,
            profile,
            no_memory,
            no_rag,
            rag_index,
            rag_limit,
            rag_debug,
            brief,
            max_tokens,
            metrics,
            metrics_json,
        } => {
            let app =
                OpticCode::new(ollama_url, model).with_keep_alive(parse_keep_alive(&keep_alive));
            if rag_debug && !no_rag {
                print_rag_debug(&rag_index, &prompt, rag_limit)?;
            }
            let output = app
                .ask_with_metrics(AskOptions {
                    workspace: path,
                    prompt,
                    profile: Some(profile),
                    include_memory: !no_memory,
                    include_rag: !no_rag,
                    rag_index,
                    rag_limit,
                    brief,
                    max_tokens,
                })
                .await?;
            println!("{}", output.text);
            io::stdout().flush()?;
            if metrics {
                print_metrics(&output.metrics);
            }
            if metrics_json {
                print_metrics_json("ask", &output.metrics)?;
            }
        }
        Command::Plan {
            goal,
            path,
            model,
            ollama_url,
            keep_alive,
            profile,
            no_memory,
            no_rag,
            rag_index,
            rag_limit,
            rag_debug,
            brief,
            max_tokens,
            metrics,
            metrics_json,
        } => {
            let app =
                OpticCode::new(ollama_url, model).with_keep_alive(parse_keep_alive(&keep_alive));
            if rag_debug && !no_rag {
                print_rag_debug(&rag_index, &goal, rag_limit)?;
            }
            let output = app
                .plan_with_metrics(PlanOptions {
                    workspace: path,
                    goal,
                    profile: Some(profile),
                    include_memory: !no_memory,
                    include_rag: !no_rag,
                    rag_index,
                    rag_limit,
                    brief,
                    max_tokens,
                })
                .await?;
            println!("{}", output.text);
            io::stdout().flush()?;
            if metrics {
                print_metrics(&output.metrics);
            }
            if metrics_json {
                print_metrics_json("plan", &output.metrics)?;
            }
        }
    }

    Ok(())
}

const APPLY_EXIT_ROLLED_BACK: i32 = 2;
const APPLY_EXIT_ROLLBACK_FAILED: i32 = 3;
const APPLY_EXIT_PRECONDITION: i32 = 4;
const APPLY_EXIT_INVALID_TRANSACTION: i32 = 5;
const WORKTREE_EXIT_VERIFICATION_FAILED: i32 = 6;
const WORKTREE_EXIT_CLEANUP_FAILED: i32 = 7;
const WORKTREE_EXIT_PRECONDITION: i32 = 8;
const WORKTREE_EXIT_INVALID_RUN_ID: i32 = 9;

fn worktree_verification_exit_code(report: &WorktreeVerificationReport) -> i32 {
    if !report.cleanup_success {
        WORKTREE_EXIT_CLEANUP_FAILED
    } else if report.status == WorktreeVerificationStatus::Passed && report.success() {
        0
    } else {
        WORKTREE_EXIT_VERIFICATION_FAILED
    }
}

fn java_edit_worktree_exit_code(report: &JavaEditWorktreeReport) -> i32 {
    if !report.cleanup_success {
        WORKTREE_EXIT_CLEANUP_FAILED
    } else if report.success() {
        0
    } else {
        WORKTREE_EXIT_VERIFICATION_FAILED
    }
}

fn java_edit_worktree_error_exit_code(error: &anyhow::Error) -> i32 {
    match java_edit_worktree_error_kind(error) {
        Some(JavaEditWorktreeErrorKind::Precondition) => WORKTREE_EXIT_PRECONDITION,
        Some(
            JavaEditWorktreeErrorKind::Verification
            | JavaEditWorktreeErrorKind::Git
            | JavaEditWorktreeErrorKind::Storage,
        )
        | None => WORKTREE_EXIT_VERIFICATION_FAILED,
    }
}

fn worktree_error_exit_code(error: &anyhow::Error) -> i32 {
    match worktree_operation_error_kind(error) {
        Some(WorktreeOperationErrorKind::InvalidRunId) => WORKTREE_EXIT_INVALID_RUN_ID,
        Some(WorktreeOperationErrorKind::Precondition) => WORKTREE_EXIT_PRECONDITION,
        Some(WorktreeOperationErrorKind::Git | WorktreeOperationErrorKind::Storage) | None => {
            WORKTREE_EXIT_VERIFICATION_FAILED
        }
    }
}

fn print_worktree_error(operation: &str, error: &anyhow::Error, json: bool) -> Result<()> {
    let kind =
        worktree_operation_error_kind(error).unwrap_or(WorktreeOperationErrorKind::Precondition);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&WorktreeErrorJson {
                schema_version: 1,
                operation,
                operation_success: false,
                error_kind: kind,
                error: format!("{error:#}"),
            })?
        );
    } else {
        eprintln!("Error: {error:#}");
    }
    Ok(())
}

fn print_java_edit_worktree_error(error: &anyhow::Error, json: bool) -> Result<()> {
    let kind =
        java_edit_worktree_error_kind(error).unwrap_or(JavaEditWorktreeErrorKind::Verification);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&JavaEditWorktreeErrorJson {
                schema_version: 1,
                operation: "java_edits_verify",
                operation_success: false,
                error_kind: kind,
                error: format!("{error:#}"),
            })?
        );
    } else {
        eprintln!("Error: {error:#}");
    }
    Ok(())
}

fn print_worktree_cleanup(report: &WorktreeCleanupReport, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
    } else {
        println!("Worktree cleanup: {}", report.run_id);
        println!("- path: {}", report.worktree.display());
        println!("- success: {}", report.success);
        println!("- descriptor removed: {}", report.descriptor_removed);
        for error in &report.errors {
            println!("Error: {error}");
        }
    }
    Ok(())
}

enum ApplyCommandOutput {
    Plan(Box<ApplyPlan>),
    Undo(Box<ApplyUndoResult>),
}

impl ApplyCommandOutput {
    fn exit_code(&self) -> i32 {
        match self {
            Self::Plan(plan) if plan.success() => 0,
            Self::Plan(plan) => apply_failure_exit_code(plan.transaction.as_ref()),
            Self::Undo(result) if result.success() => 0,
            Self::Undo(result)
                if result
                    .transaction
                    .as_ref()
                    .is_some_and(ApplyTransactionResult::rollback_failed) =>
            {
                APPLY_EXIT_ROLLBACK_FAILED
            }
            Self::Undo(_) => APPLY_EXIT_ROLLED_BACK,
        }
    }
}

fn apply_failure_exit_code(transaction: Option<&ApplyTransactionResult>) -> i32 {
    match transaction {
        Some(result) if result.rollback_failed() => APPLY_EXIT_ROLLBACK_FAILED,
        Some(result) if result.rolled_back() => APPLY_EXIT_ROLLED_BACK,
        _ => APPLY_EXIT_PRECONDITION,
    }
}

enum TransactionsCommandOutput {
    List(Vec<ApplyTransactionInspection>),
    Inspect(ApplyTransactionInspection),
    Recover(ApplyTransactionResult),
}

impl TransactionsCommandOutput {
    fn exit_code(&self) -> i32 {
        match self {
            Self::List(_) => 0,
            Self::Inspect(inspection) if inspection.valid => 0,
            Self::Inspect(_) => APPLY_EXIT_INVALID_TRANSACTION,
            Self::Recover(result) if result.rolled_back() => 0,
            Self::Recover(result) if result.rollback_failed() => APPLY_EXIT_ROLLBACK_FAILED,
            Self::Recover(_) => APPLY_EXIT_ROLLED_BACK,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_apply_command(
    path: PathBuf,
    dry_run: bool,
    copy_to: Option<PathBuf>,
    undo: Option<String>,
    allow_external: bool,
    allow_dirty: bool,
    yes: bool,
) -> Result<ApplyCommandOutput> {
    if let Some(run_id) = undo {
        if dry_run || copy_to.is_some() || allow_dirty {
            bail!("apply --undo cannot be combined with --dry-run, --copy-to, or --allow-dirty");
        }
        if !yes {
            bail!("apply --undo requires --yes");
        }
        ensure_apply_path_allowed(&path, allow_external)?;
        return undo_apply_run(&path, &run_id)
            .map(Box::new)
            .map(ApplyCommandOutput::Undo);
    }

    if dry_run && copy_to.is_some() {
        bail!("apply --dry-run cannot be combined with --copy-to");
    }
    if dry_run {
        if allow_dirty {
            bail!("apply --allow-dirty is only valid for a real in-place apply");
        }
        return prepare_java_legacy_apply_plan(&path, true)
            .map(Box::new)
            .map(ApplyCommandOutput::Plan);
    }
    if let Some(copy_to) = copy_to {
        if allow_dirty {
            bail!("apply --allow-dirty is not needed with --copy-to");
        }
        if !yes {
            bail!("apply with --copy-to requires --yes");
        }
        return apply_java_legacy_patch_to_copy(&path, &copy_to)
            .map(Box::new)
            .map(ApplyCommandOutput::Plan);
    }
    if !yes {
        bail!("real apply requires --yes; use --dry-run or --copy-to <path> --yes");
    }

    ensure_apply_path_allowed(&path, allow_external)?;
    apply_java_legacy_patch_in_place_with_options(&path, allow_dirty)
        .map(Box::new)
        .map(ApplyCommandOutput::Plan)
}

fn execute_transactions_command(
    path: PathBuf,
    inspect: Option<String>,
    recover: Option<String>,
    allow_external: bool,
    yes: bool,
) -> Result<TransactionsCommandOutput> {
    if inspect.is_some() && recover.is_some() {
        bail!("transactions --inspect cannot be combined with --recover");
    }
    if let Some(transaction_id) = recover {
        if !yes {
            bail!("transactions --recover requires --yes");
        }
        ensure_apply_path_allowed(&path, allow_external)?;
        return recover_apply_transaction(&path, &transaction_id)
            .map(TransactionsCommandOutput::Recover);
    }
    if let Some(transaction_id) = inspect {
        return inspect_apply_transaction(&path, &transaction_id)
            .map(TransactionsCommandOutput::Inspect);
    }
    list_apply_transactions(&path).map(TransactionsCommandOutput::List)
}

fn print_apply_command_output(output: &ApplyCommandOutput, json: bool) -> Result<()> {
    match output {
        ApplyCommandOutput::Plan(plan) if json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&ApplyPlanJson::from(plan.as_ref()))?
            );
        }
        ApplyCommandOutput::Plan(plan) => println!("{}", plan.to_display_string()),
        ApplyCommandOutput::Undo(result) if json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&ApplyUndoJson::from(result.as_ref()))?
            );
        }
        ApplyCommandOutput::Undo(result) => println!("{}", result.to_display_string()),
    }
    Ok(())
}

fn print_transactions_command_output(output: &TransactionsCommandOutput, json: bool) -> Result<()> {
    if json {
        match output {
            TransactionsCommandOutput::List(transactions) => println!(
                "{}",
                serde_json::to_string_pretty(&TransactionListJson {
                    schema_version: 1,
                    transactions,
                })?
            ),
            TransactionsCommandOutput::Inspect(inspection) => {
                println!("{}", serde_json::to_string_pretty(inspection)?)
            }
            TransactionsCommandOutput::Recover(result) => {
                println!("{}", serde_json::to_string_pretty(result)?)
            }
        }
        return Ok(());
    }

    match output {
        TransactionsCommandOutput::List(transactions) => {
            println!("Transactions: {}", transactions.len());
            for transaction in transactions {
                println!(
                    "- {} state={} valid={} recoverable={}{}",
                    transaction.transaction_id,
                    transaction
                        .final_state
                        .map_or("unknown", |state| state.as_str()),
                    transaction.valid,
                    transaction.recoverable,
                    if transaction.legacy { " legacy" } else { "" }
                );
            }
        }
        TransactionsCommandOutput::Inspect(inspection) => {
            println!("Transaction: {}", inspection.transaction_id);
            println!("Valid: {}", inspection.valid);
            println!("Legacy: {}", inspection.legacy);
            println!(
                "Final state: {}",
                inspection
                    .final_state
                    .map_or("unknown", |state| state.as_str())
            );
            println!("Recoverable: {}", inspection.recoverable);
            for error in &inspection.errors {
                println!("Error: {error}");
            }
        }
        TransactionsCommandOutput::Recover(result) => {
            println!("Transaction: {}", result.transaction_id);
            println!("Final state: {}", result.final_state.as_str());
            println!("Rollback success: {:?}", result.rollback_success);
            println!("Restored files: {}", result.restored_files.len());
        }
    }
    Ok(())
}

fn print_apply_error(operation: &str, error: &anyhow::Error, json: bool) -> Result<()> {
    let kind =
        apply_transaction_error_kind(error).unwrap_or(ApplyTransactionErrorKind::Precondition);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&ApplyErrorJson {
                schema_version: 1,
                operation,
                operation_success: false,
                error_kind: kind,
                error: format!("{error:#}"),
            })?
        );
    } else {
        eprintln!("Error: {error:#}");
    }
    Ok(())
}

fn apply_error_exit_code(error: &anyhow::Error) -> i32 {
    match apply_transaction_error_kind(error) {
        Some(ApplyTransactionErrorKind::Precondition) | None => APPLY_EXIT_PRECONDITION,
        Some(
            ApplyTransactionErrorKind::Collision
            | ApplyTransactionErrorKind::InvalidTransaction
            | ApplyTransactionErrorKind::Io,
        ) => APPLY_EXIT_INVALID_TRANSACTION,
    }
}

fn ensure_apply_path_allowed(path: &Path, allow_external: bool) -> Result<()> {
    let workspace = fs::canonicalize(std::env::current_dir()?)?;
    let target = fs::canonicalize(path)?;

    if target.starts_with(&workspace) {
        return Ok(());
    }

    if !allow_external {
        bail!(
            "real apply is currently limited to the current workspace: {} is outside {}; add --allow-external only for an explicit external Git project",
            target.display(),
            workspace.display()
        );
    }

    ensure_external_git_project(&target)?;

    Ok(())
}

fn ensure_external_git_project(target: &Path) -> Result<()> {
    let output = ProcessCommand::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(target)
        .output()
        .with_context(|| format!("failed to check Git project: {}", target.display()))?;

    if !output.status.success() || String::from_utf8_lossy(&output.stdout).trim() != "true" {
        bail!(
            "external apply requires a Git worktree: {}",
            target.display()
        );
    }

    Ok(())
}

#[derive(Serialize)]
struct ApplyPlanJson<'a> {
    schema_version: u32,
    operation: &'static str,
    operation_success: bool,
    mode: &'static str,
    project: &'a Path,
    copied_from: Option<&'a Path>,
    dry_run: bool,
    change_count: usize,
    files: Vec<String>,
    check: Option<&'a PatchCheckResult>,
    apply: Option<&'a PatchCheckResult>,
    transaction: Option<&'a ApplyTransactionResult>,
    log: Option<&'a ApplyLogEntry>,
}

impl<'a> From<&'a ApplyPlan> for ApplyPlanJson<'a> {
    fn from(plan: &'a ApplyPlan) -> Self {
        let mode = if plan.dry_run {
            "dry_run"
        } else if plan.copied_from.is_some() {
            "copy"
        } else {
            "in_place"
        };

        Self {
            schema_version: 1,
            operation: "apply",
            operation_success: plan.success(),
            mode,
            project: &plan.proposal.root,
            copied_from: plan.copied_from.as_deref(),
            dry_run: plan.dry_run,
            change_count: plan.proposal.changes.len(),
            files: plan
                .proposal
                .changes
                .iter()
                .map(|change| change.path.to_string_lossy().replace('\\', "/"))
                .collect(),
            check: plan.check.as_ref(),
            apply: plan.apply.as_ref(),
            transaction: plan.transaction.as_ref(),
            log: plan.log.as_ref(),
        }
    }
}

#[derive(Serialize)]
struct ApplyUndoJson<'a> {
    schema_version: u32,
    operation: &'static str,
    operation_success: bool,
    project: &'a Path,
    transaction_id: &'a str,
    patch: &'a Path,
    check: &'a PatchCheckResult,
    undo: Option<&'a PatchCheckResult>,
    transaction: Option<&'a ApplyTransactionResult>,
}

impl<'a> From<&'a ApplyUndoResult> for ApplyUndoJson<'a> {
    fn from(result: &'a ApplyUndoResult) -> Self {
        Self {
            schema_version: 1,
            operation: "undo",
            operation_success: result.success(),
            project: &result.root,
            transaction_id: &result.run_id,
            patch: &result.patch_path,
            check: &result.check,
            undo: result.undo.as_ref(),
            transaction: result.transaction.as_ref(),
        }
    }
}

#[derive(Serialize)]
struct TransactionListJson<'a> {
    schema_version: u32,
    transactions: &'a [ApplyTransactionInspection],
}

#[derive(Serialize)]
struct ApplyErrorJson<'a> {
    schema_version: u32,
    operation: &'a str,
    operation_success: bool,
    error_kind: ApplyTransactionErrorKind,
    error: String,
}

#[derive(Serialize)]
struct WorktreeListJson<'a> {
    schema_version: u32,
    leases: &'a [WorktreeLeaseInspection],
}

#[derive(Serialize)]
struct WorktreeErrorJson<'a> {
    schema_version: u32,
    operation: &'a str,
    operation_success: bool,
    error_kind: WorktreeOperationErrorKind,
    error: String,
}

#[derive(Serialize)]
struct JavaEditWorktreeErrorJson {
    schema_version: u32,
    operation: &'static str,
    operation_success: bool,
    error_kind: JavaEditWorktreeErrorKind,
    error: String,
}

#[derive(Serialize)]
struct BuildJson<'a> {
    schema_version: u32,
    project: &'a Path,
    command: &'a str,
    build_success: bool,
    overall_success: bool,
    exit_code: Option<i32>,
    duration_ms: u64,
    summary: &'a [String],
    stdout_tail: &'a str,
    stderr_tail: &'a str,
    process: BuildProcessJson<'a>,
    git_guard: &'a opticcode_tools::git_state::BuildGitReport,
}

#[derive(Serialize)]
struct BuildProcessJson<'a> {
    process_id: Option<u32>,
    status: ProcessStatus,
    timed_out: bool,
    cancelled: bool,
    timeout_ms: u64,
    output: &'a ProcessOutputStats,
    termination: &'a ProcessTermination,
}

impl<'a> From<&'a BuildResult> for BuildJson<'a> {
    fn from(result: &'a BuildResult) -> Self {
        Self {
            schema_version: 1,
            project: &result.root,
            command: &result.command,
            build_success: result.success,
            overall_success: result.command_succeeded(),
            exit_code: result.exit_code,
            duration_ms: result.duration.as_millis().min(u128::from(u64::MAX)) as u64,
            summary: &result.summary,
            stdout_tail: &result.stdout_tail,
            stderr_tail: &result.stderr_tail,
            process: BuildProcessJson {
                process_id: result.process_id,
                status: result.process_status,
                timed_out: result.timed_out,
                cancelled: result.cancelled,
                timeout_ms: result.timeout.as_millis().min(u128::from(u64::MAX)) as u64,
                output: &result.output,
                termination: &result.termination,
            },
            git_guard: &result.git_report,
        }
    }
}

#[derive(Serialize)]
struct RagScanJson<'a> {
    schema_version: u32,
    sources: &'a [RagSourceReport],
}

#[derive(Serialize)]
struct RagDebugJson<'a> {
    schema_version: u32,
    context: &'a RagContext,
}

#[derive(Serialize)]
struct MetricsJson<'a> {
    command: &'a str,
    client_seconds: f64,
    prompt_chars: usize,
    ollama_total_seconds: Option<f64>,
    ollama_load_seconds: Option<f64>,
    keep_alive: Option<String>,
    prompt_eval_count: Option<u64>,
    prompt_eval_seconds: Option<f64>,
    eval_count: Option<u64>,
    eval_seconds: Option<f64>,
    eval_tokens_per_second: Option<f64>,
}

fn print_metrics_json(command: &str, metrics: &GenerateMetrics) -> Result<()> {
    let eval_tokens_per_second = match (
        metrics.eval_count,
        metrics.eval_duration.map(|value| value.as_secs_f64()),
    ) {
        (Some(count), Some(seconds)) if seconds > 0.0 => Some(count as f64 / seconds),
        _ => None,
    };

    let payload = MetricsJson {
        command,
        client_seconds: metrics.client_duration.as_secs_f64(),
        prompt_chars: metrics.prompt_chars,
        ollama_total_seconds: metrics
            .ollama_total_duration
            .map(|value| value.as_secs_f64()),
        ollama_load_seconds: metrics
            .ollama_load_duration
            .map(|value| value.as_secs_f64()),
        keep_alive: metrics.keep_alive.clone(),
        prompt_eval_count: metrics.prompt_eval_count,
        prompt_eval_seconds: metrics
            .prompt_eval_duration
            .map(|value| value.as_secs_f64()),
        eval_count: metrics.eval_count,
        eval_seconds: metrics.eval_duration.map(|value| value.as_secs_f64()),
        eval_tokens_per_second,
    };

    eprintln!();
    eprintln!("=== metrics_json ===");
    eprintln!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}

fn print_rag_debug(index: &Path, query: &str, limit: usize) -> Result<()> {
    let rag = load_rag_context(index, query, limit)?;
    eprintln!();
    eprintln!("=== rag_debug ===");
    eprintln!("{}", rag.to_display_string());
    Ok(())
}

fn print_metrics(metrics: &GenerateMetrics) {
    eprintln!();
    eprintln!("=== metrics ===");
    eprintln!(
        "client_seconds={:.2}",
        metrics.client_duration.as_secs_f64()
    );
    eprintln!("prompt_chars={}", metrics.prompt_chars);
    if let Some(duration) = metrics.ollama_total_duration {
        eprintln!("ollama_total_seconds={:.2}", duration.as_secs_f64());
    }
    if let Some(duration) = metrics.ollama_load_duration {
        eprintln!("ollama_load_seconds={:.2}", duration.as_secs_f64());
    }
    if let Some(keep_alive) = &metrics.keep_alive {
        eprintln!("keep_alive={keep_alive}");
    }
    if let Some(count) = metrics.prompt_eval_count {
        eprintln!("prompt_eval_count={}", count);
    }
    if let Some(duration) = metrics.prompt_eval_duration {
        eprintln!("prompt_eval_seconds={:.2}", duration.as_secs_f64());
    }
    if let Some(count) = metrics.eval_count {
        eprintln!("eval_count={}", count);
    }
    if let Some(duration) = metrics.eval_duration {
        eprintln!("eval_seconds={:.2}", duration.as_secs_f64());
        if let Some(count) = metrics.eval_count {
            if duration.as_secs_f64() > 0.0 {
                eprintln!(
                    "eval_tokens_per_second={:.2}",
                    count as f64 / duration.as_secs_f64()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_failure_exit_code, APPLY_EXIT_PRECONDITION, APPLY_EXIT_ROLLBACK_FAILED,
        APPLY_EXIT_ROLLED_BACK,
    };
    use opticcode_tools::apply_transaction::{ApplyTransactionResult, ApplyTransactionState};

    #[test]
    fn apply_failure_exit_codes_distinguish_rollback_outcomes() {
        let mut result = transaction_result(ApplyTransactionState::RolledBack, Some(true));
        assert_eq!(
            apply_failure_exit_code(Some(&result)),
            APPLY_EXIT_ROLLED_BACK
        );

        result.final_state = ApplyTransactionState::RollbackFailed;
        result.rollback_success = Some(false);
        assert_eq!(
            apply_failure_exit_code(Some(&result)),
            APPLY_EXIT_ROLLBACK_FAILED
        );
        assert_eq!(apply_failure_exit_code(None), APPLY_EXIT_PRECONDITION);
    }

    fn transaction_result(
        final_state: ApplyTransactionState,
        rollback_success: Option<bool>,
    ) -> ApplyTransactionResult {
        ApplyTransactionResult {
            schema_version: 1,
            transaction_id: "apply-test".to_string(),
            workspace: "workspace".to_string(),
            operation_success: false,
            final_state,
            rollback_attempted: true,
            rollback_success,
            planned_files: Vec::new(),
            modified_files: Vec::new(),
            restored_files: Vec::new(),
            errors: Vec::new(),
            warnings: Vec::new(),
            duration_ms: 0,
            git_restored: Some(true),
            transaction_dir: ".opticcode/runs/apply-test".to_string(),
        }
    }
}
