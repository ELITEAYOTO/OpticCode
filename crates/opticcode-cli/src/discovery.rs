use std::collections::BTreeMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

use opticcode_core::{
    load_profile_for_workspace, ASSISTANT_CONTEXT_SCHEMA_VERSION, ASSISTANT_PROTOCOL_ID,
    ASSISTANT_PROTOCOL_SCHEMA_VERSION, ASSISTANT_RUN_SCHEMA_VERSION, CHAT_PROTOCOL_ID,
    CHAT_PROTOCOL_SCHEMA_VERSION,
};
use opticcode_llm::{
    HealthRequest, LlmProvider, OllamaProvider, ProviderCapabilities, LLM_PROTOCOL_ID,
    LLM_PROTOCOL_SCHEMA_VERSION,
};
use opticcode_policy::{PolicyEngine, POLICY_PROTOCOL_ID, POLICY_SCHEMA_VERSION, POLICY_VERSION};
use opticcode_tools::apply_transaction::APPLY_TRANSACTION_SCHEMA_VERSION;
use opticcode_tools::eval::EVAL_SCHEMA_VERSION;
use opticcode_tools::git_state::capture_git_state;
use opticcode_tools::java_context::JAVA_CONTEXT_SCHEMA_VERSION;
use opticcode_tools::java_edit_worktree::JAVA_EDIT_WORKTREE_SCHEMA_VERSION;
use opticcode_tools::java_edits::JAVA_EDIT_SCHEMA_VERSION;
use opticcode_tools::java_index::JAVA_INDEX_SCHEMA_VERSION;
use opticcode_tools::java_syntax::JAVA_SYNTAX_SCHEMA_VERSION;
use opticcode_tools::load_active_rag_manifest;
use opticcode_tools::process_runner::{
    run_process, ProcessLaunchMode, ProcessRequest, ProcessStatus,
};
use opticcode_tools::rag::RAG_INDEX_SCHEMA_VERSION;
use opticcode_tools::worktree::{
    inspect_disposable_worktrees_read_only, WORKTREE_LEASE_SCHEMA_VERSION,
    WORKTREE_VERIFICATION_SCHEMA_VERSION,
};
use serde::Serialize;

pub const DISCOVERY_PROTOCOL_ID: &str = "opticcode.discovery";
pub const DISCOVERY_SCHEMA_VERSION: u32 = 1;
const DOCTOR_OUTPUT_LIMIT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct ProtocolVersion {
    pub id: &'static str,
    pub schema_version: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlatformReport {
    pub os: &'static str,
    pub architecture: &'static str,
    pub target: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildReport {
    pub kind: &'static str,
    pub profile: &'static str,
    pub commit: Option<&'static str>,
    pub commit_short: Option<&'static str>,
    pub dirty: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VersionReport {
    pub schema_version: u32,
    pub protocol: &'static str,
    pub opticcode_version: &'static str,
    pub protocols: BTreeMap<&'static str, ProtocolVersion>,
    pub schemas: BTreeMap<&'static str, u32>,
    pub platform: PlatformReport,
    pub build: BuildReport,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderDescriptor {
    pub id: &'static str,
    pub active: bool,
    pub capabilities: ProviderCapabilities,
}

#[derive(Debug, Clone, Serialize)]
pub struct MachineOutputCapabilities {
    pub json: bool,
    pub ndjson: bool,
    pub streaming: bool,
    pub cancellation: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FeatureCapabilities {
    pub chat: bool,
    pub policy: bool,
    pub rag: bool,
    pub java: bool,
    pub worktrees: bool,
    pub verified_edits: bool,
    pub evaluation: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PolicyCapabilities {
    pub schema_version: u32,
    pub policy_version: &'static str,
    pub engine: bool,
    pub modes: Vec<&'static str>,
    pub audit: bool,
    pub approvals: bool,
    pub cli: bool,
    pub chat_read_only: bool,
    pub chat_write: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CapabilitiesReport {
    pub schema_version: u32,
    pub protocol: &'static str,
    pub commands: Vec<&'static str>,
    pub providers: Vec<ProviderDescriptor>,
    pub context_modes: Vec<&'static str>,
    pub machine_output: MachineOutputCapabilities,
    pub features: FeatureCapabilities,
    pub policy_runtime: PolicyCapabilities,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DoctorStatus {
    Ok,
    Warning,
    Error,
    Unavailable,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorCheck {
    pub id: &'static str,
    pub status: DoctorStatus,
    pub required: bool,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub schema_version: u32,
    pub protocol: &'static str,
    pub success: bool,
    pub workspace: PathBuf,
    pub profile: String,
    pub model: String,
    pub provider: &'static str,
    pub checks: Vec<DoctorCheck>,
}

#[derive(Debug, Clone)]
pub struct DoctorOptions {
    pub workspace: PathBuf,
    pub profile: String,
    pub model: String,
    pub ollama_url: String,
    pub rag_index: PathBuf,
    pub timeout: Duration,
}

pub fn version_report() -> VersionReport {
    let mut protocols = BTreeMap::new();
    protocols.insert(
        "assistant",
        ProtocolVersion {
            id: ASSISTANT_PROTOCOL_ID,
            schema_version: ASSISTANT_PROTOCOL_SCHEMA_VERSION,
        },
    );
    protocols.insert(
        "chat",
        ProtocolVersion {
            id: CHAT_PROTOCOL_ID,
            schema_version: CHAT_PROTOCOL_SCHEMA_VERSION,
        },
    );
    protocols.insert(
        "discovery",
        ProtocolVersion {
            id: DISCOVERY_PROTOCOL_ID,
            schema_version: DISCOVERY_SCHEMA_VERSION,
        },
    );
    protocols.insert(
        "policy",
        ProtocolVersion {
            id: POLICY_PROTOCOL_ID,
            schema_version: POLICY_SCHEMA_VERSION,
        },
    );
    protocols.insert(
        "llm",
        ProtocolVersion {
            id: LLM_PROTOCOL_ID,
            schema_version: LLM_PROTOCOL_SCHEMA_VERSION,
        },
    );

    VersionReport {
        schema_version: DISCOVERY_SCHEMA_VERSION,
        protocol: DISCOVERY_PROTOCOL_ID,
        opticcode_version: env!("CARGO_PKG_VERSION"),
        protocols,
        schemas: BTreeMap::from([
            ("apply_transaction", APPLY_TRANSACTION_SCHEMA_VERSION),
            ("assistant_context", ASSISTANT_CONTEXT_SCHEMA_VERSION),
            ("assistant_run", ASSISTANT_RUN_SCHEMA_VERSION),
            ("capabilities", DISCOVERY_SCHEMA_VERSION),
            ("chat", CHAT_PROTOCOL_SCHEMA_VERSION),
            ("doctor", DISCOVERY_SCHEMA_VERSION),
            ("evaluation", EVAL_SCHEMA_VERSION),
            ("java_context", JAVA_CONTEXT_SCHEMA_VERSION),
            ("java_edit_worktree", JAVA_EDIT_WORKTREE_SCHEMA_VERSION),
            ("java_edits", JAVA_EDIT_SCHEMA_VERSION),
            ("java_index", JAVA_INDEX_SCHEMA_VERSION),
            ("java_syntax", JAVA_SYNTAX_SCHEMA_VERSION),
            ("policy", POLICY_SCHEMA_VERSION),
            ("rag_index", RAG_INDEX_SCHEMA_VERSION),
            ("version", DISCOVERY_SCHEMA_VERSION),
            ("worktree_lease", WORKTREE_LEASE_SCHEMA_VERSION),
            (
                "worktree_verification",
                WORKTREE_VERIFICATION_SCHEMA_VERSION,
            ),
        ]),
        platform: PlatformReport {
            os: env::consts::OS,
            architecture: env::consts::ARCH,
            target: option_env!("OPTICCODE_BUILD_TARGET").unwrap_or("unknown"),
        },
        build: BuildReport {
            kind: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            profile: option_env!("OPTICCODE_BUILD_PROFILE").unwrap_or("unknown"),
            commit: option_env!("OPTICCODE_GIT_COMMIT"),
            commit_short: option_env!("OPTICCODE_GIT_COMMIT_SHORT"),
            dirty: match option_env!("OPTICCODE_GIT_DIRTY") {
                Some("true") => Some(true),
                Some("false") => Some(false),
                _ => None,
            },
        },
    }
}

pub fn capabilities_report() -> CapabilitiesReport {
    let provider = OllamaProvider::new("http://localhost:11434");
    CapabilitiesReport {
        schema_version: DISCOVERY_SCHEMA_VERSION,
        protocol: DISCOVERY_PROTOCOL_ID,
        commands: vec![
            "analyze-java",
            "apply",
            "ask",
            "build",
            "capabilities",
            "chat",
            "context",
            "doctor",
            "eval",
            "git-state",
            "inspect",
            "java-context",
            "java-edits",
            "java-edits-verify",
            "java-index",
            "java-legacy-rules",
            "java-syntax",
            "memory",
            "pack-scan",
            "patch",
            "plan",
            "policy",
            "profile",
            "rag-debug",
            "rag-index",
            "rag-scan",
            "rag-search",
            "search",
            "transactions",
            "version",
            "worktree-verify",
            "worktrees",
        ],
        providers: vec![ProviderDescriptor {
            id: provider.id().as_str(),
            active: true,
            capabilities: provider.capabilities(),
        }],
        context_modes: vec!["legacy", "symbol", "compare"],
        machine_output: MachineOutputCapabilities {
            json: true,
            ndjson: true,
            streaming: true,
            cancellation: true,
        },
        features: FeatureCapabilities {
            chat: true,
            policy: true,
            rag: true,
            java: true,
            worktrees: true,
            verified_edits: true,
            evaluation: true,
        },
        policy_runtime: PolicyCapabilities {
            schema_version: POLICY_SCHEMA_VERSION,
            policy_version: POLICY_VERSION,
            engine: true,
            modes: vec!["read_only", "worktree_edit", "approved_apply"],
            audit: true,
            approvals: true,
            cli: true,
            chat_read_only: true,
            chat_write: true,
        },
    }
}

pub async fn doctor_report(options: DoctorOptions) -> DoctorReport {
    let workspace = absolute_path(&options.workspace);
    let rag_index = absolute_path(&options.rag_index);
    let mut checks = Vec::new();

    checks.push(match env::current_exe() {
        Ok(path) if path.is_file() => DoctorCheck {
            id: "opticcode_executable",
            status: DoctorStatus::Ok,
            required: true,
            summary: "OpticCode executable is available".to_string(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
            path: Some(path),
        },
        Ok(path) => failed_check(
            "opticcode_executable",
            true,
            format!("current executable is not a file: {}", path.display()),
        ),
        Err(error) => failed_check(
            "opticcode_executable",
            true,
            format!("cannot resolve current executable: {error}"),
        ),
    });

    checks.push(match PolicyEngine::default_engine() {
        Ok(engine) => DoctorCheck {
            id: "policy_engine",
            status: DoctorStatus::Ok,
            required: true,
            summary: "Deny-by-default PolicyEngine state is available".to_string(),
            version: Some(POLICY_VERSION.to_string()),
            path: Some(engine.audit_store().state_root().to_path_buf()),
        },
        Err(error) => failed_check(
            "policy_engine",
            true,
            format!("PolicyEngine state is unavailable: {error}"),
        ),
    });

    checks.push(command_check(
        "git",
        true,
        &["--version"],
        &workspace,
        options.timeout,
    ));
    checks.push(command_check(
        "java",
        true,
        &["-version"],
        &workspace,
        options.timeout,
    ));
    checks.push(command_check(
        "maven",
        false,
        &["--version"],
        &workspace,
        options.timeout,
    ));
    checks.push(command_check(
        "gradle",
        false,
        &["--version"],
        &workspace,
        options.timeout,
    ));
    checks.push(command_check(
        "ollama_cli",
        false,
        &["--version"],
        &workspace,
        options.timeout,
    ));

    let (provider_check, model_check) = provider_checks(&options).await;
    checks.push(provider_check);
    checks.push(model_check);

    checks.push(match load_active_rag_manifest(&rag_index) {
        Ok(manifest) => DoctorCheck {
            id: "rag_index",
            status: DoctorStatus::Ok,
            required: false,
            summary: format!(
                "validated RAG generation {} ({} documents, {} chunks)",
                manifest.generation_id, manifest.documents, manifest.chunks
            ),
            version: Some(manifest.schema_version.to_string()),
            path: Some(rag_index),
        },
        Err(error) => warning_check("rag_index", format!("RAG index unavailable: {error:#}")),
    });

    checks.push(
        match load_profile_for_workspace(&workspace, Some(&options.profile)) {
            Ok(Some(profile)) => DoctorCheck {
                id: "profile",
                status: DoctorStatus::Ok,
                required: true,
                summary: format!("profile `{}` is readable", profile.id),
                version: None,
                path: Some(profile.source),
            },
            Ok(None) => warning_check("profile", "profile is disabled".to_string()),
            Err(error) => failed_check("profile", true, format!("profile unavailable: {error:#}")),
        },
    );

    checks.push(match capture_git_state(&workspace) {
        Ok(snapshot) => DoctorCheck {
            id: "workspace_git",
            status: DoctorStatus::Ok,
            required: true,
            summary: format!(
                "Git workspace inspected ({} change(s))",
                snapshot.changes.len()
            ),
            version: Some(snapshot.schema_version.to_string()),
            path: Some(snapshot.root),
        },
        Err(error) => failed_check(
            "workspace_git",
            true,
            format!("workspace Git state unavailable: {error:#}"),
        ),
    });

    checks.push(match (
        inspect_git_worktrees(&workspace, options.timeout),
        inspect_disposable_worktrees_read_only(),
    ) {
        (Ok(worktree_count), Ok(leases)) => {
            let invalid = leases.iter().filter(|lease| !lease.valid).count();
            DoctorCheck {
                id: "worktrees_and_leases",
                status: if invalid == 0 {
                    DoctorStatus::Ok
                } else {
                    DoctorStatus::Warning
                },
                required: false,
                summary: format!(
                    "{worktree_count} Git worktree(s), {} OpticCode lease(s), {} requiring inspection",
                    leases.len(),
                    invalid
                ),
                version: Some(WORKTREE_LEASE_SCHEMA_VERSION.to_string()),
                path: None,
            }
        }
        (Err(error), _) => warning_check(
            "worktrees_and_leases",
            format!("Git worktrees could not be inspected: {error}"),
        ),
        (_, Err(error)) => warning_check(
            "worktrees_and_leases",
            format!("worktree storage could not be inspected: {error:#}"),
        ),
    });

    let success = checks
        .iter()
        .all(|check| !check.required || check.status == DoctorStatus::Ok);
    DoctorReport {
        schema_version: DISCOVERY_SCHEMA_VERSION,
        protocol: DISCOVERY_PROTOCOL_ID,
        success,
        workspace,
        profile: options.profile,
        model: options.model,
        provider: "ollama",
        checks,
    }
}

fn inspect_git_worktrees(working_directory: &Path, timeout: Duration) -> Result<usize, String> {
    let Some((program, launch_mode)) = resolve_program("git") else {
        return Err("`git` was not found on PATH".to_string());
    };
    let mut request = ProcessRequest::new(program, working_directory);
    request.args = ["worktree", "list", "--porcelain"]
        .into_iter()
        .map(OsString::from)
        .collect();
    request.timeout = timeout;
    request.output_limit_bytes = DOCTOR_OUTPUT_LIMIT_BYTES;
    request.launch_mode = launch_mode;
    let result = run_process(&request).map_err(|error| format!("{error:#}"))?;
    if !result.success() || result.output.output_truncated {
        return Err(format!(
            "git worktree list ended with status {}",
            result.status.as_str()
        ));
    }
    Ok(result
        .stdout
        .lines()
        .filter(|line| line.starts_with("worktree "))
        .count())
}

async fn provider_checks(options: &DoctorOptions) -> (DoctorCheck, DoctorCheck) {
    let provider = match OllamaProvider::try_new(&options.ollama_url) {
        Ok(provider) => provider,
        Err(error) => {
            return (
                failed_check(
                    "ollama_provider",
                    true,
                    format!("invalid provider: {error:#}"),
                ),
                failed_check(
                    "configured_model",
                    true,
                    "model cannot be checked while provider configuration is invalid".to_string(),
                ),
            );
        }
    };
    let timeout_ms = options.timeout.as_millis().min(u128::from(u64::MAX)) as u64;
    match provider
        .health(HealthRequest {
            model: Some(options.model.clone()),
            timeout_ms,
            ..HealthRequest::default()
        })
        .await
    {
        Ok(health) => {
            let provider_check = DoctorCheck {
                id: "ollama_provider",
                status: if health.reachable {
                    DoctorStatus::Ok
                } else {
                    DoctorStatus::Error
                },
                required: true,
                summary: format!(
                    "Ollama provider is reachable ({} model(s), {} ms)",
                    health.model_count, health.latency_ms
                ),
                version: None,
                path: None,
            };
            let model_available = health.model_available == Some(true);
            let model_check = DoctorCheck {
                id: "configured_model",
                status: if model_available {
                    DoctorStatus::Ok
                } else {
                    DoctorStatus::Error
                },
                required: true,
                summary: if model_available {
                    format!("configured model `{}` is available", options.model)
                } else {
                    format!("configured model `{}` is unavailable", options.model)
                },
                version: None,
                path: None,
            };
            (provider_check, model_check)
        }
        Err(error) => (
            failed_check(
                "ollama_provider",
                true,
                format!("Ollama provider unavailable: {error}"),
            ),
            failed_check(
                "configured_model",
                true,
                "model could not be checked because Ollama is unavailable".to_string(),
            ),
        ),
    }
}

fn command_check(
    id: &'static str,
    required: bool,
    args: &[&str],
    working_directory: &Path,
    timeout: Duration,
) -> DoctorCheck {
    let executable_name = match id {
        "maven" => "mvn",
        "ollama_cli" => "ollama",
        other => other,
    };
    let Some((program, launch_mode)) = resolve_program(executable_name) else {
        return DoctorCheck {
            id,
            status: DoctorStatus::Unavailable,
            required,
            summary: format!("`{executable_name}` was not found on PATH"),
            version: None,
            path: None,
        };
    };
    let mut request = ProcessRequest::new(&program, working_directory);
    request.args = args.iter().map(OsString::from).collect();
    request.timeout = timeout;
    request.output_limit_bytes = DOCTOR_OUTPUT_LIMIT_BYTES;
    request.launch_mode = launch_mode;
    match run_process(&request) {
        Ok(result) if result.status == ProcessStatus::Success => DoctorCheck {
            id,
            status: DoctorStatus::Ok,
            required,
            summary: format!("`{executable_name}` is available"),
            version: first_output_line(&result.stdout, &result.stderr),
            path: Some(program),
        },
        Ok(result) => failed_check(
            id,
            required,
            format!(
                "`{executable_name}` check ended with status {}",
                result.status.as_str()
            ),
        ),
        Err(error) => failed_check(
            id,
            required,
            format!("`{executable_name}` could not be started: {error:#}"),
        ),
    }
}

fn resolve_program(name: &str) -> Option<(PathBuf, ProcessLaunchMode)> {
    let path = Path::new(name);
    if path.components().count() > 1 {
        return executable_candidate(path.to_path_buf());
    }
    let search_path = env::var_os("PATH")?;
    let extensions = executable_extensions(path.extension().is_some());
    for directory in env::split_paths(&search_path) {
        for extension in &extensions {
            let candidate = if extension.is_empty() {
                directory.join(name)
            } else {
                directory.join(format!("{name}{extension}"))
            };
            if let Some(found) = executable_candidate(candidate) {
                return Some(found);
            }
        }
    }
    None
}

fn executable_extensions(already_has_extension: bool) -> Vec<String> {
    if already_has_extension {
        return vec![String::new()];
    }
    #[cfg(windows)]
    {
        let extensions = env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
        extensions
            .split(';')
            .filter(|extension| !extension.is_empty())
            .map(|extension| extension.to_ascii_lowercase())
            .collect()
    }
    #[cfg(not(windows))]
    vec![String::new()]
}

fn executable_candidate(path: PathBuf) -> Option<(PathBuf, ProcessLaunchMode)> {
    if !path.is_file() {
        return None;
    }
    let extension = path.extension().and_then(OsStr::to_str).unwrap_or_default();
    let launch_mode = if cfg!(windows)
        && (extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat"))
    {
        ProcessLaunchMode::WindowsCommandScript
    } else {
        ProcessLaunchMode::Direct
    };
    Some((path, launch_mode))
}

fn first_output_line(stdout: &str, stderr: &str) -> Option<String> {
    stdout
        .lines()
        .chain(stderr.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(512).collect())
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn failed_check(id: &'static str, required: bool, summary: String) -> DoctorCheck {
    DoctorCheck {
        id,
        status: DoctorStatus::Error,
        required,
        summary,
        version: None,
        path: None,
    }
}

fn warning_check(id: &'static str, summary: String) -> DoctorCheck {
    DoctorCheck {
        id,
        status: DoctorStatus::Warning,
        required: false,
        summary,
        version: None,
        path: None,
    }
}

pub fn render_version(report: &VersionReport) -> String {
    let commit = report
        .build
        .commit_short
        .or(report.build.commit)
        .unwrap_or("unknown");
    let state = match report.build.dirty {
        Some(true) => "dirty",
        Some(false) => "clean",
        None => "unknown",
    };

    format!(
        "OpticCode {}\nassistant protocol: {} v{}\nLLM protocol: {} v{}\nplatform: {}/{} target={}\nbuild: {} profile={} commit={} state={}",
        report.opticcode_version,
        report.protocols["assistant"].id,
        report.protocols["assistant"].schema_version,
        report.protocols["llm"].id,
        report.protocols["llm"].schema_version,
        report.platform.os,
        report.platform.architecture,
        report.platform.target,
        report.build.kind,
        report.build.profile,
        commit,
        state
    )
}

pub fn render_capabilities(report: &CapabilitiesReport) -> String {
    format!(
        "Commands: {}\nProviders: {}\nContext modes: {}\nJSON: yes; NDJSON streaming: yes",
        report.commands.len(),
        report
            .providers
            .iter()
            .map(|provider| provider.id)
            .collect::<Vec<_>>()
            .join(", "),
        report.context_modes.join(", ")
    )
}

pub fn render_doctor(report: &DoctorReport) -> String {
    let mut output = format!(
        "OpticCode doctor: {}\n",
        if report.success { "ready" } else { "not ready" }
    );
    for check in &report.checks {
        output.push_str(&format!(
            "- {} [{:?}]: {}\n",
            check.id, check.status, check.summary
        ));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{capabilities_report, version_report, DISCOVERY_PROTOCOL_ID};
    use opticcode_core::{ASSISTANT_PROTOCOL_ID, ASSISTANT_PROTOCOL_SCHEMA_VERSION};
    use opticcode_llm::{LLM_PROTOCOL_ID, LLM_PROTOCOL_SCHEMA_VERSION};

    #[test]
    fn version_report_exposes_all_machine_protocols() {
        let report = version_report();
        assert_eq!(report.protocol, DISCOVERY_PROTOCOL_ID);
        assert_eq!(report.protocols["assistant"].id, ASSISTANT_PROTOCOL_ID);
        assert_eq!(
            report.protocols["assistant"].schema_version,
            ASSISTANT_PROTOCOL_SCHEMA_VERSION
        );
        assert_eq!(report.protocols["llm"].id, LLM_PROTOCOL_ID);
        assert_eq!(
            report.protocols["llm"].schema_version,
            LLM_PROTOCOL_SCHEMA_VERSION
        );
        assert_eq!(report.schemas["java_syntax"], 2);
        assert_eq!(report.schemas["rag_index"], 2);
    }

    #[test]
    fn capabilities_keep_legacy_default_available_without_hiding_symbol_modes() {
        let report = capabilities_report();
        assert_eq!(report.context_modes, ["legacy", "symbol", "compare"]);
        assert!(report.commands.contains(&"ask"));
        assert!(report.commands.contains(&"doctor"));
        assert!(report.machine_output.ndjson);
        assert!(report.providers[0].capabilities.streaming);
        assert!(report.features.verified_edits);
    }
}
