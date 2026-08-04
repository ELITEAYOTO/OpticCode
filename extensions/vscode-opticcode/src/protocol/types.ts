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
    rag: boolean;
    java: boolean;
    worktrees: boolean;
    verified_edits: boolean;
    evaluation: boolean;
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
