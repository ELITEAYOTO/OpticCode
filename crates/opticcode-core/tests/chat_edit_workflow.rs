use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use async_trait::async_trait;
use opticcode_core::{
    chat_event_channel, execute_chat, ChatBudgets, ChatClientMetadata, ChatCommand,
    ChatEditControl, ChatExpectedProtocols, ChatGenerationOptions, ChatNativeConfirmation,
    ChatProtocolEvent, ChatProtocolEventPayload, ChatProtocolSession, ChatReference,
    ChatReferenceTarget, ChatRequest, ChatRuntimeOptions, ChatSecurityMode, ContextMode, OpticCode,
    DEFAULT_CHAT_EVENT_CAPACITY,
};
use opticcode_edit::{
    ByteRange, EditOperation, EditPlan, EditPlanLimits, EditValidationKind, LineEnding,
    TextEncoding,
};
use opticcode_llm::{
    CancellationToken, EventSink, FinishReason, GenerationRequest, GenerationResult,
    GenerationTimings, GenerationUsage, HealthReport, HealthRequest, HealthStatus, LlmProvider,
    ModelInfo, ProviderCapabilities, ProviderError, ProviderId, LLM_PROTOCOL_SCHEMA_VERSION,
};

const SOURCE_PATH: &str = "src/main/java/test/Plugin.java";
const SOURCE_BEFORE: &str = concat!(
    "package test;\n",
    "public final class Plugin {\n",
    "    public String message() { return \"before\"; }\n",
    "}\n"
);
const SOURCE_AFTER: &str = concat!(
    "package test;\n",
    "public final class Plugin {\n",
    "    public String message() { return \"after\"; }\n",
    "}\n"
);

#[derive(Debug)]
struct EditPlanProvider;

#[async_trait]
impl LlmProvider for EditPlanProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Ollama
    }

    fn endpoint(&self) -> &str {
        "mock://edit-plan"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            local_only: true,
            health: true,
            model_listing: true,
            generation: true,
            streaming: false,
            cancellation: true,
            token_usage: true,
            provider_timings: true,
            deterministic_seed: true,
        }
    }

    async fn health(&self, request: HealthRequest) -> Result<HealthReport, ProviderError> {
        Ok(HealthReport {
            schema_version: LLM_PROTOCOL_SCHEMA_VERSION,
            provider: ProviderId::Ollama,
            endpoint: self.endpoint().to_string(),
            status: HealthStatus::Healthy,
            reachable: true,
            latency_ms: 0,
            model_count: 1,
            requested_model: request.model,
            model_available: Some(true),
        })
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        Ok(Vec::new())
    }

    async fn generate(
        &self,
        request: GenerationRequest,
        _cancellation: CancellationToken,
    ) -> Result<GenerationResult, ProviderError> {
        assert_eq!(
            request.options.output_format,
            opticcode_llm::GenerationOutputFormat::Json
        );
        assert!(request.options.output_schema.is_some());
        let mut plan = EditPlan::new_empty();
        plan.plan_id = prompt_value(&request.prompt, "plan_id");
        plan.request_id = prompt_value(&request.prompt, "request_id");
        plan.workspace_id = prompt_value(&request.prompt, "workspace_id");
        plan.workspace_root_hash = prompt_value(&request.prompt, "workspace_root_hash");
        plan.profile = prompt_value(&request.prompt, "profile");
        plan.provider = ProviderId::Ollama;
        plan.model = prompt_value(&request.prompt, "model");
        plan.base_head = prompt_value(&request.prompt, "base_head");
        plan.working_tree_digest = prompt_value(&request.prompt, "working_tree_digest");
        plan.summary = "Replace the fixture message after isolated verification.".to_string();
        plan.rationale_summary = "A byte-exact literal replacement satisfies the task.".to_string();
        let start = SOURCE_BEFORE.find("before").unwrap();
        plan.operations = vec![EditOperation::Modify {
            path: SOURCE_PATH.to_string(),
            expected_file_hash: blake3::hash(SOURCE_BEFORE.as_bytes()).to_hex().to_string(),
            encoding: TextEncoding::Utf8,
            line_ending: LineEnding::Lf,
            range: ByteRange {
                start,
                end: start + "before".len(),
            },
            expected_old: "before".to_string(),
            replacement: "after".to_string(),
            reason: "Implement the explicit fixture request.".to_string(),
            symbol: Some("test.Plugin.message".to_string()),
            provenance: vec!["user_reference".to_string()],
        }];
        plan.validations = vec![
            EditValidationKind::ReparseJava,
            EditValidationKind::BuildOffline,
            EditValidationKind::TestOffline,
        ];
        plan.risks = vec!["Fixture-only behavioral text change.".to_string()];
        plan.limitations = Vec::new();
        plan.limits = EditPlanLimits::default();
        plan.expires_at_unix_ms = prompt_value(&request.prompt, "expires_at_unix_ms")
            .parse()
            .unwrap();
        let output = serde_json::to_string(&plan).unwrap();
        Ok(GenerationResult {
            schema_version: LLM_PROTOCOL_SCHEMA_VERSION,
            request_id: request.request_id,
            provider: ProviderId::Ollama,
            model: request.model,
            output,
            finish_reason: FinishReason::Stop,
            prompt_chars: request.prompt.len(),
            usage: GenerationUsage {
                prompt_tokens: Some(400),
                generated_tokens: Some(200),
            },
            timings: GenerationTimings {
                client_ms: 2,
                provider_total_ms: Some(2),
                load_ms: Some(0),
                prompt_eval_ms: Some(1),
                generation_ms: Some(1),
            },
        })
    }

    async fn stream(
        &self,
        request: GenerationRequest,
        _events: EventSink,
        cancellation: CancellationToken,
    ) -> Result<GenerationResult, ProviderError> {
        self.generate(request, cancellation).await
    }
}

#[tokio::test]
async fn chat_fix_apply_and_rollback_restores_the_original_fixture() {
    if !real_maven_integration_enabled() {
        eprintln!(
            "skipping real chat edit Maven workflow because explicit integration is disabled or mvn is unavailable"
        );
        return;
    }
    let fixture = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    initialize_fixture(fixture.path());
    let baseline = fs::read(fixture.path().join(SOURCE_PATH)).unwrap();
    let options = ChatRuntimeOptions {
        rag_index: Path::new("missing-index").to_path_buf(),
        verify_model: false,
        policy_state_root: Some(state.path().to_path_buf()),
        proposal_state_root: Some(state.path().to_path_buf()),
    };
    let app = OpticCode::with_provider(Arc::new(EditPlanProvider), "fixture-model").unwrap();

    let (fix, fix_events) = run_chat(
        Some(&app),
        request(
            fixture.path(),
            ChatCommand::Fix,
            "Replace before with after.",
        ),
        options.clone(),
    )
    .await;
    assert_eq!(fix.status, opticcode_core::ChatExecutionStatus::Completed);
    assert_eq!(
        fs::read(fixture.path().join(SOURCE_PATH)).unwrap(),
        baseline
    );
    let proposal_id = fix_events
        .iter()
        .find_map(|event| match &event.payload {
            ChatProtocolEventPayload::ProposalStored { proposal_id, .. } => {
                Some(proposal_id.clone())
            }
            _ => None,
        })
        .expect("fix must publish a proposal ID");
    let apply_approval = approval(&fix_events, "apply");
    assert!(fix_events
        .iter()
        .any(|event| matches!(event.payload, ChatProtocolEventPayload::DiffReady { .. })));
    assert!(fix_events.iter().any(|event| matches!(
        event.payload,
        ChatProtocolEventPayload::VerificationCompleted { success: true, .. }
    )));
    assert_eq!(worktree_count(fixture.path()), 1);

    let mut typed_apply = request(fixture.path(), ChatCommand::Apply, "yes apply it");
    typed_apply.edit = Some(ChatEditControl {
        proposal_id: Some(proposal_id.clone()),
        ..ChatEditControl::default()
    });
    let (typed_report, typed_events) = run_chat(None, typed_apply, options.clone()).await;
    assert_eq!(
        typed_report.status,
        opticcode_core::ChatExecutionStatus::Completed
    );
    assert_eq!(approval(&typed_events, "apply"), apply_approval);
    assert_eq!(
        fs::read(fixture.path().join(SOURCE_PATH)).unwrap(),
        baseline
    );

    let mut apply = request(fixture.path(), ChatCommand::Apply, "");
    apply.edit = Some(ChatEditControl {
        proposal_id: Some(proposal_id.clone()),
        native_confirmation: Some(ChatNativeConfirmation {
            client: "opticcode-vscode".to_string(),
            confirmation_id: "vscode-modal-apply-1".to_string(),
            approval_request_id: apply_approval,
        }),
        ..ChatEditControl::default()
    });
    let (apply_report, apply_events) = run_chat(None, apply, options.clone()).await;
    assert_eq!(
        apply_report.status,
        opticcode_core::ChatExecutionStatus::Completed
    );
    assert_eq!(
        fs::read_to_string(fixture.path().join(SOURCE_PATH)).unwrap(),
        SOURCE_AFTER
    );
    let transaction_id = apply_events
        .iter()
        .find_map(|event| match &event.payload {
            ChatProtocolEventPayload::ApplyCompleted {
                transaction_id,
                success: true,
                ..
            } => Some(transaction_id.clone()),
            _ => None,
        })
        .expect("apply must publish its committed transaction");

    let mut foreign_rollback = request(fixture.path(), ChatCommand::Rollback, "");
    foreign_rollback.edit = Some(ChatEditControl {
        proposal_id: Some(proposal_id.clone()),
        transaction_id: Some("apply-foreign-transaction".to_string()),
        ..ChatEditControl::default()
    });
    let (foreign_report, _) = run_chat(None, foreign_rollback, options.clone()).await;
    assert_eq!(
        foreign_report.status,
        opticcode_core::ChatExecutionStatus::Failed
    );
    assert_eq!(
        fs::read_to_string(fixture.path().join(SOURCE_PATH)).unwrap(),
        SOURCE_AFTER
    );

    let mut rollback_prompt = request(fixture.path(), ChatCommand::Rollback, "");
    rollback_prompt.edit = Some(ChatEditControl {
        proposal_id: Some(proposal_id.clone()),
        transaction_id: Some(transaction_id.clone()),
        ..ChatEditControl::default()
    });
    let (_, rollback_prompt_events) = run_chat(None, rollback_prompt, options.clone()).await;
    let rollback_approval = approval(&rollback_prompt_events, "rollback");
    assert_eq!(
        fs::read_to_string(fixture.path().join(SOURCE_PATH)).unwrap(),
        SOURCE_AFTER
    );

    let mut rollback = request(fixture.path(), ChatCommand::Rollback, "");
    rollback.edit = Some(ChatEditControl {
        proposal_id: Some(proposal_id),
        transaction_id: Some(transaction_id),
        native_confirmation: Some(ChatNativeConfirmation {
            client: "opticcode-vscode".to_string(),
            confirmation_id: "vscode-modal-rollback-1".to_string(),
            approval_request_id: rollback_approval,
        }),
        ..ChatEditControl::default()
    });
    let (rollback_report, rollback_events) = run_chat(None, rollback, options).await;
    assert_eq!(
        rollback_report.status,
        opticcode_core::ChatExecutionStatus::Completed
    );
    assert!(rollback_events.iter().any(|event| matches!(
        event.payload,
        ChatProtocolEventPayload::RollbackCompleted { success: true, .. }
    )));
    assert_eq!(
        fs::read(fixture.path().join(SOURCE_PATH)).unwrap(),
        baseline
    );
    assert_eq!(worktree_count(fixture.path()), 1);
}

async fn run_chat(
    app: Option<&OpticCode>,
    request: ChatRequest,
    options: ChatRuntimeOptions,
) -> (opticcode_core::ChatExecutionReport, Vec<ChatProtocolEvent>) {
    let (events, mut receiver) = chat_event_channel(DEFAULT_CHAT_EVENT_CAPACITY).unwrap();
    let session = ChatProtocolSession {
        request_id: request.request_id.clone(),
        events,
        cancellation: CancellationToken::new(),
    };
    let report = execute_chat(app, request, session, options).await.unwrap();
    let mut captured = Vec::new();
    while let Some(event) = receiver.recv().await {
        captured.push(event);
    }
    (report, captured)
}

fn request(root: &Path, command: ChatCommand, prompt: &str) -> ChatRequest {
    ChatRequest {
        schema_version: 1,
        protocol: "opticcode.chat".to_string(),
        request_id: format!("chat-edit-{}-{}", command.as_str(), random_suffix()),
        workspace_id: "workspace-chat-edit-fixture".to_string(),
        workspace_root: root.to_string_lossy().to_string(),
        command,
        prompt: prompt.to_string(),
        profile: "minecraft-java-1.8".to_string(),
        provider: ProviderId::Ollama,
        model: "fixture-model".to_string(),
        context_mode: ContextMode::Legacy,
        context_scope: opticcode_core::ChatContextScope::Automatic,
        scope_reason: opticcode_core::ChatScopeReason::DefaultSetting,
        evidence_mode: opticcode_core::ChatEvidenceMode::Optional,
        references: if command == ChatCommand::Fix {
            vec![ChatReference {
                reference_id: "fixture-source".to_string(),
                inclusion_reason: "explicit test source".to_string(),
                target: ChatReferenceTarget::File {
                    path: SOURCE_PATH.to_string(),
                },
            }]
        } else {
            Vec::new()
        },
        history: Vec::new(),
        budgets: ChatBudgets {
            rag_hits: 0,
            ..ChatBudgets::default()
        },
        generation: ChatGenerationOptions {
            max_output_tokens: 4_096,
            temperature: Some(0.0),
            seed: Some(1),
            brief: true,
            compare_generate: false,
        },
        security_mode: ChatSecurityMode::ReadOnly,
        client: ChatClientMetadata {
            name: "opticcode-vscode".to_string(),
            version: "0.2.0".to_string(),
            vscode_version: "1.125.0".to_string(),
            session_id: "session-chat-edit".to_string(),
            locale: "en".to_string(),
            recent_run_ids: Vec::new(),
            previous_repository_state: None,
        },
        expected_protocols: ChatExpectedProtocols::default(),
        edit: None,
    }
}

fn real_maven_integration_enabled() -> bool {
    std::env::var("OPTICCODE_RUN_REAL_INTEGRATION").as_deref() == Ok("1") && maven_available()
}

fn maven_available() -> bool {
    Command::new(if cfg!(windows) { "where.exe" } else { "which" })
        .arg(if cfg!(windows) { "mvn.cmd" } else { "mvn" })
        .output()
        .is_ok_and(|output| output.status.success())
}

fn initialize_fixture(root: &Path) {
    fs::create_dir_all(root.join("src/main/java/test")).unwrap();
    fs::write(root.join(SOURCE_PATH), SOURCE_BEFORE).unwrap();
    fs::write(
        root.join("pom.xml"),
        concat!(
            "<project xmlns=\"http://maven.apache.org/POM/4.0.0\">\n",
            "<modelVersion>4.0.0</modelVersion>\n",
            "<groupId>test</groupId><artifactId>chat-edit</artifactId><version>1</version>\n",
            "<properties><maven.compiler.source>1.8</maven.compiler.source>",
            "<maven.compiler.target>1.8</maven.compiler.target></properties>\n",
            "</project>\n"
        ),
    )
    .unwrap();
    fs::write(
        root.join(".gitignore"),
        "target/\n.gradle/\nbuild/\n.opticcode/\n",
    )
    .unwrap();
    git(root, &["init", "--quiet"]);
    git(root, &["add", "--all"]);
    git(
        root,
        &[
            "-c",
            "user.name=OpticCode Test",
            "-c",
            "user.email=opticcode@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ],
    );
}

fn approval(events: &[ChatProtocolEvent], operation: &str) -> String {
    events
        .iter()
        .find_map(|event| match &event.payload {
            ChatProtocolEventPayload::ApprovalRequired {
                approval_request_id,
                operation: candidate,
                ..
            } if candidate == operation => Some(approval_request_id.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing {operation} approval request: {events:#?}"))
}

fn prompt_value(prompt: &str, key: &str) -> String {
    prompt
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")))
        .unwrap_or_else(|| panic!("missing {key} in structured prompt"))
        .to_string()
}

fn worktree_count(root: &Path) -> usize {
    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .filter(|line| line.starts_with("worktree "))
        .count()
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn random_suffix() -> String {
    format!(
        "{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}
