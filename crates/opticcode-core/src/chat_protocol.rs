use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use opticcode_llm::{CancellationToken, ProviderId};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::{
    ChatContextScope, ChatEvidenceMode, ChatScopeReason, ComplianceReport, ContextManifest,
    ContextMode, EvidenceValidationReport, GroundedResponse, GroundingRoute,
};

pub const CHAT_PROTOCOL_ID: &str = "opticcode.chat";
pub const CHAT_PROTOCOL_SCHEMA_VERSION: u32 = 1;
pub const CHAT_CONTROL_PROTOCOL_ID: &str = "opticcode.chat.control";
pub const DEFAULT_CHAT_EVENT_CAPACITY: usize = 128;
pub const MAX_CHAT_REQUEST_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_CHAT_REQUEST_ID_BYTES: usize = 128;
pub const MAX_CHAT_PROMPT_CHARS: usize = 64 * 1024;
pub const MAX_CHAT_HISTORY_TURNS: usize = 32;
pub const MAX_CHAT_HISTORY_CHARS: usize = 128 * 1024;
pub const MAX_CHAT_HISTORY_TOKENS: usize = 32 * 1024;
pub const MAX_CHAT_REFERENCES: usize = 64;
pub const MAX_CHAT_REFERENCE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_CHAT_OUTPUT_TOKENS: u32 = 4_096;
pub const MAX_CHAT_EVENT_TEXT_BYTES: usize = 256 * 1024;
const MAX_CHAT_EVENT_CAPACITY: usize = 4_096;
const EVENT_DELIVERY_TIMEOUT: Duration = Duration::from_secs(5);

pub type ChatEventSink = mpsc::Sender<ChatProtocolEvent>;
pub type ChatEventReceiver = mpsc::Receiver<ChatProtocolEvent>;

#[derive(Debug, Clone)]
pub struct ChatProtocolSession {
    pub request_id: String,
    pub events: ChatEventSink,
    pub cancellation: CancellationToken,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ChatRequest {
    pub schema_version: u32,
    pub protocol: String,
    pub request_id: String,
    pub workspace_id: String,
    pub workspace_root: String,
    pub command: ChatCommand,
    pub prompt: String,
    pub profile: String,
    pub provider: ProviderId,
    pub model: String,
    pub context_mode: ContextMode,
    #[serde(default)]
    pub context_scope: ChatContextScope,
    #[serde(default)]
    pub scope_reason: ChatScopeReason,
    #[serde(default)]
    pub evidence_mode: ChatEvidenceMode,
    pub references: Vec<ChatReference>,
    pub history: Vec<ChatHistoryTurn>,
    pub budgets: ChatBudgets,
    pub generation: ChatGenerationOptions,
    pub security_mode: ChatSecurityMode,
    pub client: ChatClientMetadata,
    pub expected_protocols: ChatExpectedProtocols,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit: Option<ChatEditControl>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChatEditControl {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaction_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_confirmation: Option<ChatNativeConfirmation>,
    #[serde(default)]
    pub discard: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChatNativeConfirmation {
    pub client: String,
    pub confirmation_id: String,
    pub approval_request_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatCommand {
    Ask,
    Plan,
    Context,
    Analyze,
    Index,
    Legacy,
    Inspect,
    Fix,
    Verify,
    Diff,
    Apply,
    Rollback,
    Status,
    Runs,
    Help,
    #[serde(other)]
    Unknown,
}

impl ChatCommand {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Plan => "plan",
            Self::Context => "context",
            Self::Analyze => "analyze",
            Self::Index => "index",
            Self::Legacy => "legacy",
            Self::Inspect => "inspect",
            Self::Fix => "fix",
            Self::Verify => "verify",
            Self::Diff => "diff",
            Self::Apply => "apply",
            Self::Rollback => "rollback",
            Self::Status => "status",
            Self::Runs => "runs",
            Self::Help => "help",
            Self::Unknown => "unknown",
        }
    }

    pub const fn requires_prompt(self) -> bool {
        matches!(
            self,
            Self::Ask | Self::Plan | Self::Context | Self::Inspect | Self::Fix
        )
    }
}

impl std::fmt::Display for ChatCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatSecurityMode {
    #[default]
    ReadOnly,
    WorktreeEdit,
    ApprovedApply,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChatBudgets {
    pub max_history_turns: usize,
    pub max_history_chars: usize,
    pub max_history_tokens: usize,
    pub max_references: usize,
    pub max_reference_bytes: usize,
    pub max_prompt_tokens: usize,
    pub rag_hits: usize,
}

impl Default for ChatBudgets {
    fn default() -> Self {
        Self {
            max_history_turns: 12,
            max_history_chars: 32 * 1024,
            max_history_tokens: 8 * 1024,
            max_references: 24,
            max_reference_bytes: 1024 * 1024,
            max_prompt_tokens: 32 * 1024,
            rag_hits: 6,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ChatGenerationOptions {
    pub max_output_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    pub brief: bool,
    pub compare_generate: bool,
}

impl Default for ChatGenerationOptions {
    fn default() -> Self {
        Self {
            max_output_tokens: 1_024,
            temperature: None,
            seed: None,
            brief: false,
            compare_generate: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChatClientMetadata {
    pub name: String,
    pub version: String,
    pub vscode_version: String,
    pub session_id: String,
    pub locale: String,
    #[serde(default)]
    pub recent_run_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_repository_state: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChatExpectedProtocols {
    pub chat: u32,
    pub assistant: u32,
    pub discovery: u32,
    pub llm: u32,
}

impl Default for ChatExpectedProtocols {
    fn default() -> Self {
        Self {
            chat: CHAT_PROTOCOL_SCHEMA_VERSION,
            assistant: 1,
            discovery: 1,
            llm: 1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChatHistoryTurn {
    pub role: ChatHistoryRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<ChatCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_scope: Option<ChatContextScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grounding_status: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatHistoryRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChatReference {
    pub reference_id: String,
    pub inclusion_reason: String,
    #[serde(flatten)]
    pub target: ChatReferenceTarget,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ChatReferenceWire {
    File {
        reference_id: String,
        inclusion_reason: String,
        path: String,
    },
    Range {
        reference_id: String,
        inclusion_reason: String,
        path: String,
        range: ChatTextRange,
    },
    Selection {
        reference_id: String,
        inclusion_reason: String,
        path: String,
        range: ChatTextRange,
    },
    Symbol {
        reference_id: String,
        inclusion_reason: String,
        path: String,
        symbol: String,
        #[serde(default)]
        range: Option<ChatTextRange>,
    },
    ActiveFile {
        reference_id: String,
        inclusion_reason: String,
        path: String,
    },
    Finding {
        reference_id: String,
        inclusion_reason: String,
        finding_id: String,
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        range: Option<ChatTextRange>,
    },
    Run {
        reference_id: String,
        inclusion_reason: String,
        run_id: String,
    },
    Diff {
        reference_id: String,
        inclusion_reason: String,
        proposal_id: String,
    },
}

impl<'de> Deserialize<'de> for ChatReference {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = ChatReferenceWire::deserialize(deserializer)?;
        let (reference_id, inclusion_reason, target) = match wire {
            ChatReferenceWire::File {
                reference_id,
                inclusion_reason,
                path,
            } => (
                reference_id,
                inclusion_reason,
                ChatReferenceTarget::File { path },
            ),
            ChatReferenceWire::Range {
                reference_id,
                inclusion_reason,
                path,
                range,
            } => (
                reference_id,
                inclusion_reason,
                ChatReferenceTarget::Range { path, range },
            ),
            ChatReferenceWire::Selection {
                reference_id,
                inclusion_reason,
                path,
                range,
            } => (
                reference_id,
                inclusion_reason,
                ChatReferenceTarget::Selection { path, range },
            ),
            ChatReferenceWire::Symbol {
                reference_id,
                inclusion_reason,
                path,
                symbol,
                range,
            } => (
                reference_id,
                inclusion_reason,
                ChatReferenceTarget::Symbol {
                    path,
                    symbol,
                    range,
                },
            ),
            ChatReferenceWire::ActiveFile {
                reference_id,
                inclusion_reason,
                path,
            } => (
                reference_id,
                inclusion_reason,
                ChatReferenceTarget::ActiveFile { path },
            ),
            ChatReferenceWire::Finding {
                reference_id,
                inclusion_reason,
                finding_id,
                path,
                range,
            } => (
                reference_id,
                inclusion_reason,
                ChatReferenceTarget::Finding {
                    finding_id,
                    path,
                    range,
                },
            ),
            ChatReferenceWire::Run {
                reference_id,
                inclusion_reason,
                run_id,
            } => (
                reference_id,
                inclusion_reason,
                ChatReferenceTarget::Run { run_id },
            ),
            ChatReferenceWire::Diff {
                reference_id,
                inclusion_reason,
                proposal_id,
            } => (
                reference_id,
                inclusion_reason,
                ChatReferenceTarget::Diff { proposal_id },
            ),
        };
        Ok(Self {
            reference_id,
            inclusion_reason,
            target,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatReferenceTarget {
    File {
        path: String,
    },
    Range {
        path: String,
        range: ChatTextRange,
    },
    Selection {
        path: String,
        range: ChatTextRange,
    },
    Symbol {
        path: String,
        symbol: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        range: Option<ChatTextRange>,
    },
    ActiveFile {
        path: String,
    },
    Finding {
        finding_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        range: Option<ChatTextRange>,
    },
    Run {
        run_id: String,
    },
    Diff {
        proposal_id: String,
    },
}

impl ChatReferenceTarget {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::File { .. } => "file",
            Self::Range { .. } => "range",
            Self::Selection { .. } => "selection",
            Self::Symbol { .. } => "symbol",
            Self::ActiveFile { .. } => "active_file",
            Self::Finding { .. } => "finding",
            Self::Run { .. } => "run",
            Self::Diff { .. } => "diff",
        }
    }

    pub fn path(&self) -> Option<&str> {
        match self {
            Self::File { path }
            | Self::Range { path, .. }
            | Self::Selection { path, .. }
            | Self::Symbol { path, .. }
            | Self::ActiveFile { path } => Some(path),
            Self::Finding { path, .. } => path.as_deref(),
            Self::Run { .. } | Self::Diff { .. } => None,
        }
    }

    pub const fn range(&self) -> Option<&ChatTextRange> {
        match self {
            Self::Range { range, .. } | Self::Selection { range, .. } => Some(range),
            Self::Symbol { range, .. } | Self::Finding { range, .. } => range.as_ref(),
            Self::File { .. } | Self::ActiveFile { .. } | Self::Run { .. } | Self::Diff { .. } => {
                None
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChatTextPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChatTextRange {
    pub start: ChatTextPosition,
    pub end: ChatTextPosition,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChatControlMessage {
    pub schema_version: u32,
    pub protocol: String,
    pub request_id: String,
    #[serde(rename = "type")]
    pub kind: ChatControlKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatControlKind {
    Cancel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatProtocolEvent {
    pub schema_version: u32,
    pub protocol: String,
    pub request_id: String,
    pub sequence: u64,
    pub elapsed_ms: u64,
    #[serde(flatten)]
    pub payload: ChatProtocolEventPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChatEditReviewFile {
    pub path: String,
    pub status: String,
    pub line_ending: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_hash: Option<String>,
    pub proposed_content: String,
    pub proposed_hash: String,
    pub proposed_bytes: usize,
    pub additions: usize,
    pub deletions: usize,
    pub hunks: usize,
}

impl ChatProtocolEvent {
    pub fn is_terminal(&self) -> bool {
        self.payload.is_terminal()
    }

    pub fn output_delta(&self) -> Option<&str> {
        match &self.payload {
            ChatProtocolEventPayload::TokenDelta { text } => Some(text),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatProtocolEventPayload {
    RequestAccepted {
        command: ChatCommand,
        #[serde(default)]
        requested_security_mode: ChatSecurityMode,
        security_mode: ChatSecurityMode,
        #[serde(default)]
        effective_security_mode: ChatSecurityMode,
        #[serde(default)]
        policy_version: String,
        #[serde(default)]
        policy_decision: String,
        #[serde(default)]
        policy_rule_id: String,
    },
    ReferencesResolving {
        count: usize,
    },
    ReferencesResolved {
        accepted: Vec<ChatResolvedReference>,
        rejected: Vec<ChatRejectedReference>,
    },
    ReferenceSelected {
        reference_id: String,
        kind: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        origin: String,
    },
    ReferenceResolved {
        reference: ChatResolvedReference,
    },
    ReferenceInjected {
        reference: ChatResolvedReference,
    },
    ReferenceRefused {
        reference: ChatRejectedReference,
    },
    ContextManifestReady {
        manifest: ContextManifest,
        prompt_fingerprint: String,
    },
    ContextStarted {
        requested_mode: ContextMode,
    },
    ContextReady {
        requested_mode: ContextMode,
        used_mode: Option<ContextMode>,
        analysis_complete: bool,
        estimated_tokens: usize,
        files: Vec<ChatContextFile>,
    },
    RetrievalProgress {
        query_count: usize,
        hit_count: usize,
    },
    ProviderStarted {
        provider: ProviderId,
        model: String,
        context_mode: ContextMode,
    },
    TokenDelta {
        text: String,
    },
    Finding {
        finding_id: String,
        severity: String,
        message: String,
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        range: Option<ChatTextRange>,
    },
    Warning {
        code: String,
        message: String,
    },
    Metrics {
        metrics: ChatMetrics,
    },
    GroundingValidationStarted {
        route: GroundingRoute,
        evidence_mode: ChatEvidenceMode,
    },
    GroundingValidationCompleted {
        evidence: EvidenceValidationReport,
        compliance: ComplianceReport,
    },
    TaskComplianceFailed {
        errors: Vec<String>,
    },
    InternalContextLeakDetected {
        markers: Vec<String>,
    },
    DocumentInspectionCompleted {
        format: String,
        facts: usize,
        model_calls: usize,
    },
    TimingMetrics {
        metrics: ChatMetrics,
    },
    EditPlanStarted {
        plan_id: String,
    },
    EditPlanReady {
        plan_id: String,
        summary: String,
        file_count: usize,
    },
    PolicyDecision {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        proposal_id: Option<String>,
        stage: String,
        action_kind: String,
        decision: String,
        rule_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        audit_event_id: Option<String>,
    },
    ProposalStored {
        proposal_id: String,
        state: String,
        expires_at_unix_ms: u64,
    },
    VerificationStarted {
        proposal_id: String,
    },
    WorktreeCreated {
        proposal_id: String,
        run_id: String,
    },
    EditAppliedInWorktree {
        proposal_id: String,
        success: bool,
    },
    BuildStarted {
        proposal_id: String,
        offline: bool,
    },
    BuildCompleted {
        proposal_id: String,
        success: bool,
        build: String,
        tests: String,
    },
    VerificationCompleted {
        proposal_id: String,
        success: bool,
        build: String,
        tests: String,
    },
    DiffReady {
        proposal_id: String,
        files: usize,
        additions: usize,
        deletions: usize,
        display_patch: String,
        display_truncated: bool,
        changes: Vec<ChatEditReviewFile>,
    },
    ApprovalRequired {
        proposal_id: String,
        approval_request_id: String,
        operation: String,
        summary: String,
    },
    ApplyStarted {
        proposal_id: String,
        transaction_id: String,
    },
    ApplyCompleted {
        proposal_id: String,
        transaction_id: String,
        success: bool,
    },
    RollbackAvailable {
        proposal_id: String,
        transaction_id: String,
    },
    RollbackStarted {
        proposal_id: String,
        transaction_id: String,
    },
    RollbackCompleted {
        proposal_id: String,
        transaction_id: String,
        success: bool,
        already_rolled_back: bool,
    },
    ProposalDiscarded {
        proposal_id: String,
    },
    Completed {
        summary: Box<ChatCompletionSummary>,
    },
    Cancelled {
        reason: String,
    },
    Failed {
        error: ChatProtocolError,
    },
}

impl ChatProtocolEventPayload {
    pub const fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed { .. } | Self::Cancelled { .. } | Self::Failed { .. }
        )
    }

    fn validate_bounds(&self) -> Result<()> {
        let text = match self {
            Self::TokenDelta { text } => Some(text.as_str()),
            Self::Warning { message, .. }
            | Self::Finding { message, .. }
            | Self::EditPlanReady {
                summary: message, ..
            }
            | Self::ApprovalRequired {
                summary: message, ..
            } => Some(message.as_str()),
            Self::Cancelled { reason } => Some(reason.as_str()),
            Self::Failed { error } => Some(error.message.as_str()),
            _ => None,
        };
        if text.is_some_and(|value| value.len() > MAX_CHAT_EVENT_TEXT_BYTES) {
            bail!("chat event text exceeds its bounded payload limit");
        }
        if let Self::TaskComplianceFailed { errors }
        | Self::InternalContextLeakDetected { markers: errors } = self
        {
            if errors.len() > 128
                || errors.iter().map(String::len).sum::<usize>() > MAX_CHAT_EVENT_TEXT_BYTES
            {
                bail!("chat grounding diagnostics exceed their bounded payload limit");
            }
        }
        if let Self::DiffReady {
            display_patch,
            changes,
            ..
        } = self
        {
            if display_patch.len() > 1024 * 1024 || changes.len() > 5 {
                bail!("chat diff review event exceeds its bounded collection or patch limit");
            }
            let snapshot_bytes = changes.iter().try_fold(0usize, |total, file| {
                if file.path.len() > 4 * 1024
                    || file.proposed_content.len() > 512 * 1024
                    || file
                        .base_content
                        .as_ref()
                        .is_some_and(|content| content.len() > 512 * 1024)
                {
                    bail!("chat diff review file exceeds its bounded payload limit");
                }
                Ok(total
                    .saturating_add(file.proposed_content.len())
                    .saturating_add(file.base_content.as_ref().map_or(0, String::len)))
            })?;
            if snapshot_bytes > 4 * 1024 * 1024 {
                bail!("chat diff review snapshots exceed their aggregate payload limit");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatResolvedReference {
    pub reference_id: String,
    pub kind: String,
    pub path: Option<String>,
    pub range: Option<ChatTextRange>,
    pub inclusion_reason: String,
    pub provenance: String,
    pub bytes: usize,
    pub content_hash: Option<String>,
    #[serde(default)]
    pub origin: String,
    #[serde(default)]
    pub resolution: String,
    #[serde(default)]
    pub security_decision: String,
    #[serde(default)]
    pub injection: String,
    #[serde(default)]
    pub bytes_injected: usize,
    #[serde(default)]
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_content_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatRejectedReference {
    pub reference_id: String,
    pub kind: String,
    pub rule_id: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default)]
    pub origin: String,
    #[serde(default)]
    pub injection: String,
    #[serde(default)]
    pub reason_code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatContextFile {
    pub path: String,
    pub snippets: usize,
    pub provenance: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ChatMetrics {
    pub preparation_ms: u64,
    pub total_ms: u64,
    pub estimated_prompt_tokens: usize,
    pub prompt_tokens: Option<u64>,
    pub generated_tokens: Option<u64>,
    pub generated_tokens_per_second: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing: Option<ChatTimingReport>,
    #[serde(default)]
    pub route: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChatTimingPhase {
    pub name: String,
    pub duration_ms: u64,
    pub measured_by: String,
    pub includes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChatTimingReport {
    pub schema_version: u32,
    #[serde(default)]
    pub request_id: String,
    #[serde(default)]
    pub run_id: String,
    #[serde(default)]
    pub workspace_id: String,
    #[serde(default)]
    pub command: String,
    pub unit: String,
    pub clock: String,
    pub phases: Vec<ChatTimingPhase>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChatGroundingSummary {
    pub schema_version: u32,
    pub route: GroundingRoute,
    pub requested_scope: ChatContextScope,
    pub effective_scope: ChatContextScope,
    pub scope_reason: ChatScopeReason,
    pub evidence_mode: ChatEvidenceMode,
    pub selected_references: usize,
    pub resolved_references: usize,
    pub injected_references: usize,
    pub refused_references: usize,
    pub discovered_files: usize,
    pub rag_hits: usize,
    pub historical_turns: usize,
    pub prompt_fingerprint: String,
    pub manifest: ContextManifest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<GroundedResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<EvidenceValidationReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compliance: Option<ComplianceReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatCompletionSummary {
    pub command: ChatCommand,
    pub success: bool,
    pub model: String,
    pub requested_context_mode: ContextMode,
    pub used_context_mode: Option<ContextMode>,
    pub references: Vec<ChatResolvedReference>,
    pub rejected_references: usize,
    pub context_files: Vec<ChatContextFile>,
    pub warnings: Vec<String>,
    pub metrics: ChatMetrics,
    pub repository_state: String,
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grounding: Option<ChatGroundingSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatProtocolError {
    pub code: String,
    pub stage: String,
    pub message: String,
    pub retriable: bool,
}

pub fn chat_event_channel(capacity: usize) -> Result<(ChatEventSink, ChatEventReceiver)> {
    if capacity == 0 || capacity > MAX_CHAT_EVENT_CAPACITY {
        bail!("chat event channel capacity must be between 1 and {MAX_CHAT_EVENT_CAPACITY}");
    }
    Ok(mpsc::channel(capacity))
}

pub fn validate_chat_request_id(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > MAX_CHAT_REQUEST_ID_BYTES
        || value
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte)))
    {
        bail!(
            "chat request id must contain 1-{MAX_CHAT_REQUEST_ID_BYTES} ASCII letters, digits, '-', '_', '.' or ':'"
        );
    }
    Ok(())
}

pub fn chat_setup_failure_event(
    request_id: &str,
    code: impl Into<String>,
    message: impl Into<String>,
) -> ChatProtocolEvent {
    ChatProtocolEvent {
        schema_version: CHAT_PROTOCOL_SCHEMA_VERSION,
        protocol: CHAT_PROTOCOL_ID.to_string(),
        request_id: request_id.to_string(),
        sequence: 0,
        elapsed_ms: 0,
        payload: ChatProtocolEventPayload::Failed {
            error: ChatProtocolError {
                code: code.into(),
                stage: "request_decode".to_string(),
                message: message.into(),
                retriable: false,
            },
        },
    }
}

#[derive(Clone)]
pub(crate) struct ChatEventEmitter {
    request_id: Arc<str>,
    events: ChatEventSink,
    sequence: Arc<AtomicU64>,
    terminal: Arc<AtomicBool>,
    started: Instant,
}

impl ChatEventEmitter {
    pub(crate) fn new(session: &ChatProtocolSession) -> Result<Self> {
        validate_chat_request_id(&session.request_id)?;
        Ok(Self {
            request_id: Arc::from(session.request_id.as_str()),
            events: session.events.clone(),
            sequence: Arc::new(AtomicU64::new(0)),
            terminal: Arc::new(AtomicBool::new(false)),
            started: Instant::now(),
        })
    }

    pub(crate) async fn send(&self, payload: ChatProtocolEventPayload) -> Result<()> {
        payload.validate_bounds()?;
        if self.terminal.load(Ordering::Acquire) {
            bail!("chat protocol cannot emit an event after its terminal event");
        }
        if payload.is_terminal() && self.terminal.swap(true, Ordering::AcqRel) {
            bail!("chat protocol cannot emit more than one terminal event");
        }
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let event = ChatProtocolEvent {
            schema_version: CHAT_PROTOCOL_SCHEMA_VERSION,
            protocol: CHAT_PROTOCOL_ID.to_string(),
            request_id: self.request_id.to_string(),
            sequence,
            elapsed_ms: self.started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            payload,
        };
        tokio::time::timeout(EVENT_DELIVERY_TIMEOUT, self.events.send(event))
            .await
            .context("chat protocol event delivery timed out")?
            .context("chat protocol event receiver was closed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> ChatRequest {
        ChatRequest {
            schema_version: CHAT_PROTOCOL_SCHEMA_VERSION,
            protocol: CHAT_PROTOCOL_ID.to_string(),
            request_id: "chat-request-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            workspace_root: "C:/workspace".to_string(),
            command: ChatCommand::Ask,
            prompt: "Explain Plugin.java".to_string(),
            profile: "minecraft-java-1.8".to_string(),
            provider: ProviderId::Ollama,
            model: "qwen2.5-coder:14b".to_string(),
            context_mode: ContextMode::Legacy,
            context_scope: ChatContextScope::Automatic,
            scope_reason: ChatScopeReason::DefaultSetting,
            evidence_mode: ChatEvidenceMode::Optional,
            references: Vec::new(),
            history: Vec::new(),
            budgets: ChatBudgets::default(),
            generation: ChatGenerationOptions::default(),
            security_mode: ChatSecurityMode::ReadOnly,
            client: ChatClientMetadata {
                name: "vscode".to_string(),
                version: "0.2.0".to_string(),
                vscode_version: "1.125.0".to_string(),
                session_id: "session-1".to_string(),
                locale: "fr".to_string(),
                recent_run_ids: Vec::new(),
                previous_repository_state: None,
            },
            expected_protocols: ChatExpectedProtocols::default(),
            edit: None,
        }
    }

    #[test]
    fn request_schema_round_trips_and_unknown_commands_fail_closed() {
        let encoded = serde_json::to_string(&request()).unwrap();
        let decoded: ChatRequest = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.command, ChatCommand::Ask);

        let unknown = encoded.replace("\"ask\"", "\"future_tool\"");
        let decoded: ChatRequest = serde_json::from_str(&unknown).unwrap();
        assert_eq!(decoded.command, ChatCommand::Unknown);
    }

    #[test]
    fn pre_policy_acceptance_events_remain_decodable() {
        let payload: ChatProtocolEventPayload = serde_json::from_value(serde_json::json!({
            "type": "request_accepted",
            "command": "ask",
            "security_mode": "read_only"
        }))
        .unwrap();
        assert!(matches!(
            payload,
            ChatProtocolEventPayload::RequestAccepted {
                requested_security_mode: ChatSecurityMode::ReadOnly,
                effective_security_mode: ChatSecurityMode::ReadOnly,
                ref policy_version,
                ref policy_decision,
                ref policy_rule_id,
                ..
            } if policy_version.is_empty()
                && policy_decision.is_empty()
                && policy_rule_id.is_empty()
        ));
    }

    #[test]
    fn flattened_references_decode_strictly() {
        let mut encoded = serde_json::to_value(request()).unwrap();
        encoded["references"] = serde_json::json!([{
            "reference_id": "selection-1",
            "inclusion_reason": "selected by user",
            "kind": "selection",
            "path": "src/Plugin.java",
            "range": {
                "start": { "line": 1, "character": 2 },
                "end": { "line": 1, "character": 6 }
            }
        }]);
        let decoded: ChatRequest = serde_json::from_value(encoded.clone()).unwrap();
        assert!(matches!(
            decoded.references[0].target,
            ChatReferenceTarget::Selection { .. }
        ));

        encoded["references"][0]["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<ChatRequest>(encoded).is_err());
    }

    #[tokio::test]
    async fn events_are_versioned_sequenced_and_terminal_once() {
        let (events, mut receiver) = chat_event_channel(4).unwrap();
        let session = ChatProtocolSession {
            request_id: "chat-events-1".to_string(),
            events,
            cancellation: CancellationToken::new(),
        };
        let emitter = ChatEventEmitter::new(&session).unwrap();
        emitter
            .send(ChatProtocolEventPayload::RequestAccepted {
                command: ChatCommand::Help,
                requested_security_mode: ChatSecurityMode::ReadOnly,
                security_mode: ChatSecurityMode::ReadOnly,
                effective_security_mode: ChatSecurityMode::ReadOnly,
                policy_version: "opticcode.default.v1".to_string(),
                policy_decision: "allow".to_string(),
                policy_rule_id: "analysis.context_read_only".to_string(),
            })
            .await
            .unwrap();
        emitter
            .send(ChatProtocolEventPayload::Cancelled {
                reason: "test".to_string(),
            })
            .await
            .unwrap();
        assert!(emitter
            .send(ChatProtocolEventPayload::TokenDelta {
                text: "late".to_string(),
            })
            .await
            .is_err());

        let first = receiver.recv().await.unwrap();
        let terminal = receiver.recv().await.unwrap();
        assert_eq!(first.sequence, 0);
        assert_eq!(terminal.sequence, 1);
        assert!(terminal.is_terminal());
        assert_eq!(terminal.protocol, CHAT_PROTOCOL_ID);
    }

    #[test]
    fn request_ids_and_payloads_are_bounded() {
        assert!(validate_chat_request_id("vscode-chat:1").is_ok());
        assert!(validate_chat_request_id("../escape").is_err());
        assert!(ChatProtocolEventPayload::TokenDelta {
            text: "x".repeat(MAX_CHAT_EVENT_TEXT_BYTES + 1),
        }
        .validate_bounds()
        .is_err());
    }
}
