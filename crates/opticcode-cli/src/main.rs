use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use opticcode_core::{
    load_memory_for_workspace, load_profile_for_workspace, load_rag_context, parse_keep_alive,
    AskOptions, GenerateMetrics, OpticCode, PlanOptions, DEFAULT_PROFILE,
};
use opticcode_tools::{
    analyze_java_project, apply_java_legacy_patch_in_place, apply_java_legacy_patch_to_copy,
    build_java_project, build_project_context, build_rag_index, check_patch_with_git,
    inspect_rag_source, inspect_resource_pack, inspect_workspace, prepare_java_legacy_apply_plan,
    propose_java_legacy_patch, search_rag_index, search_workspace, undo_apply_run,
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

#[derive(Debug, Subcommand)]
enum Command {
    Inspect {
        #[arg(long, default_value = ".")]
        path: PathBuf,
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
    AnalyzeJava {
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
    Build {
        #[arg(long, default_value = ".")]
        path: PathBuf,
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
    },
    RagIndex {
        #[arg(long = "path", required = true)]
        paths: Vec<PathBuf>,
        #[arg(long, default_value = "data/index")]
        output: PathBuf,
        #[arg(long, default_value_t = 4000)]
        chunk_chars: usize,
    },
    RagSearch {
        query: String,
        #[arg(long, default_value = "data/index")]
        index: PathBuf,
        #[arg(long, default_value_t = 8)]
        limit: usize,
    },
    RagDebug {
        query: String,
        #[arg(long, default_value = "data/index")]
        index: PathBuf,
        #[arg(long, default_value_t = 4)]
        limit: usize,
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
        yes: bool,
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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Inspect { path } => {
            let report = inspect_workspace(&path)?;
            println!("{}", report.to_display_string());
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
        Command::AnalyzeJava { path } => {
            let analysis = analyze_java_project(&path)?;
            println!("{}", analysis.to_display_string());
        }
        Command::Build { path } => {
            let result = build_java_project(&path)?;
            println!("{}", result.to_display_string());
            if !result.success {
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
        Command::RagScan { paths, limit } => {
            for (index, path) in paths.iter().enumerate() {
                if index > 0 {
                    println!("\n---\n");
                }
                let report = inspect_rag_source(path, limit)?;
                println!("{}", report.to_display_string());
            }
        }
        Command::RagIndex {
            paths,
            output,
            chunk_chars,
        } => {
            let report = build_rag_index(&paths, &output, chunk_chars)?;
            println!("{}", report.to_display_string());
        }
        Command::RagSearch {
            query,
            index,
            limit,
        } => {
            let hits = search_rag_index(&index, &query, limit)?;
            if hits.is_empty() {
                println!("No matches found.");
            } else {
                for hit in hits {
                    println!("{}\n", hit.to_display_string());
                }
            }
        }
        Command::RagDebug {
            query,
            index,
            limit,
        } => {
            let rag = load_rag_context(&index, &query, limit)?;
            println!("{}", rag.to_display_string());
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
            yes,
        } => {
            if let Some(run_id) = undo {
                if dry_run || copy_to.is_some() {
                    bail!("apply --undo cannot be combined with --dry-run or --copy-to");
                }
                if !yes {
                    bail!("apply --undo requires --yes");
                }
                ensure_apply_path_is_inside_current_workspace(&path)?;
                let result = undo_apply_run(&path, &run_id)?;
                println!("{}", result.to_display_string());
                if !result.success() {
                    std::process::exit(1);
                }
                return Ok(());
            }

            let plan = if dry_run {
                prepare_java_legacy_apply_plan(&path, true)?
            } else if let Some(copy_to) = copy_to {
                if !yes {
                    bail!("apply with --copy-to requires --yes");
                }
                apply_java_legacy_patch_to_copy(&path, &copy_to)?
            } else if yes {
                ensure_apply_path_is_inside_current_workspace(&path)?;
                apply_java_legacy_patch_in_place(&path)?
            } else {
                bail!("real apply requires --yes; use --dry-run or --copy-to <path> --yes");
            };
            println!("{}", plan.to_display_string());
            if !plan.success() {
                std::process::exit(1);
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

fn ensure_apply_path_is_inside_current_workspace(path: &Path) -> Result<()> {
    let workspace = fs::canonicalize(std::env::current_dir()?)?;
    let target = fs::canonicalize(path)?;

    if !target.starts_with(&workspace) {
        bail!(
            "real apply is currently limited to the current workspace: {} is outside {}",
            target.display(),
            workspace.display()
        );
    }

    Ok(())
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

fn print_rag_debug(index: &PathBuf, query: &str, limit: usize) -> Result<()> {
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
