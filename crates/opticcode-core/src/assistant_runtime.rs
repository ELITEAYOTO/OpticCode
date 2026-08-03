use std::path::Path;
use std::time::Instant;

use anyhow::{bail, Result};
use opticcode_llm::{GenerateMetrics, GenerateOptions, OllamaClient};
use serde::Serialize;

use crate::{
    build_plan_prompt, build_prompt, load_memory_for_workspace, load_profile_for_workspace,
    load_rag_context, prepare_assistant_context, ContextFallbackPolicy, ContextMode,
    ContextPreparation, MemoryContext, ProfileContext, RagContext,
};

pub const ASSISTANT_RUN_SCHEMA_VERSION: u32 = 1;
pub const ASSISTANT_PROMPT_VERSION: &str = "opticcode-assistant-prompt-v2";

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AssistantCommandKind {
    Ask,
    Plan,
}

impl AssistantCommandKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Plan => "plan",
        }
    }

    fn brief_default_tokens(self) -> u32 {
        match self {
            Self::Ask => 240,
            Self::Plan => 320,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantGenerationConfiguration {
    pub provider: &'static str,
    pub endpoint: String,
    pub model: String,
    pub temperature: Option<f32>,
    pub seed: Option<i64>,
    pub max_generated_tokens: Option<u32>,
    pub http_timeout_ms: u64,
    pub absolute_determinism_guaranteed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantPromptReport {
    pub version: &'static str,
    pub bytes: usize,
    pub chars: usize,
    pub estimated_tokens: usize,
    pub token_estimator: &'static str,
    pub content_hash: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantGenerationMetrics {
    pub client_ms: u64,
    pub ollama_total_ms: Option<u64>,
    pub ollama_load_ms: Option<u64>,
    pub prompt_eval_count: Option<u64>,
    pub prompt_eval_ms: Option<u64>,
    pub generated_tokens: Option<u64>,
    pub generation_ms: Option<u64>,
    pub generated_tokens_per_second: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantRagHitReport {
    pub source: String,
    pub chunk_id: String,
    pub score: usize,
    pub weighted_score: usize,
    pub matched_queries: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantRagReport {
    pub enabled: bool,
    pub validated_active_v2: bool,
    pub queries: Vec<String>,
    pub hits: Vec<AssistantRagHitReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantStructuredError {
    pub code: String,
    pub stage: String,
    pub message: String,
    pub context_mode: Option<ContextMode>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantRunReport {
    pub context_mode: ContextMode,
    pub generated: bool,
    pub skipped_reason: Option<String>,
    pub prompt: AssistantPromptReport,
    pub metrics: Option<AssistantGenerationMetrics>,
    pub response: Option<String>,
    pub error: Option<AssistantStructuredError>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantCommandReport {
    pub schema_version: u32,
    pub command: AssistantCommandKind,
    pub request: String,
    pub success: bool,
    pub provider: &'static str,
    pub model: String,
    pub requested_context_mode: ContextMode,
    pub used_context_mode: Option<ContextMode>,
    pub analysis_complete: bool,
    pub double_generation_authorized: bool,
    pub preparation_duration_us: u64,
    pub generation: AssistantGenerationConfiguration,
    pub context: ContextPreparation,
    pub rag: AssistantRagReport,
    pub runs: Vec<AssistantRunReport>,
    pub warnings: Vec<String>,
    pub errors: Vec<AssistantStructuredError>,
}

impl AssistantCommandReport {
    pub fn generated_run(&self) -> Option<&AssistantRunReport> {
        self.runs.iter().find(|run| run.generated)
    }
}

// Kept private to prevent callers from treating serialized millisecond metrics as the
// original high-resolution provider metrics.
pub(crate) struct AssistantExecutionOutput {
    pub report: AssistantCommandReport,
    pub raw_metrics: Vec<(ContextMode, GenerateMetrics)>,
}

pub(crate) struct AssistantExecutionOptions<'a> {
    pub command: AssistantCommandKind,
    pub workspace: &'a Path,
    pub request: &'a str,
    pub profile: Option<&'a str>,
    pub include_memory: bool,
    pub include_rag: bool,
    pub rag_index: &'a Path,
    pub rag_limit: usize,
    pub brief: bool,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub seed: Option<i64>,
    pub context_mode: ContextMode,
    pub fallback_policy: ContextFallbackPolicy,
    pub compare_generate: bool,
    pub verify_model: bool,
}

pub(crate) async fn execute_assistant(
    llm: &OllamaClient,
    model: &str,
    options: AssistantExecutionOptions<'_>,
) -> Result<AssistantExecutionOutput> {
    validate_execution_options(model, &options)?;
    let preparation_started = Instant::now();
    let context = prepare_assistant_context(
        options.workspace,
        options.request,
        options.context_mode,
        options.fallback_policy,
    )?;
    let profile = load_profile_for_workspace(options.workspace, options.profile)?;
    let memory = if options.include_memory {
        load_memory_for_workspace(options.workspace, options.profile)?
    } else {
        MemoryContext::default()
    };
    let rag = if options.include_rag {
        load_rag_context(options.rag_index, options.request, options.rag_limit)?
    } else {
        RagContext::default()
    };
    let max_generated_tokens = options.max_tokens.or_else(|| {
        options
            .brief
            .then(|| options.command.brief_default_tokens())
    });
    let mut warnings = context
        .fallback
        .as_ref()
        .map(|fallback| vec![fallback.warning.clone()])
        .unwrap_or_default();
    if options.context_mode == ContextMode::Compare && !options.compare_generate {
        warnings.push(
            "context comparison completed without model generation; use --compare-generate to explicitly authorize two calls"
                .to_string(),
        );
    }
    if options.compare_generate {
        warnings.push(
            "two model generations were explicitly authorized; both use identical generation settings"
                .to_string(),
        );
    }

    let generation = AssistantGenerationConfiguration {
        provider: "ollama",
        endpoint: llm.base_url().to_string(),
        model: model.to_string(),
        temperature: options.temperature,
        seed: options.seed,
        max_generated_tokens,
        http_timeout_ms: duration_ms(llm.timeout()),
        absolute_determinism_guaranteed: false,
    };
    let modes = requested_run_modes(&context, options.context_mode);
    let mut prompts = Vec::new();
    let mut runs = Vec::new();
    for mode in modes {
        let variant = context
            .variant(mode)
            .ok_or_else(|| anyhow::anyhow!("prepared context variant `{mode}` is missing"))?;
        let prompt = compose_prompt(
            options.command,
            options.request,
            &variant.prompt_context,
            profile.as_ref(),
            &memory,
            &rag,
            options.brief,
        );
        let skipped_reason = (!variant.report.usable_for_generation).then(|| {
            format!(
                "context rejected: {}",
                variant
                    .report
                    .rejection_reasons
                    .iter()
                    .map(|reason| reason.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        });
        runs.push(AssistantRunReport {
            context_mode: mode,
            generated: false,
            skipped_reason,
            prompt: prompt_report(&prompt),
            metrics: None,
            response: None,
            error: None,
        });
        prompts.push(prompt);
    }

    let rag_report = rag_report(options.include_rag, &rag);
    let preparation_duration_us = duration_us(preparation_started.elapsed());
    let mut report = AssistantCommandReport {
        schema_version: ASSISTANT_RUN_SCHEMA_VERSION,
        command: options.command,
        request: options.request.to_string(),
        success: false,
        provider: "ollama",
        model: model.to_string(),
        requested_context_mode: options.context_mode,
        used_context_mode: context.used_mode,
        analysis_complete: context.analysis_complete,
        double_generation_authorized: options.compare_generate,
        preparation_duration_us,
        generation,
        context,
        rag: rag_report,
        runs,
        warnings,
        errors: Vec::new(),
    };

    if options.context_mode == ContextMode::Compare && !options.compare_generate {
        report.success = true;
        return Ok(AssistantExecutionOutput {
            report,
            raw_metrics: Vec::new(),
        });
    }

    if report.used_context_mode.is_none() {
        let error = context_rejected_error(&report.context, ContextMode::Symbol);
        attach_error(&mut report, error);
        return Ok(AssistantExecutionOutput {
            report,
            raw_metrics: Vec::new(),
        });
    }
    if options.context_mode == ContextMode::Compare
        && report
            .context
            .variant(ContextMode::Symbol)
            .is_some_and(|variant| !variant.report.usable_for_generation)
    {
        let error = context_rejected_error(&report.context, ContextMode::Symbol);
        attach_error(&mut report, error);
        return Ok(AssistantExecutionOutput {
            report,
            raw_metrics: Vec::new(),
        });
    }

    if options.verify_model {
        match llm.model_available(model).await {
            Ok(true) => {}
            Ok(false) => {
                attach_error(
                    &mut report,
                    AssistantStructuredError {
                        code: "model_unavailable".to_string(),
                        stage: "ollama_preflight".to_string(),
                        message: format!(
                        "configured model `{model}` is not present in the local Ollama inventory"
                    ),
                        context_mode: None,
                    },
                );
                return Ok(AssistantExecutionOutput {
                    report,
                    raw_metrics: Vec::new(),
                });
            }
            Err(error) => {
                attach_error(
                    &mut report,
                    AssistantStructuredError {
                        code: "ollama_unavailable".to_string(),
                        stage: "ollama_preflight".to_string(),
                        message: format!("local Ollama preflight failed: {error:#}"),
                        context_mode: None,
                    },
                );
                return Ok(AssistantExecutionOutput {
                    report,
                    raw_metrics: Vec::new(),
                });
            }
        }
    }

    let generation_options = GenerateOptions {
        num_predict: max_generated_tokens,
        temperature: options.temperature,
        seed: options.seed,
    };
    let mut raw_metrics = Vec::new();
    for (index, prompt) in prompts.iter().enumerate() {
        if report.runs[index].skipped_reason.is_some() {
            continue;
        }
        let mode = report.runs[index].context_mode;
        match llm
            .generate_timed_with_options(model, prompt, generation_options)
            .await
        {
            Ok(generated) => {
                report.runs[index].generated = true;
                report.runs[index].response = Some(generated.response);
                report.runs[index].metrics = Some(metrics_report(&generated.metrics));
                raw_metrics.push((mode, generated.metrics));
            }
            Err(error) => {
                let structured = AssistantStructuredError {
                    code: "generation_failed".to_string(),
                    stage: "ollama_generate".to_string(),
                    message: format!("local Ollama generation failed: {error:#}"),
                    context_mode: Some(mode),
                };
                report.runs[index].error = Some(structured.clone());
                report.errors.push(structured);
            }
        }
    }
    report.success = report.errors.is_empty()
        && report
            .runs
            .iter()
            .all(|run| run.generated && run.error.is_none());
    Ok(AssistantExecutionOutput {
        report,
        raw_metrics,
    })
}

fn validate_execution_options(model: &str, options: &AssistantExecutionOptions<'_>) -> Result<()> {
    if model.trim().is_empty() {
        bail!("Ollama model name must not be empty");
    }
    if options.request.trim().is_empty() {
        bail!("assistant request must not be empty");
    }
    if options.max_tokens == Some(0) {
        bail!("maximum generated tokens must be greater than zero");
    }
    if options
        .temperature
        .is_some_and(|value| !value.is_finite() || !(0.0..=2.0).contains(&value))
    {
        bail!("temperature must be a finite value between 0 and 2");
    }
    if options.compare_generate && options.context_mode != ContextMode::Compare {
        bail!("double generation requires context mode `compare`");
    }
    Ok(())
}

fn requested_run_modes(context: &ContextPreparation, requested: ContextMode) -> Vec<ContextMode> {
    if requested == ContextMode::Compare {
        vec![ContextMode::Legacy, ContextMode::Symbol]
    } else if let Some(used) = context.used_mode {
        vec![used]
    } else {
        vec![requested]
    }
}

fn compose_prompt(
    command: AssistantCommandKind,
    request: &str,
    project_context: &str,
    profile: Option<&ProfileContext>,
    memory: &MemoryContext,
    rag: &RagContext,
    brief: bool,
) -> String {
    match command {
        AssistantCommandKind::Ask => {
            build_prompt(request, project_context, profile, memory, rag, brief)
        }
        AssistantCommandKind::Plan => {
            build_plan_prompt(request, project_context, profile, memory, rag, brief)
        }
    }
}

fn prompt_report(prompt: &str) -> AssistantPromptReport {
    AssistantPromptReport {
        version: ASSISTANT_PROMPT_VERSION,
        bytes: prompt.len(),
        chars: prompt.chars().count(),
        estimated_tokens: estimate_tokens(prompt),
        token_estimator: "estimate:ceil_unicode_chars_div_4",
        content_hash: blake3::hash(prompt.as_bytes()).to_hex().to_string(),
    }
}

fn metrics_report(metrics: &GenerateMetrics) -> AssistantGenerationMetrics {
    let generated_tokens_per_second = match (
        metrics.eval_count,
        metrics.eval_duration.map(|duration| duration.as_secs_f64()),
    ) {
        (Some(tokens), Some(seconds)) if seconds > 0.0 => Some(tokens as f64 / seconds),
        _ => None,
    };
    AssistantGenerationMetrics {
        client_ms: duration_ms(metrics.client_duration),
        ollama_total_ms: metrics.ollama_total_duration.map(duration_ms),
        ollama_load_ms: metrics.ollama_load_duration.map(duration_ms),
        prompt_eval_count: metrics.prompt_eval_count,
        prompt_eval_ms: metrics.prompt_eval_duration.map(duration_ms),
        generated_tokens: metrics.eval_count,
        generation_ms: metrics.eval_duration.map(duration_ms),
        generated_tokens_per_second,
    }
}

fn rag_report(enabled: bool, rag: &RagContext) -> AssistantRagReport {
    AssistantRagReport {
        enabled,
        validated_active_v2: enabled,
        queries: rag.queries.clone(),
        hits: rag
            .hits
            .iter()
            .map(|hit| AssistantRagHitReport {
                source: hit.source.clone(),
                chunk_id: hit.chunk_id.clone(),
                score: hit.score,
                weighted_score: hit.weighted_score,
                matched_queries: hit.matched_queries.clone(),
            })
            .collect(),
    }
}

fn context_rejected_error(
    context: &ContextPreparation,
    mode: ContextMode,
) -> AssistantStructuredError {
    let reasons = context
        .variant(mode)
        .map(|variant| {
            variant
                .report
                .rejection_reasons
                .iter()
                .map(|reason| reason.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|reasons| !reasons.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    AssistantStructuredError {
        code: "context_rejected".to_string(),
        stage: "context_preparation".to_string(),
        message: format!("symbol context is not safe for generation: {reasons}"),
        context_mode: Some(mode),
    }
}

fn attach_error(report: &mut AssistantCommandReport, error: AssistantStructuredError) {
    if let Some(mode) = error.context_mode {
        if let Some(run) = report.runs.iter_mut().find(|run| run.context_mode == mode) {
            run.error = Some(error.clone());
        }
    }
    report.errors.push(error);
    report.success = false;
}

fn estimate_tokens(value: &str) -> usize {
    value.chars().count().saturating_add(3) / 4
}

fn duration_us(duration: std::time::Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

fn duration_ms(duration: std::time::Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::sync::mpsc::{self, Receiver};
    use std::thread;
    use std::time::Duration;

    use crate::{AskOptions, ContextFallbackPolicy, ContextMode, OpticCode, PlanOptions};

    const TAGS: &str =
        r#"{"models":[{"name":"qwen2.5-coder:14b","model":"qwen2.5-coder:14b","size":1}]}"#;
    const GENERATE: &str = r#"{"response":"mock response","done":true,"total_duration":1000000,"load_duration":1000,"prompt_eval_count":20,"prompt_eval_duration":2000,"eval_count":5,"eval_duration":3000}"#;

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/java-index-mini")
    }

    fn app(responses: Vec<&'static str>) -> (OpticCode, Receiver<String>) {
        let (url, requests) = spawn_mock(responses);
        let app = OpticCode::try_new(url, "qwen2.5-coder:14b")
            .unwrap()
            .with_http_timeout(Duration::from_secs(2))
            .unwrap();
        (app, requests)
    }

    fn ask_options(prompt: &str, mode: ContextMode) -> AskOptions {
        AskOptions {
            workspace: fixture(),
            prompt: prompt.to_string(),
            profile: None,
            include_memory: false,
            include_rag: false,
            rag_index: PathBuf::from("unused"),
            rag_limit: 4,
            brief: false,
            max_tokens: Some(32),
            temperature: Some(0.0),
            seed: Some(7),
            context_mode: mode,
            fallback_policy: ContextFallbackPolicy::Legacy,
            compare_generate: false,
            verify_model: true,
        }
    }

    #[tokio::test]
    async fn ask_legacy_generates_one_measured_response() {
        let (app, requests) = app(vec![TAGS, GENERATE]);

        let report = app
            .ask_with_report(ask_options("Locate Helpers#ping().", ContextMode::Legacy))
            .await
            .unwrap();

        assert!(report.success);
        assert_eq!(report.used_context_mode, Some(ContextMode::Legacy));
        assert_eq!(report.runs.len(), 1);
        assert_eq!(
            report.runs[0].metrics.as_ref().unwrap().prompt_eval_count,
            Some(20)
        );
        assert!(requests.recv().unwrap().starts_with("GET /api/tags "));
        assert!(requests.recv().unwrap().starts_with("POST /api/generate "));
    }

    #[tokio::test]
    async fn plan_symbol_generates_when_analysis_is_complete() {
        let (app, _requests) = app(vec![TAGS, GENERATE]);
        let report = app
            .plan_with_report(PlanOptions {
                workspace: fixture(),
                goal: "Find plugin.yml main and commands.".to_string(),
                profile: None,
                include_memory: false,
                include_rag: false,
                rag_index: PathBuf::from("unused"),
                rag_limit: 4,
                brief: true,
                max_tokens: Some(32),
                temperature: Some(0.0),
                seed: Some(7),
                context_mode: ContextMode::Symbol,
                fallback_policy: ContextFallbackPolicy::Refuse,
                compare_generate: false,
                verify_model: true,
            })
            .await
            .unwrap();

        assert!(report.success);
        assert_eq!(report.used_context_mode, Some(ContextMode::Symbol));
        assert!(report.runs[0].generated);
    }

    #[tokio::test]
    async fn compare_does_not_contact_ollama_without_explicit_generation() {
        let app = OpticCode::try_new("http://127.0.0.1:9", "qwen2.5-coder:14b").unwrap();
        let report = app
            .ask_with_report(ask_options(
                "Locate dev.opticcode.util.Helpers#ping().",
                ContextMode::Compare,
            ))
            .await
            .unwrap();

        assert!(report.success);
        assert_eq!(report.runs.len(), 2);
        assert!(report.runs.iter().all(|run| !run.generated));
        assert!(!report.double_generation_authorized);
    }

    #[tokio::test]
    async fn compare_generates_two_distinct_runs_only_when_authorized() {
        let (app, requests) = app(vec![TAGS, GENERATE, GENERATE]);
        let mut options = ask_options(
            "Locate dev.opticcode.util.Helpers#ping().",
            ContextMode::Compare,
        );
        options.compare_generate = true;

        let report = app.ask_with_report(options).await.unwrap();

        assert!(report.success);
        assert!(report.double_generation_authorized);
        assert_eq!(report.runs.iter().filter(|run| run.generated).count(), 2);
        assert!(requests.recv().unwrap().starts_with("GET /api/tags "));
        assert!(requests.recv().unwrap().starts_with("POST /api/generate "));
        assert!(requests.recv().unwrap().starts_with("POST /api/generate "));
    }

    #[tokio::test]
    async fn symbol_fallback_is_explicit_in_the_generated_report() {
        let (app, _requests) = app(vec![TAGS, GENERATE]);
        let report = app
            .ask_with_report(ask_options(
                "Inspect dev.opticcode.util.Helpers#create(String).",
                ContextMode::Symbol,
            ))
            .await
            .unwrap();

        assert!(report.success);
        assert_eq!(report.used_context_mode, Some(ContextMode::Legacy));
        assert!(report.context.fallback.as_ref().unwrap().applied);
        assert_eq!(report.runs[0].context_mode, ContextMode::Legacy);
    }

    #[tokio::test]
    async fn strict_ambiguous_symbol_context_fails_before_network_access() {
        let app = OpticCode::try_new("http://127.0.0.1:9", "qwen2.5-coder:14b").unwrap();
        let mut options = ask_options("Inspect Duplicate.", ContextMode::Symbol);
        options.fallback_policy = ContextFallbackPolicy::Refuse;

        let report = app.ask_with_report(options).await.unwrap();

        assert!(!report.success);
        assert_eq!(report.used_context_mode, None);
        assert_eq!(report.errors[0].code, "context_rejected");
        assert!(report.runs.iter().all(|run| !run.generated));
    }

    fn spawn_mock(responses: Vec<&'static str>) -> (String, Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            for body in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                sender.send(read_http_request(&mut stream)).unwrap();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        (format!("http://{address}"), receiver)
    }

    fn read_http_request(stream: &mut std::net::TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 2048];
        loop {
            let read = stream.read(&mut buffer).unwrap_or(0);
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
            let Some(header_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") else {
                continue;
            };
            let header_end = header_end + 4;
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.strip_prefix("content-length: ")
                        .or_else(|| line.strip_prefix("Content-Length: "))
                })
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if bytes.len() >= header_end + content_length {
                break;
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }
}
