mod assistant_runtime;
mod context_runtime;
mod eval_runtime;
mod protocol;

pub use assistant_runtime::{
    AssistantCommandKind, AssistantCommandReport, AssistantGenerationConfiguration,
    AssistantGenerationMetrics, AssistantPromptReport, AssistantRagHitReport, AssistantRagReport,
    AssistantRunReport, AssistantStructuredError, ASSISTANT_PROMPT_VERSION,
    ASSISTANT_RUN_SCHEMA_VERSION,
};
pub use context_runtime::{
    prepare_assistant_context, AssistantContextFile, AssistantContextSnippet,
    AssistantContextTimings, ContextComparison, ContextFallback, ContextFallbackPolicy,
    ContextMode, ContextPreparation, ContextRejectionReason, ContextVariantReport,
    PreparedContextVariant, ASSISTANT_CONTEXT_SCHEMA_VERSION,
};
pub use eval_runtime::{enrich_evaluation_with_llm, EvalLlmRuntimeOptions};
pub use protocol::{
    assistant_event_channel, generated_request_id, validate_assistant_request_id,
    AssistantEventReceiver, AssistantEventSink, AssistantProtocolEvent,
    AssistantProtocolEventPayload, AssistantProtocolSession, ASSISTANT_PROTOCOL_ID,
    ASSISTANT_PROTOCOL_SCHEMA_VERSION, DEFAULT_ASSISTANT_EVENT_CAPACITY,
    MAX_ASSISTANT_REQUEST_ID_BYTES,
};

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use opticcode_llm::{
    default_ollama_keep_alive, HealthRequest, LlmProvider, OllamaProvider,
    DEFAULT_PROVIDER_TIMEOUT_MS, MAX_PROVIDER_TIMEOUT_MS,
};
pub use opticcode_llm::{parse_keep_alive, CancellationToken, GenerateMetrics, ProviderId};
use opticcode_tools::search_rag_index_queries;
use serde::Serialize;

use assistant_runtime::{execute_assistant, AssistantExecutionOptions, AssistantExecutionOutput};

pub struct OpticCode {
    llm: Arc<dyn LlmProvider>,
    model: String,
    keep_alive: Option<String>,
    http_timeout: Duration,
}

pub struct AskOptions {
    pub workspace: PathBuf,
    pub prompt: String,
    pub profile: Option<String>,
    pub include_memory: bool,
    pub include_rag: bool,
    pub rag_index: PathBuf,
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

pub struct PlanOptions {
    pub workspace: PathBuf,
    pub goal: String,
    pub profile: Option<String>,
    pub include_memory: bool,
    pub include_rag: bool,
    pub rag_index: PathBuf,
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

pub struct AssistantOutput {
    pub text: String,
    pub metrics: GenerateMetrics,
    pub report: AssistantCommandReport,
}

#[derive(Debug, Clone)]
pub struct ProfileContext {
    pub id: String,
    pub source: PathBuf,
    pub content: String,
}

#[derive(Debug, Clone, Default)]
pub struct MemoryContext {
    pub entries: Vec<MemoryEntry>,
}

#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub scope: String,
    pub source: PathBuf,
    pub content: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RagContext {
    pub index: Option<PathBuf>,
    pub queries: Vec<String>,
    pub hits: Vec<RagContextHit>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RagContextHit {
    pub source: String,
    pub chunk_id: String,
    pub score: usize,
    pub weighted_score: usize,
    pub matched_queries: Vec<String>,
    pub query_scores: Vec<RagQueryScore>,
    pub preview: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RagQueryScore {
    pub query: String,
    pub score: usize,
}

pub const DEFAULT_PROFILE: &str = "minecraft-java-1.8";
const MAX_MEMORY_ENTRY_CHARS: usize = 2_500;
const MAX_MEMORY_TOTAL_CHARS: usize = 7_000;
const MAX_RAG_HITS: usize = 6;
const MAX_RAG_TOTAL_CHARS: usize = 4_500;

fn duration_ms(duration: Duration) -> u64 {
    let nanos = duration.as_nanos();
    nanos
        .saturating_add(999_999)
        .checked_div(1_000_000)
        .unwrap_or(u128::from(u64::MAX))
        .min(u128::from(u64::MAX)) as u64
}

impl OpticCode {
    pub fn new(ollama_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            llm: Arc::new(OllamaProvider::new(ollama_url)),
            model: model.into(),
            keep_alive: default_ollama_keep_alive(),
            http_timeout: Duration::from_millis(DEFAULT_PROVIDER_TIMEOUT_MS),
        }
    }

    pub fn try_new(ollama_url: impl AsRef<str>, model: impl Into<String>) -> Result<Self> {
        let model = model.into();
        if model.trim().is_empty() {
            bail!("Ollama model name must not be empty");
        }
        Ok(Self {
            llm: Arc::new(OllamaProvider::try_new(ollama_url)?),
            model,
            keep_alive: default_ollama_keep_alive(),
            http_timeout: Duration::from_millis(DEFAULT_PROVIDER_TIMEOUT_MS),
        })
    }

    pub fn with_provider(provider: Arc<dyn LlmProvider>, model: impl Into<String>) -> Result<Self> {
        let model = model.into();
        if model.trim().is_empty() {
            bail!("LLM model name must not be empty");
        }
        Ok(Self {
            llm: provider,
            model,
            keep_alive: None,
            http_timeout: Duration::from_millis(DEFAULT_PROVIDER_TIMEOUT_MS),
        })
    }

    pub fn with_keep_alive(mut self, keep_alive: Option<String>) -> Self {
        self.keep_alive = keep_alive;
        self
    }

    pub fn with_http_timeout(mut self, timeout: Duration) -> Result<Self> {
        if timeout.is_zero() || timeout > Duration::from_millis(MAX_PROVIDER_TIMEOUT_MS) {
            bail!(
                "LLM provider timeout must be between 1 ns and {} milliseconds",
                MAX_PROVIDER_TIMEOUT_MS
            );
        }
        self.http_timeout = timeout;
        Ok(self)
    }

    pub fn provider_id(&self) -> ProviderId {
        self.llm.id()
    }

    pub fn provider_capabilities(&self) -> opticcode_llm::ProviderCapabilities {
        self.llm.capabilities()
    }

    pub async fn ensure_model_available(&self) -> Result<()> {
        let health = self
            .llm
            .health(HealthRequest {
                model: Some(self.model.clone()),
                timeout_ms: duration_ms(self.http_timeout),
                ..HealthRequest::default()
            })
            .await?;
        if health.model_available != Some(true) {
            bail!(
                "configured model `{}` is not present in the provider inventory",
                self.model
            );
        }
        Ok(())
    }

    pub async fn ask_with_project_context(&self, options: AskOptions) -> Result<String> {
        Ok(self.ask_with_metrics(options).await?.text)
    }

    pub async fn ask_with_metrics(&self, options: AskOptions) -> Result<AssistantOutput> {
        execution_to_output(self.execute_ask(options).await?)
    }

    pub async fn ask_with_report(&self, options: AskOptions) -> Result<AssistantCommandReport> {
        Ok(self.execute_ask(options).await?.report)
    }

    pub async fn ask_with_protocol(
        &self,
        options: AskOptions,
        session: AssistantProtocolSession,
    ) -> Result<AssistantCommandReport> {
        Ok(self.execute_ask_inner(options, Some(session)).await?.report)
    }

    pub async fn plan_with_project_context(&self, options: PlanOptions) -> Result<String> {
        Ok(self.plan_with_metrics(options).await?.text)
    }

    pub async fn plan_with_metrics(&self, options: PlanOptions) -> Result<AssistantOutput> {
        execution_to_output(self.execute_plan(options).await?)
    }

    pub async fn plan_with_report(&self, options: PlanOptions) -> Result<AssistantCommandReport> {
        Ok(self.execute_plan(options).await?.report)
    }

    pub async fn plan_with_protocol(
        &self,
        options: PlanOptions,
        session: AssistantProtocolSession,
    ) -> Result<AssistantCommandReport> {
        Ok(self
            .execute_plan_inner(options, Some(session))
            .await?
            .report)
    }

    async fn execute_ask(&self, options: AskOptions) -> Result<AssistantExecutionOutput> {
        self.execute_ask_inner(options, None).await
    }

    async fn execute_ask_inner(
        &self,
        options: AskOptions,
        protocol: Option<AssistantProtocolSession>,
    ) -> Result<AssistantExecutionOutput> {
        execute_assistant(
            self.llm.as_ref(),
            &self.model,
            AssistantExecutionOptions {
                command: AssistantCommandKind::Ask,
                workspace: &options.workspace,
                request: &options.prompt,
                profile: options.profile.as_deref(),
                include_memory: options.include_memory,
                include_rag: options.include_rag,
                rag_index: &options.rag_index,
                rag_limit: options.rag_limit,
                brief: options.brief,
                max_tokens: options.max_tokens,
                temperature: options.temperature,
                seed: options.seed,
                context_mode: options.context_mode,
                fallback_policy: options.fallback_policy,
                compare_generate: options.compare_generate,
                verify_model: options.verify_model,
                keep_alive: self.keep_alive.clone(),
                http_timeout: self.http_timeout,
                protocol,
            },
        )
        .await
    }

    async fn execute_plan(&self, options: PlanOptions) -> Result<AssistantExecutionOutput> {
        self.execute_plan_inner(options, None).await
    }

    async fn execute_plan_inner(
        &self,
        options: PlanOptions,
        protocol: Option<AssistantProtocolSession>,
    ) -> Result<AssistantExecutionOutput> {
        execute_assistant(
            self.llm.as_ref(),
            &self.model,
            AssistantExecutionOptions {
                command: AssistantCommandKind::Plan,
                workspace: &options.workspace,
                request: &options.goal,
                profile: options.profile.as_deref(),
                include_memory: options.include_memory,
                include_rag: options.include_rag,
                rag_index: &options.rag_index,
                rag_limit: options.rag_limit,
                brief: options.brief,
                max_tokens: options.max_tokens,
                temperature: options.temperature,
                seed: options.seed,
                context_mode: options.context_mode,
                fallback_policy: options.fallback_policy,
                compare_generate: options.compare_generate,
                verify_model: options.verify_model,
                keep_alive: self.keep_alive.clone(),
                http_timeout: self.http_timeout,
                protocol,
            },
        )
        .await
    }
}

fn execution_to_output(execution: AssistantExecutionOutput) -> Result<AssistantOutput> {
    if !execution.report.success {
        let details = execution
            .report
            .errors
            .iter()
            .map(|error| format!("{}: {}", error.code, error.message))
            .collect::<Vec<_>>()
            .join("; ");
        bail!("assistant generation did not complete: {details}");
    }
    let (mode, text) = {
        let run = execution
            .report
            .generated_run()
            .context("assistant report contains no generated response")?;
        (
            run.context_mode,
            run.response
                .clone()
                .context("assistant generated run contains no response")?,
        )
    };
    let metrics = execution
        .raw_metrics
        .into_iter()
        .find(|(candidate, _)| *candidate == mode)
        .map(|(_, metrics)| metrics)
        .context("assistant generated run contains no raw metrics")?;
    Ok(AssistantOutput {
        text,
        metrics,
        report: execution.report,
    })
}

pub fn load_profile_for_workspace(
    workspace: &Path,
    profile_id: Option<&str>,
) -> Result<Option<ProfileContext>> {
    let Some(profile_id) = profile_id else {
        return Ok(None);
    };

    let profile_id = profile_id.trim();
    if profile_id.is_empty() || profile_id.eq_ignore_ascii_case("none") {
        return Ok(None);
    }

    let relative = Path::new("skills")
        .join("profiles")
        .join(profile_id)
        .join("profile.md");
    let candidates = vec![
        workspace.join(&relative),
        std::env::current_dir()?.join(&relative),
    ];

    for candidate in candidates {
        if candidate.exists() {
            let content = fs::read_to_string(&candidate)
                .with_context(|| format!("failed to read profile {}", candidate.display()))?;
            return Ok(Some(ProfileContext {
                id: profile_id.to_string(),
                source: candidate,
                content,
            }));
        }
    }

    anyhow::bail!("profile `{profile_id}` not found under skills/profiles/{profile_id}/profile.md")
}

pub fn load_memory_for_workspace(
    workspace: &Path,
    profile_id: Option<&str>,
) -> Result<MemoryContext> {
    let mut candidates = Vec::new();
    let current_dir = std::env::current_dir()?;

    for root in [workspace.to_path_buf(), current_dir] {
        candidates.push(("global".to_string(), root.join("skills/memory/global.md")));

        if let Some(profile_id) = normalized_id(profile_id) {
            candidates.push((
                format!("profile:{profile_id}"),
                root.join("skills")
                    .join("memory")
                    .join("profiles")
                    .join(format!("{profile_id}.md")),
            ));
        }

        candidates.push(("project".to_string(), root.join(".opticcode/memory.md")));
    }

    let mut entries = Vec::new();
    let mut seen = Vec::<PathBuf>::new();
    let mut total_chars = 0usize;

    for (scope, path) in candidates {
        if !path.exists() || seen.iter().any(|seen_path| seen_path == &path) {
            continue;
        }

        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read memory {}", path.display()))?;
        let content = truncate_memory(&raw, MAX_MEMORY_ENTRY_CHARS);
        let content_chars = content.chars().count();
        if total_chars + content_chars > MAX_MEMORY_TOTAL_CHARS {
            break;
        }

        seen.push(path.clone());
        total_chars += content_chars;
        entries.push(MemoryEntry {
            scope,
            source: path,
            content,
        });
    }

    Ok(MemoryContext { entries })
}

pub fn load_rag_context(index_dir: &Path, query: &str, limit: usize) -> Result<RagContext> {
    let chunks_path = index_dir.join("chunks.jsonl");
    let current_path = index_dir.join("CURRENT");
    if !current_path.is_file() {
        if chunks_path.exists() {
            bail!(
                "refusing legacy RAG index without CURRENT; publish a validated schema v2 generation"
            );
        }
        bail!("RAG is enabled but no published schema v2 generation exists (missing CURRENT)");
    }

    let limit = limit.clamp(1, MAX_RAG_HITS);
    let mut hits = Vec::new();
    let queries = expand_rag_queries(query);

    let search_reports = search_rag_index_queries(index_dir, &queries, limit)?;
    for (expanded_query, report) in queries.iter().zip(search_reports) {
        for hit in report.hits {
            if let Some(existing) = hits.iter_mut().find(|existing: &&mut RagContextHit| {
                existing.source == hit.document_path && existing.chunk_id == hit.chunk_id
            }) {
                existing.score = existing.score.max(hit.score);
                if !existing.matched_queries.contains(expanded_query) {
                    existing.matched_queries.push(expanded_query.clone());
                    existing.matched_queries.sort();
                }
                upsert_query_score(&mut existing.query_scores, expanded_query, hit.score);
                existing.weighted_score = weighted_query_score(&existing.query_scores);
                continue;
            }
            let query_scores = vec![RagQueryScore {
                query: expanded_query.clone(),
                score: hit.score,
            }];
            hits.push(RagContextHit {
                source: hit.document_path,
                chunk_id: hit.chunk_id,
                score: hit.score,
                weighted_score: weighted_query_score(&query_scores),
                matched_queries: vec![expanded_query.clone()],
                query_scores,
                preview: hit.preview,
            });
        }
    }

    select_rag_hits_for_prompt(&mut hits);
    hits.truncate(limit);

    Ok(RagContext {
        index: Some(index_dir.to_path_buf()),
        queries,
        hits,
    })
}

impl ProfileContext {
    pub fn to_display_string(&self) -> String {
        format!(
            "Profile: {}\nSource: {}\n\n{}",
            self.id,
            self.source.display(),
            self.content
        )
    }
}

impl MemoryContext {
    pub fn to_display_string(&self) -> String {
        if self.entries.is_empty() {
            return "Memory: none".to_string();
        }

        let mut out = String::new();
        out.push_str(&format!("Memory entries: {}\n", self.entries.len()));
        for entry in &self.entries {
            out.push_str(&format!("\n## {}\n", entry.scope));
            out.push_str(&format!("Source: {}\n\n", entry.source.display()));
            out.push_str(&entry.content);
            if !entry.content.ends_with('\n') {
                out.push('\n');
            }
        }
        out
    }
}

impl RagContext {
    pub fn to_display_string(&self) -> String {
        let mut out = String::new();
        out.push_str("RAG context\n");
        out.push_str(&format!(
            "Index: {}\n",
            self.index
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "none".to_string())
        ));

        out.push_str("\nExpanded queries:\n");
        if self.queries.is_empty() {
            out.push_str("- none\n");
        } else {
            for query in &self.queries {
                out.push_str(&format!("- {query}\n"));
            }
        }

        out.push_str("\nInjected hits:\n");
        if self.hits.is_empty() {
            out.push_str("- none\n");
        } else {
            for hit in &self.hits {
                out.push_str(&format!("\nsource: {}\n", hit.source));
                out.push_str(&format!("chunk: {}\n", hit.chunk_id));
                out.push_str(&format!("score: {}\n", hit.score));
                out.push_str(&format!("weighted_score: {}\n", hit.weighted_score));
                out.push_str(&format!(
                    "matched_queries: {}\n",
                    if hit.matched_queries.is_empty() {
                        "none".to_string()
                    } else {
                        hit.matched_queries.join(", ")
                    }
                ));
                out.push_str(&format!(
                    "query_scores: {}\n",
                    if hit.query_scores.is_empty() {
                        "none".to_string()
                    } else {
                        format_query_scores(&hit.query_scores)
                    }
                ));
                out.push_str(&hit.preview);
                if !hit.preview.ends_with('\n') {
                    out.push('\n');
                }
            }
        }

        out
    }
}

fn build_prompt(
    user_prompt: &str,
    project_context: &str,
    profile: Option<&ProfileContext>,
    memory: &MemoryContext,
    rag: &RagContext,
    brief: bool,
) -> String {
    let style = if brief {
        "Mode court : reponds en 6 lignes maximum, sans introduction longue."
    } else {
        "Mode normal : reste precis, verifiable et proportionne a la demande."
    };
    let profile_context = format_profile_context(profile);
    let memory_context = format_memory_context(memory);
    let rag_context = format_rag_context(rag);

    format!(
        r#"[SYSTEM {prompt_version} ask]
Tu es OpticCode, un assistant code local specialise Java Minecraft legacy.

[POLICY minecraft-java8-v1]
Contraintes obligatoires :
- cible Java 8 stricte ;
- cible Bukkit/Spigot/PandaSpigot 1.8.8 / 1.8.9 ;
- pas de records, pas de var, pas d'API moderne inutile ;
- pour Bukkit 1.8.8, ne pas utiliser api-version dans plugin.yml ;
- pour gunpowder en 1.8.8, preferer Material.SULPHUR a Material.GUNPOWDER ;
- proposer un plan et signaler les risques avant toute modification ;
- si une information legacy est incertaine, le dire clairement.
{style}

[TOOLS readonly-context-v1]
- aucun outil d'ecriture ou d'execution n'est disponible pendant cette reponse ;
- ne pretends jamais avoir modifie, compile ou teste un fichier.

[PROFILE]
{profile_context}

[HISTORY]
{memory_context}

[REQUEST]
{user_prompt}

[DYNAMIC_CONTEXT]
Contexte projet :
{project_context}

Connaissances RAG :
{rag_context}
"#,
        prompt_version = ASSISTANT_PROMPT_VERSION
    )
}

fn build_plan_prompt(
    goal: &str,
    project_context: &str,
    profile: Option<&ProfileContext>,
    memory: &MemoryContext,
    rag: &RagContext,
    brief: bool,
) -> String {
    let format_instruction = if brief {
        "Format attendu :\n- 6 puces maximum ;\n- chaque puce doit etre courte ;\n- inclure seulement les risques et verifications essentiels."
    } else {
        "Format attendu :\n1. Resume de l'objectif\n2. Fichiers a inspecter ou creer\n3. Plan d'implementation\n4. Points legacy Bukkit/Java 8 a surveiller\n5. Verifications a lancer\n6. Questions bloquantes, seulement si necessaire"
    };
    let profile_context = format_profile_context(profile);
    let memory_context = format_memory_context(memory);
    let rag_context = format_rag_context(rag);

    format!(
        r#"[SYSTEM {prompt_version} plan]
Tu es OpticCode en mode plan. Tu dois produire uniquement un plan d'action. Tu ne dois pas ecrire un patch complet et tu ne dois pas pretendre avoir modifie des fichiers.

[POLICY minecraft-java8-plan-v1]
Contraintes obligatoires :
- cible Java 8 stricte ;
- cible Bukkit/Spigot/PandaSpigot 1.8.8 / 1.8.9 ;
- pas de records, pas de var, pas d'API moderne inutile ;
- pour Bukkit 1.8.8, ne pas utiliser api-version dans plugin.yml ;
- si la demande concerne gunpowder en 1.8.8, preferer Material.SULPHUR a Material.GUNPOWDER ;
- signaler les risques, les fichiers probables, les tests a lancer et les informations manquantes ;
- si une information legacy est incertaine, le dire clairement.

Interdictions en mode plan :
- ne pas inclure de bloc de code ;
- ne pas ecrire de classe complete ;
- ne pas ecrire de fichier plugin.yml complet ;
- ne pas inventer de package exact si le projet ne le montre pas ;
- ne pas citer une regle legacy qui ne concerne pas l'objectif.

{format_instruction}

[TOOLS readonly-context-v1]
- aucun outil d'ecriture ou d'execution n'est disponible pendant cette reponse ;
- ne pretends jamais avoir modifie, compile ou teste un fichier.

[PROFILE]
{profile_context}

[HISTORY]
{memory_context}

[REQUEST]
{goal}

[DYNAMIC_CONTEXT]
Contexte projet :
{project_context}

Connaissances RAG :
{rag_context}
"#,
        prompt_version = ASSISTANT_PROMPT_VERSION
    )
}

fn format_profile_context(profile: Option<&ProfileContext>) -> String {
    profile.map_or_else(
        || "none".to_string(),
        |profile| format!("id: {}\n{}", profile.id, profile.content),
    )
}

fn format_memory_context(memory: &MemoryContext) -> String {
    if memory.entries.is_empty() {
        return "none".to_string();
    }

    memory
        .entries
        .iter()
        .map(|entry| format!("scope: {}\n{}", entry.scope, entry.content))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn format_rag_context(rag: &RagContext) -> String {
    if rag.hits.is_empty() {
        return "none".to_string();
    }

    let mut out = String::new();
    let mut total_chars = 0usize;
    for hit in &rag.hits {
        let mut preview = hit.preview.clone();
        let remaining = MAX_RAG_TOTAL_CHARS.saturating_sub(total_chars);
        if remaining == 0 {
            break;
        }
        preview = truncate_chars(&preview, remaining);
        total_chars += preview.chars().count();

        out.push_str(&format!(
            "\nsource: {}\nscore: {}\n{}\n",
            hit.source, hit.score, preview
        ));
    }

    out
}

fn expand_rag_queries(query: &str) -> Vec<String> {
    let lower = query.to_ascii_lowercase();
    let mut queries = vec![query.trim().to_string()];

    if lower.contains("spawner") {
        queries.push("spawner".to_string());
        queries.push("mob_spawner".to_string());
        queries.push("MOB_SPAWNER".to_string());
    }

    if lower.contains("nether wart")
        || lower.contains("nether_wart")
        || lower.contains("nether stalk")
        || lower.contains("nether_stalk")
    {
        queries.push("nether wart".to_string());
        queries.push("nether_stalk".to_string());
        queries.push("NETHER_STALK".to_string());
    }

    if lower.contains("pelle")
        || lower.contains("pelles")
        || lower.contains("shovel")
        || lower.contains("spade")
    {
        queries.push("shovel".to_string());
        queries.push("spade".to_string());
        queries.push("WOOD_SPADE".to_string());
        queries.push("DIAMOND_SPADE".to_string());
    }

    if lower.contains("spawn egg")
        || lower.contains("spawn_egg")
        || lower.contains("oeuf")
        || lower.contains("oeufs")
    {
        queries.push("spawn_egg".to_string());
        queries.push("monster_placer".to_string());
        queries.push("monsterPlacer".to_string());
        queries.push("MONSTER_EGG".to_string());
        queries.push("Material.MONSTER_EGG".to_string());
    }

    if lower.contains("gunpowder") || lower.contains("sulphur") || lower.contains("poudre") {
        queries.push("gunpowder".to_string());
        queries.push("SULPHUR".to_string());
        queries.push("Material.SULPHUR".to_string());
    }

    queries.retain(|value| !value.trim().is_empty());
    queries.sort();
    queries.dedup();
    queries
}

fn sort_rag_hits_for_prompt(hits: &mut [RagContextHit]) {
    hits.sort_by(|left, right| {
        rag_source_priority(&left.source)
            .cmp(&rag_source_priority(&right.source))
            .then_with(|| right.weighted_score.cmp(&left.weighted_score))
            .then_with(|| right.score.cmp(&left.score))
            .then_with(|| left.source.cmp(&right.source))
    });
}

fn upsert_query_score(scores: &mut Vec<RagQueryScore>, query: &str, score: usize) {
    if let Some(existing) = scores.iter_mut().find(|entry| entry.query == query) {
        existing.score = existing.score.max(score);
    } else {
        scores.push(RagQueryScore {
            query: query.to_string(),
            score,
        });
    }

    scores.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.query.cmp(&right.query))
    });
}

fn weighted_query_score(scores: &[RagQueryScore]) -> usize {
    scores
        .iter()
        .map(|entry| entry.score * rag_query_weight(&entry.query))
        .sum()
}

fn rag_query_weight(query: &str) -> usize {
    let lower = query.to_ascii_lowercase();
    match lower.as_str() {
        "material.sulphur"
        | "sulphur"
        | "mob_spawner"
        | "nether_stalk"
        | "material.monster_egg"
        | "spawn_egg"
        | "monster_placer"
        | "monsterplacer" => 4,
        "nether wart" | "gunpowder" => 3,
        "spawner" | "spade" => 2,
        "diamond_spade" | "monster_egg" | "wood_spade" => 1,
        "shovel" => 1,
        _ => 1,
    }
}

fn format_query_scores(scores: &[RagQueryScore]) -> String {
    scores
        .iter()
        .map(|score| {
            let weight = rag_query_weight(&score.query);
            if weight == 1 {
                format!("{}={}", score.query, score.score)
            } else {
                format!("{}={}x{}", score.query, score.score, weight)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn select_rag_hits_for_prompt(hits: &mut Vec<RagContextHit>) {
    sort_rag_hits_for_prompt(hits);

    let mut selected = Vec::new();
    let mut seen_keys = BTreeSet::new();

    for hit in hits.drain(..) {
        if is_low_value_rag_hit(&hit) {
            continue;
        }
        if let Some(key) = rag_duplicate_key(&hit) {
            if !seen_keys.insert(key) {
                continue;
            }
        }
        selected.push(hit);
    }

    *hits = selected;
}

fn is_low_value_rag_hit(hit: &RagContextHit) -> bool {
    hit.score <= 1
        && legacy_concepts(&hit.preview).is_empty()
        && !hit.source.starts_with("opticcode:docs/")
        && !hit.source.starts_with("opticcode:skills/")
}

fn rag_source_priority(source: &str) -> u8 {
    if source == "opticcode:docs/minecraft-legacy-rules.md" {
        0
    } else if source.starts_with("opticcode:skills/profiles/") {
        1
    } else if source.starts_with("plugin:") {
        2
    } else if source.starts_with("resource-pack:assets/minecraft/lang/") {
        3
    } else if source.starts_with("resource-pack:") {
        4
    } else if source.starts_with("pandaspigot:") && source.contains(".patch") {
        5
    } else if source.starts_with("pandaspigot:") {
        6
    } else if source.starts_with("opticcode:docs/") {
        7
    } else if source.starts_with("opticcode:crates/") {
        8
    } else {
        9
    }
}

fn rag_duplicate_key(hit: &RagContextHit) -> Option<String> {
    let concepts = legacy_concepts(&hit.preview);
    if concepts.is_empty() {
        return None;
    }

    let group = if hit.source.starts_with("opticcode:docs/")
        || hit.source.starts_with("opticcode:skills/")
        || hit.source.starts_with("opticcode:crates/")
    {
        "opticcode"
    } else if hit.source.starts_with("resource-pack:") {
        "resource-pack"
    } else if hit.source.starts_with("plugin:") {
        "plugin"
    } else if hit.source.starts_with("pandaspigot:") {
        "pandaspigot"
    } else {
        "other"
    };

    Some(format!("{group}:{}", concepts.join("+")))
}

fn legacy_concepts(value: &str) -> Vec<&'static str> {
    let lower = value.to_ascii_lowercase();
    let mut concepts = Vec::new();

    if lower.contains("sulphur") || lower.contains("gunpowder") {
        concepts.push("gunpowder");
    }
    if lower.contains("spade") || lower.contains("shovel") {
        concepts.push("spade");
    }
    if lower.contains("mob_spawner") || lower.contains("spawner") {
        concepts.push("spawner");
    }
    if lower.contains("nether_stalk")
        || lower.contains("nether_wart")
        || lower.contains("nether wart")
    {
        concepts.push("nether_stalk");
    }
    if lower.contains("spawn_egg")
        || lower.contains("monster_placer")
        || lower.contains("monsterplacer")
        || lower.contains("monster egg")
    {
        concepts.push("spawn_egg");
    }

    concepts
}

fn normalized_id(value: Option<&str>) -> Option<&str> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("none"))
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut iter = value.chars();
    let mut content = iter.by_ref().take(max_chars).collect::<String>();
    if iter.next().is_some() {
        content.push_str("\n[truncated]\n");
    }
    content
}

fn truncate_memory(value: &str, max_chars: usize) -> String {
    truncate_chars(value, max_chars)
}

#[cfg(test)]
mod tests {
    use super::{
        build_plan_prompt, build_prompt, expand_rag_queries, format_rag_context, rag_duplicate_key,
        rag_query_weight, select_rag_hits_for_prompt, sort_rag_hits_for_prompt,
        weighted_query_score, MemoryContext, MemoryEntry, ProfileContext, RagContext,
        RagContextHit, RagQueryScore,
    };
    use std::path::PathBuf;

    fn test_profile() -> ProfileContext {
        ProfileContext {
            id: "minecraft-java-1.8".to_string(),
            source: PathBuf::from("skills/profiles/minecraft-java-1.8/profile.md"),
            content: "Regle profil: Java 8 et Material.SULPHUR.".to_string(),
        }
    }

    fn test_memory() -> MemoryContext {
        MemoryContext {
            entries: vec![MemoryEntry {
                scope: "profile:minecraft-java-1.8".to_string(),
                source: PathBuf::from("skills/memory/profiles/minecraft-java-1.8.md"),
                content: "Memoire: ne jamais proposer Material.NETHER_WART.".to_string(),
            }],
        }
    }

    fn test_rag_hit(
        source: &str,
        chunk_id: &str,
        score: usize,
        query: &str,
        preview: &str,
    ) -> RagContextHit {
        let query_scores = vec![RagQueryScore {
            query: query.to_string(),
            score,
        }];
        RagContextHit {
            source: source.to_string(),
            chunk_id: chunk_id.to_string(),
            score,
            weighted_score: weighted_query_score(&query_scores),
            matched_queries: vec![query.to_string()],
            query_scores,
            preview: preview.to_string(),
        }
    }

    #[test]
    fn prompt_contains_legacy_guardrails() {
        let profile = test_profile();
        let memory = test_memory();
        let prompt = build_prompt(
            "test",
            "context",
            Some(&profile),
            &memory,
            &RagContext::default(),
            false,
        );

        assert!(prompt.contains("Java 8"));
        assert!(prompt.contains("PandaSpigot"));
        assert!(prompt.contains("Material.SULPHUR"));
        assert!(prompt.contains("api-version"));
        assert!(prompt.contains("Regle profil"));
        assert!(prompt.contains("Memoire: ne jamais proposer Material.NETHER_WART"));
    }

    #[test]
    fn plan_prompt_forbids_file_modification_claims() {
        let profile = test_profile();
        let memory = test_memory();
        let prompt = build_plan_prompt(
            "ajouter /coins",
            "context",
            Some(&profile),
            &memory,
            &RagContext::default(),
            false,
        );

        assert!(prompt.contains("mode plan"));
        assert!(prompt.contains("Tu ne dois pas ecrire un patch complet"));
        assert!(prompt.contains("ne pas inclure de bloc de code"));
        assert!(prompt.contains("Verifications a lancer"));
        assert!(prompt.contains("Bukkit"));
    }

    #[test]
    fn brief_plan_prompt_limits_shape() {
        let prompt = build_plan_prompt(
            "verifier plugin",
            "context",
            None,
            &MemoryContext::default(),
            &RagContext::default(),
            true,
        );

        assert!(prompt.contains("6 puces maximum"));
        assert!(!prompt.contains("1. Resume de l'objectif"));
    }

    #[test]
    fn prompt_includes_rag_context() {
        let rag = RagContext {
            index: Some(PathBuf::from("data/index")),
            queries: vec!["nether wart".to_string(), "nether_stalk".to_string()],
            hits: vec![test_rag_hit(
                "resource-pack:assets/minecraft/lang/en_US.lang",
                "lang:0",
                12,
                "nether wart",
                "tile.netherStalk.name=Nether Wart",
            )],
        };
        let prompt = build_prompt(
            "nether wart",
            "context",
            None,
            &MemoryContext::default(),
            &rag,
            false,
        );

        assert!(prompt.contains("Connaissances RAG"));
        assert!(prompt.contains("tile.netherStalk.name=Nether Wart"));
    }

    #[test]
    fn prompt_sections_have_a_stable_cache_friendly_order() {
        let profile = test_profile();
        let memory = test_memory();
        let prompt = build_prompt(
            "CURRENT_REQUEST_MARKER",
            "DYNAMIC_PROJECT_MARKER",
            Some(&profile),
            &memory,
            &RagContext::default(),
            false,
        );
        let markers = [
            "[SYSTEM",
            "[POLICY",
            "[TOOLS",
            "[PROFILE]",
            "[HISTORY]",
            "[REQUEST]",
            "[DYNAMIC_CONTEXT]",
        ];
        let positions = markers
            .iter()
            .map(|marker| prompt.find(marker).unwrap())
            .collect::<Vec<_>>();

        assert!(positions.windows(2).all(|window| window[0] < window[1]));
        assert!(
            prompt.find("CURRENT_REQUEST_MARKER").unwrap()
                < prompt.find("DYNAMIC_PROJECT_MARKER").unwrap()
        );
        assert!(!prompt.contains("skills/profiles/"));
        assert!(!prompt.contains("skills/memory/"));
    }

    #[test]
    fn rag_context_is_bounded() {
        let rag = RagContext {
            index: Some(PathBuf::from("data/index")),
            queries: vec!["spade".to_string()],
            hits: vec![test_rag_hit(
                "plugin:big.java",
                "big:0",
                1,
                "spade",
                &"a".repeat(6_000),
            )],
        };
        let formatted = format_rag_context(&rag);

        assert!(formatted.contains("[truncated]"));
        assert!(formatted.chars().count() < 4_800);
    }

    #[test]
    fn expands_legacy_rag_queries() {
        let queries = expand_rag_queries("Verifier les pelles, spawners et nether wart");

        assert!(queries.contains(&"spade".to_string()));
        assert!(queries.contains(&"mob_spawner".to_string()));
        assert!(queries.contains(&"nether_stalk".to_string()));
    }

    #[test]
    fn expands_spawn_egg_rag_queries() {
        let queries = expand_rag_queries("Verifier les spawn eggs en Bukkit 1.8.8");

        assert!(queries.contains(&"spawn_egg".to_string()));
        assert!(queries.contains(&"monster_placer".to_string()));
        assert!(queries.contains(&"monsterPlacer".to_string()));
        assert!(queries.contains(&"MONSTER_EGG".to_string()));
        assert!(queries.contains(&"Material.MONSTER_EGG".to_string()));
    }

    #[test]
    fn displays_rag_debug_context() {
        let rag = RagContext {
            index: Some(PathBuf::from("data/index")),
            queries: vec!["spade".to_string()],
            hits: vec![test_rag_hit(
                "opticcode:docs/minecraft-legacy-rules.md",
                "rules:0",
                6,
                "spade",
                "WOOD_SPADE",
            )],
        };
        let display = rag.to_display_string();

        assert!(display.contains("Expanded queries"));
        assert!(display.contains("matched_queries: spade"));
        assert!(display.contains("weighted_score: 12"));
        assert!(display.contains("query_scores: spade=6x2"));
        assert!(display.contains("WOOD_SPADE"));
    }

    #[test]
    fn weights_precise_legacy_queries_above_generic_synonyms() {
        assert!(rag_query_weight("MOB_SPAWNER") > rag_query_weight("shovel"));
        assert!(rag_query_weight("NETHER_STALK") > rag_query_weight("shovel"));
        assert!(rag_query_weight("Material.SULPHUR") > rag_query_weight("shovel"));
        assert!(rag_query_weight("Material.MONSTER_EGG") > rag_query_weight("MONSTER_EGG"));
    }

    #[test]
    fn weighted_query_score_combines_raw_score_and_query_weight() {
        let scores = vec![
            RagQueryScore {
                query: "shovel".to_string(),
                score: 10,
            },
            RagQueryScore {
                query: "NETHER_STALK".to_string(),
                score: 2,
            },
        ];

        assert_eq!(weighted_query_score(&scores), 18);
    }

    #[test]
    fn prioritizes_docs_and_skills_over_internal_code_for_rag() {
        let mut hits = vec![
            test_rag_hit(
                "opticcode:crates/opticcode-tools/src/lib.rs",
                "crates:0",
                20,
                "spade",
                "internal implementation",
            ),
            test_rag_hit(
                "opticcode:skills/profiles/minecraft-java-1.8/profile.md",
                "skills:0",
                6,
                "spade",
                "profile rule",
            ),
            test_rag_hit(
                "opticcode:docs/minecraft-legacy-rules.md",
                "docs:0",
                5,
                "spade",
                "documented rule",
            ),
        ];

        sort_rag_hits_for_prompt(&mut hits);

        assert_eq!(hits[0].source, "opticcode:docs/minecraft-legacy-rules.md");
        assert_eq!(
            hits[1].source,
            "opticcode:skills/profiles/minecraft-java-1.8/profile.md"
        );
        assert_eq!(
            hits[2].source,
            "opticcode:crates/opticcode-tools/src/lib.rs"
        );
    }

    #[test]
    fn prioritizes_legacy_rules_over_other_docs() {
        let mut hits = vec![
            test_rag_hit(
                "opticcode:docs/mini-bukkit-benchmark.md",
                "bench:0",
                20,
                "SULPHUR",
                "Material.SULPHUR benchmark",
            ),
            test_rag_hit(
                "opticcode:docs/minecraft-legacy-rules.md",
                "rules:0",
                1,
                "SULPHUR",
                "Material.SULPHUR rule",
            ),
        ];

        sort_rag_hits_for_prompt(&mut hits);

        assert_eq!(hits[0].source, "opticcode:docs/minecraft-legacy-rules.md");
    }

    #[test]
    fn deduplicates_repeated_opticcode_legacy_rules() {
        let mut hits = vec![
            test_rag_hit(
                "opticcode:docs/minecraft-legacy-rules.md",
                "rules:0",
                10,
                "SULPHUR",
                "Material.GUNPOWDER -> Material.SULPHUR",
            ),
            test_rag_hit(
                "opticcode:crates/opticcode-tools/src/lib.rs",
                "crates:0",
                30,
                "SULPHUR",
                "Material.GUNPOWDER legacy Material.SULPHUR",
            ),
            test_rag_hit(
                "plugin:src/main/java/JoinListener.java",
                "plugin:0",
                5,
                "SULPHUR",
                "new ItemStack(Material.SULPHUR)",
            ),
        ];

        select_rag_hits_for_prompt(&mut hits);

        assert_eq!(hits.len(), 2);
        assert!(hits
            .iter()
            .any(|hit| hit.source == "opticcode:docs/minecraft-legacy-rules.md"));
        assert!(hits
            .iter()
            .any(|hit| hit.source == "plugin:src/main/java/JoinListener.java"));
        assert!(!hits
            .iter()
            .any(|hit| hit.source == "opticcode:crates/opticcode-tools/src/lib.rs"));
    }

    #[test]
    fn detects_duplicate_legacy_keys() {
        let hit = test_rag_hit(
            "opticcode:docs/minecraft-legacy-rules.md",
            "rules:0",
            1,
            "spade",
            "Material.WOODEN_SHOVEL -> Material.WOOD_SPADE",
        );

        assert_eq!(rag_duplicate_key(&hit), Some("opticcode:spade".to_string()));
    }

    #[test]
    fn filters_low_value_hits_without_legacy_concepts() {
        let mut hits = vec![
            test_rag_hit(
                "plugin:docs/config.yml",
                "plugin:0",
                1,
                "DIAMOND_SPADE",
                "DIAMOND_HELMET DIAMOND_CHESTPLATE",
            ),
            test_rag_hit(
                "resource-pack:assets/minecraft/lang/en_US.lang",
                "lang:0",
                1,
                "nether wart",
                "tile.netherStalk.name=Nether Wart",
            ),
        ];

        select_rag_hits_for_prompt(&mut hits);

        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].source,
            "resource-pack:assets/minecraft/lang/en_US.lang"
        );
    }
}
