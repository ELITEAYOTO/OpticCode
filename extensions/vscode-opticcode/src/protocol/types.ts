export type JsonObject = Record<string, unknown>;

export interface ProtocolDescriptor {
  id: string;
  schema_version: number;
}

export interface VersionReport extends JsonObject {
  schema_version: number;
  protocol: string;
  opticcode_version: string;
  protocols: {
    assistant: ProtocolDescriptor;
    chat: ProtocolDescriptor;
    discovery: ProtocolDescriptor;
    llm: ProtocolDescriptor;
  };
  schemas: Record<string, number>;
  platform: {
    os: string;
    architecture: string;
  };
  build: {
    kind: string;
    commit?: string | null;
  };
}

export interface CapabilitiesReport extends JsonObject {
  schema_version: number;
  protocol: string;
  commands: string[];
  providers: Array<{
    id: string;
    active: boolean;
    capabilities: Record<string, boolean>;
  }>;
  context_modes: string[];
  machine_output: {
    json: boolean;
    ndjson: boolean;
    streaming: boolean;
    cancellation: boolean;
  };
  features: {
    chat: boolean;
    policy?: boolean;
    rag: boolean;
    java: boolean;
    worktrees: boolean;
    verified_edits: boolean;
    evaluation: boolean;
  };
  policy_runtime?: {
    schema_version: number;
    policy_version: string;
    engine: boolean;
    modes: Array<'read_only' | 'worktree_edit' | 'approved_apply'>;
    audit: boolean;
    approvals: boolean;
    cli: boolean;
    chat_read_only: boolean;
    chat_write: boolean;
  };
}

export type DoctorStatus = 'ok' | 'warning' | 'error' | 'unavailable';

export interface DoctorCheck extends JsonObject {
  id: string;
  status: DoctorStatus;
  required: boolean;
  summary: string;
  version?: string;
  path?: string;
}

export interface DoctorReport extends JsonObject {
  schema_version: number;
  protocol: string;
  success: boolean;
  workspace: string;
  profile: string;
  model: string;
  provider: string;
  checks: DoctorCheck[];
}

export type ContextMode = 'legacy' | 'symbol' | 'compare';
export type ChatCommand =
  | 'ask'
  | 'plan'
  | 'inspect'
  | 'context'
  | 'analyze'
  | 'index'
  | 'legacy'
  | 'fix'
  | 'verify'
  | 'diff'
  | 'apply'
  | 'rollback'
  | 'status'
  | 'runs'
  | 'help';
export type ChatSecurityMode = 'read_only' | 'worktree_edit' | 'approved_apply';
export type ChatHistoryRole = 'user' | 'assistant';
export type ChatContextScope = 'automatic' | 'references_preferred' | 'references_only';
export type ChatScopeReason =
  | 'explicit_setting'
  | 'explicit_prompt_restriction'
  | 'default_setting'
  | 'server_downgrade';
export type ChatEvidenceMode = 'optional' | 'required';
export type GroundingRoute = 'automatic_assistant' | 'reference_llm' | 'document_facts';

export interface ChatTextPosition {
  line: number;
  character: number;
}

export interface ChatTextRange {
  start: ChatTextPosition;
  end: ChatTextPosition;
}

export type ChatReferenceTarget =
  | { kind: 'file'; path: string }
  | { kind: 'range'; path: string; range: ChatTextRange }
  | { kind: 'selection'; path: string; range: ChatTextRange }
  | { kind: 'symbol'; path: string; symbol: string; range?: ChatTextRange | undefined }
  | { kind: 'active_file'; path: string }
  | {
      kind: 'finding';
      finding_id: string;
      path?: string | undefined;
      range?: ChatTextRange | undefined;
    }
  | { kind: 'run'; run_id: string }
  | { kind: 'diff'; proposal_id: string };

export type ChatReference = {
  reference_id: string;
  inclusion_reason: string;
} & ChatReferenceTarget;

export interface ChatHistoryTurn {
  role: ChatHistoryRole;
  content: string;
  command?: ChatCommand | undefined;
  result_id?: string | undefined;
  source_scope?: ChatContextScope | undefined;
  workspace_id?: string | undefined;
  context_fingerprint?: string | undefined;
  grounding_status?: string | undefined;
}

export interface ChatBudgets {
  max_history_turns: number;
  max_history_chars: number;
  max_history_tokens: number;
  max_references: number;
  max_reference_bytes: number;
  max_prompt_tokens: number;
  rag_hits: number;
}

export interface ChatGenerationOptions {
  max_output_tokens: number;
  temperature?: number | null | undefined;
  seed?: number | null | undefined;
  brief: boolean;
  compare_generate: boolean;
}

export interface ChatClientMetadata {
  name: string;
  version: string;
  vscode_version: string;
  session_id: string;
  locale: string;
  recent_run_ids: string[];
  previous_repository_state?: string | null | undefined;
}

export interface ChatExpectedProtocols {
  chat: number;
  assistant: number;
  discovery: number;
  llm: number;
}

export interface ChatNativeConfirmation {
  client: string;
  confirmation_id: string;
  approval_request_id: string;
}

export interface ChatEditControl {
  proposal_id?: string | undefined;
  transaction_id?: string | undefined;
  native_confirmation?: ChatNativeConfirmation | undefined;
  discard?: boolean | undefined;
}

export interface ChatProtocolRequest extends JsonObject {
  schema_version: number;
  protocol: string;
  request_id: string;
  workspace_id: string;
  workspace_root: string;
  command: ChatCommand;
  prompt: string;
  profile: string;
  provider: 'ollama';
  model: string;
  context_mode: ContextMode;
  context_scope: ChatContextScope;
  scope_reason: ChatScopeReason;
  evidence_mode: ChatEvidenceMode;
  references: ChatReference[];
  history: ChatHistoryTurn[];
  budgets: ChatBudgets;
  generation: ChatGenerationOptions;
  security_mode: ChatSecurityMode;
  client: ChatClientMetadata;
  expected_protocols: ChatExpectedProtocols;
  edit?: ChatEditControl | undefined;
}

export interface ChatEditReviewFile {
  path: string;
  status: 'modified' | 'created';
  line_ending: 'none' | 'lf' | 'crlf';
  base_content?: string | undefined;
  base_hash?: string | undefined;
  proposed_content: string;
  proposed_hash: string;
  proposed_bytes: number;
  additions: number;
  deletions: number;
  hunks: number;
}

export interface ChatResolvedReference {
  reference_id: string;
  kind: string;
  path: string | null;
  range: ChatTextRange | null;
  inclusion_reason: string;
  provenance: string;
  bytes: number;
  content_hash: string | null;
  origin?: string | undefined;
  resolution?: string | undefined;
  security_decision?: string | undefined;
  injection?: string | undefined;
  bytes_injected?: number | undefined;
  reason?: string | undefined;
  full_content_hash?: string | null | undefined;
}

export interface ChatRejectedReference {
  reference_id: string;
  kind: string;
  rule_id: string;
  reason: string;
  path?: string | null | undefined;
  origin?: string | undefined;
  injection?: string | undefined;
  reason_code?: string | undefined;
}

export interface ChatContextFile {
  path: string;
  snippets: number;
  provenance: string;
}

export interface ChatMetrics {
  preparation_ms: number;
  total_ms: number;
  estimated_prompt_tokens: number;
  prompt_tokens: number | null;
  generated_tokens: number | null;
  generated_tokens_per_second: number | null;
  timing?: ChatTimingReport | null | undefined;
  route?: string | undefined;
}

export interface ChatTimingPhase {
  name: string;
  duration_ms: number;
  measured_by: string;
  includes: string[];
}

export interface ChatTimingReport {
  schema_version: number;
  request_id?: string | undefined;
  run_id?: string | undefined;
  workspace_id?: string | undefined;
  command?: string | undefined;
  unit: string;
  clock: string;
  phases: ChatTimingPhase[];
}

export interface ContextManifestRange {
  start_line: number;
  end_line: number;
  start_byte: number;
  end_byte: number;
}

export interface ContextManifestEntry {
  reference_id: string;
  path: string;
  origin: string;
  hash: string;
  injected_hash: string;
  size_bytes: number;
  encoding: string;
  line_ending: string;
  ranges: ContextManifestRange[];
  bytes_injected: number;
  reason: string;
  git_state: string;
  workspace_id: string;
}

export interface ContextManifest {
  schema_version: number;
  context_scope: ChatContextScope;
  workspace_id: string;
  request_id: string;
  prompt_version: string;
  profile: string;
  entries: ContextManifestEntry[];
  total_bytes: number;
  estimated_tokens: number;
  fingerprint: string;
}

export type ClaimClassification =
  | 'observed'
  | 'inferred'
  | 'general_knowledge'
  | 'insufficient_evidence';

export interface EvidenceCitation {
  path: string;
  start_line: number;
  end_line: number;
  content_hash: string;
}

export interface GroundedClaim {
  claim_id: string;
  text: string;
  classification: ClaimClassification;
  evidence: EvidenceCitation[];
}

export interface GroundedResponse {
  schema_version: number;
  answer: string;
  claims: GroundedClaim[];
  missing_information: string[];
  used_general_knowledge: boolean;
}

export interface EvidenceValidationReport {
  valid: boolean;
  claims_checked: number;
  citations_checked: number;
  errors: string[];
}

export interface ComplianceReport {
  compliant: boolean;
  internal_context_leak: boolean;
  cross_file_leak: boolean;
  task_format_violation: boolean;
  errors: string[];
}

export interface ChatGroundingSummary {
  schema_version: number;
  route: GroundingRoute;
  requested_scope: ChatContextScope;
  effective_scope: ChatContextScope;
  scope_reason: ChatScopeReason;
  evidence_mode: ChatEvidenceMode;
  selected_references: number;
  resolved_references: number;
  injected_references: number;
  refused_references: number;
  discovered_files: number;
  rag_hits: number;
  historical_turns: number;
  prompt_fingerprint: string;
  manifest: ContextManifest;
  response?: GroundedResponse | null | undefined;
  evidence?: EvidenceValidationReport | null | undefined;
  compliance?: ComplianceReport | null | undefined;
}

export interface ChatCompletionSummary {
  command: ChatCommand;
  success: boolean;
  model: string;
  requested_context_mode: ContextMode;
  used_context_mode: ContextMode | null;
  references: ChatResolvedReference[];
  rejected_references: number;
  context_files: ChatContextFile[];
  warnings: string[];
  metrics: ChatMetrics;
  repository_state: string;
  run_id: string;
  grounding?: ChatGroundingSummary | null | undefined;
}

interface ChatEventBase extends JsonObject {
  schema_version: number;
  protocol: string;
  request_id: string;
  sequence: number;
  elapsed_ms: number;
}

export type ChatProtocolEvent = ChatEventBase &
  (
    | {
        type: 'request_accepted';
        command: ChatCommand;
        requested_security_mode?: ChatSecurityMode;
        security_mode: ChatSecurityMode;
        effective_security_mode?: ChatSecurityMode;
        policy_version?: string;
        policy_decision?: 'allow' | 'require_approval' | 'deny';
        policy_rule_id?: string;
      }
    | { type: 'references_resolving'; count: number }
    | {
        type: 'references_resolved';
        accepted: ChatResolvedReference[];
        rejected: ChatRejectedReference[];
      }
    | {
        type: 'reference_selected';
        reference_id: string;
        kind: string;
        path?: string | null | undefined;
        origin: string;
      }
    | { type: 'reference_resolved'; reference: ChatResolvedReference }
    | { type: 'reference_injected'; reference: ChatResolvedReference }
    | { type: 'reference_refused'; reference: ChatRejectedReference }
    | {
        type: 'context_manifest_ready';
        manifest: ContextManifest;
        prompt_fingerprint: string;
      }
    | { type: 'context_started'; requested_mode: ContextMode }
    | {
        type: 'context_ready';
        requested_mode: ContextMode;
        used_mode: ContextMode | null;
        analysis_complete: boolean;
        estimated_tokens: number;
        files: ChatContextFile[];
      }
    | { type: 'retrieval_progress'; query_count: number; hit_count: number }
    | {
        type: 'provider_started';
        provider: 'ollama';
        model: string;
        context_mode: ContextMode;
      }
    | { type: 'token_delta'; text: string }
    | {
        type: 'finding';
        finding_id: string;
        severity: string;
        message: string;
        path: string;
        range?: ChatTextRange | null | undefined;
      }
    | { type: 'warning'; code: string; message: string }
    | { type: 'metrics'; metrics: ChatMetrics }
    | {
        type: 'grounding_validation_started';
        route: GroundingRoute;
        evidence_mode: ChatEvidenceMode;
      }
    | {
        type: 'grounding_validation_completed';
        evidence: EvidenceValidationReport;
        compliance: ComplianceReport;
      }
    | { type: 'task_compliance_failed'; errors: string[] }
    | { type: 'internal_context_leak_detected'; markers: string[] }
    | {
        type: 'document_inspection_completed';
        format: string;
        facts: number;
        model_calls: number;
      }
    | { type: 'timing_metrics'; metrics: ChatMetrics }
    | { type: 'edit_plan_started'; plan_id: string }
    | { type: 'edit_plan_ready'; plan_id: string; summary: string; file_count: number }
    | {
        type: 'policy_decision';
        proposal_id?: string | undefined;
        stage: string;
        action_kind: string;
        decision: string;
        rule_id: string;
        audit_event_id?: string | undefined;
      }
    | {
        type: 'proposal_stored';
        proposal_id: string;
        state: string;
        expires_at_unix_ms: number;
      }
    | { type: 'verification_started'; proposal_id: string }
    | { type: 'worktree_created'; proposal_id: string; run_id: string }
    | { type: 'edit_applied_in_worktree'; proposal_id: string; success: boolean }
    | { type: 'build_started'; proposal_id: string; offline: boolean }
    | {
        type: 'build_completed';
        proposal_id: string;
        success: boolean;
        build: string;
        tests: string;
      }
    | {
        type: 'verification_completed';
        proposal_id: string;
        success: boolean;
        build: string;
        tests: string;
      }
    | {
        type: 'diff_ready';
        proposal_id: string;
        files: number;
        additions: number;
        deletions: number;
        display_patch?: string | undefined;
        display_truncated?: boolean | undefined;
        changes?: ChatEditReviewFile[] | undefined;
      }
    | {
        type: 'approval_required';
        proposal_id: string;
        approval_request_id: string;
        operation?: 'apply' | 'rollback' | undefined;
        summary: string;
      }
    | { type: 'apply_started'; proposal_id: string; transaction_id: string }
    | {
        type: 'apply_completed';
        proposal_id: string;
        transaction_id: string;
        success: boolean;
      }
    | { type: 'rollback_available'; proposal_id?: string | undefined; transaction_id: string }
    | { type: 'rollback_started'; proposal_id: string; transaction_id: string }
    | {
        type: 'rollback_completed';
        proposal_id: string;
        transaction_id: string;
        success: boolean;
        already_rolled_back: boolean;
      }
    | { type: 'proposal_discarded'; proposal_id: string }
    | { type: 'completed'; summary: ChatCompletionSummary }
    | { type: 'cancelled'; reason: string }
    | {
        type: 'failed';
        error: { code: string; stage: string; message: string; retriable: boolean };
      }
  );

export type ChatTerminalType = 'completed' | 'failed' | 'cancelled';

export interface ChatStreamResult {
  requestId: string;
  status: ChatTerminalType;
  response: string;
  events: ChatProtocolEvent[];
  terminal: ChatProtocolEvent;
  summary?: ChatCompletionSummary | undefined;
  durationMs: number;
  exitCode: number | null;
  stderr: string;
  cancellationConfirmed: boolean;
  clientTiming?: ChatClientTiming | undefined;
  uiTiming?: ChatUiTiming | undefined;
}

export interface ChatClientTiming {
  schema_version: 1;
  request_id: string;
  clock: 'performance.now';
  transport_started_ms: 0;
  child_spawn_started_ms: number;
  child_spawned_ms: number;
  request_written_ms: number | null;
  first_protocol_event_ms: number | null;
  first_content_delta_ms: number | null;
  last_content_delta_ms: number | null;
  terminal_received_ms: number | null;
  process_completed_ms: number;
}

export interface ChatUiTiming {
  schema_version: 1;
  request_id: string;
  clock: 'performance.now';
  first_token_ms: number | null;
  answer_streaming_ms: number;
  visible_response_ms: number | null;
  total_pipeline_ms: number;
  post_processing_ms: number;
  terminal_rendered_ms: number;
  report_persisted_ms?: number | undefined;
  handler_completed_ms?: number | undefined;
}

export type AssistantTerminalType = 'completed' | 'failed' | 'cancelled';

export interface AssistantCompletionContextFile {
  context_mode: ContextMode;
  path: string;
  snippets: number;
  max_score?: number | null | undefined;
}

export interface AssistantCompletionRun {
  context_mode: ContextMode;
  generated: boolean;
  estimated_prompt_tokens: number;
  client_ms?: number | null | undefined;
  prompt_tokens?: number | null | undefined;
  generated_tokens?: number | null | undefined;
  generated_tokens_per_second?: number | null | undefined;
}

export interface AssistantCompletionSummary {
  command: 'ask' | 'plan';
  success: boolean;
  model: string;
  requested_context_mode: ContextMode;
  used_context_mode?: ContextMode | null | undefined;
  preparation_duration_us: number;
  warnings: string[];
  context_files: AssistantCompletionContextFile[];
  runs: AssistantCompletionRun[];
}

export interface GenerationResult {
  schema_version: number;
  request_id: string;
  provider: string;
  model: string;
  output: string;
  finish_reason: string;
  prompt_chars: number;
  usage: {
    prompt_tokens?: number | undefined;
    generated_tokens?: number | undefined;
  };
  timings: {
    client_ms: number;
    provider_total_ms?: number | undefined;
    load_ms?: number | undefined;
    prompt_eval_ms?: number | undefined;
    generation_ms?: number | undefined;
  };
}

export interface LlmProtocolEvent extends JsonObject {
  schema_version: number;
  protocol: string;
  request_id: string;
  sequence: number;
  type: 'started' | 'delta' | 'completed' | 'failed' | 'cancelled';
  text?: string;
  result?: GenerationResult;
  error?: JsonObject;
  reason?: string;
}

export interface AssistantProtocolEvent extends JsonObject {
  schema_version: number;
  protocol: string;
  request_id: string;
  sequence: number;
  type:
    | 'started'
    | 'context_prepared'
    | 'provider_event'
    | AssistantTerminalType;
  command?: 'ask' | 'plan';
  provider?: string;
  model?: string;
  requested_context_mode?: ContextMode;
  used_context_mode?: ContextMode | null;
  context_mode?: ContextMode;
  event?: LlmProtocolEvent;
  summary?: AssistantCompletionSummary;
  errors?: JsonObject[];
}

export type AssistantRunStatus = 'completed' | 'failed' | 'cancelled';

export interface AssistantStreamResult {
  requestId: string;
  status: AssistantRunStatus;
  response: string;
  events: AssistantProtocolEvent[];
  terminal: AssistantProtocolEvent;
  summary?: AssistantCompletionSummary | undefined;
  generation?: GenerationResult | undefined;
  durationMs: number;
  exitCode: number | null;
  stderr: string;
  cancellationConfirmed: boolean;
}

export interface CancellationLike {
  readonly isCancellationRequested: boolean;
  onCancellationRequested(listener: () => void): { dispose(): void };
}
