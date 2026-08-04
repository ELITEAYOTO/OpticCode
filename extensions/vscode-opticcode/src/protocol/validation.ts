import { OpticCodeClientError } from './errors';
import type {
  AssistantCompletionContextFile,
  AssistantCompletionRun,
  AssistantCompletionSummary,
  AssistantProtocolEvent,
  CapabilitiesReport,
  ContextMode,
  DoctorCheck,
  DoctorReport,
  GenerationResult,
  JsonObject,
  LlmProtocolEvent,
  VersionReport,
} from './types';

export const DISCOVERY_PROTOCOL = 'opticcode.discovery';
export const ASSISTANT_PROTOCOL = 'opticcode.assistant';
export const LLM_PROTOCOL = 'opticcode.llm';
export const SUPPORTED_SCHEMA_VERSION = 1;

export function isRecord(value: unknown): value is JsonObject {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function incompatible(message: string): never {
  throw new OpticCodeClientError('protocol_incompatible', message);
}

function requireRecord(value: unknown, field: string): JsonObject {
  if (!isRecord(value)) {
    incompatible(`Expected object at ${field}.`);
  }
  return value;
}

function requireString(value: unknown, field: string): string {
  if (typeof value !== 'string') {
    incompatible(`Expected string at ${field}.`);
  }
  return value;
}

function requireBoolean(value: unknown, field: string): boolean {
  if (typeof value !== 'boolean') {
    incompatible(`Expected boolean at ${field}.`);
  }
  return value;
}

function requireNumber(value: unknown, field: string): number {
  if (typeof value !== 'number' || !Number.isFinite(value)) {
    incompatible(`Expected finite number at ${field}.`);
  }
  return value;
}

function requireInteger(value: unknown, field: string): number {
  const number = requireNumber(value, field);
  if (!Number.isSafeInteger(number) || number < 0) {
    incompatible(`Expected non-negative integer at ${field}.`);
  }
  return number;
}

function requireArray(value: unknown, field: string): unknown[] {
  if (!Array.isArray(value)) {
    incompatible(`Expected array at ${field}.`);
  }
  return value;
}

function requireStringArray(value: unknown, field: string): string[] {
  return requireArray(value, field).map((entry, index) =>
    requireString(entry, `${field}[${index}]`),
  );
}

function requireProtocol(
  value: JsonObject,
  expectedProtocol: string,
  expectedSchema = SUPPORTED_SCHEMA_VERSION,
): void {
  const protocol = requireString(value.protocol, 'protocol');
  const schema = requireInteger(value.schema_version, 'schema_version');
  if (protocol !== expectedProtocol || schema !== expectedSchema) {
    incompatible(
      `Unsupported protocol ${protocol} schema ${schema}; expected ${expectedProtocol} schema ${expectedSchema}.`,
    );
  }
}

export function validateVersionReport(value: unknown): VersionReport {
  const report = requireRecord(value, 'version');
  requireProtocol(report, DISCOVERY_PROTOCOL);
  requireString(report.opticcode_version, 'opticcode_version');
  const protocols = requireRecord(report.protocols, 'protocols');
  for (const [name, id] of [
    ['assistant', ASSISTANT_PROTOCOL],
    ['discovery', DISCOVERY_PROTOCOL],
    ['llm', LLM_PROTOCOL],
  ] as const) {
    const descriptor = requireRecord(protocols[name], `protocols.${name}`);
    if (
      requireString(descriptor.id, `protocols.${name}.id`) !== id ||
      requireInteger(descriptor.schema_version, `protocols.${name}.schema_version`) !==
        SUPPORTED_SCHEMA_VERSION
    ) {
      incompatible(`Unsupported ${name} protocol descriptor.`);
    }
  }
  requireRecord(report.schemas, 'schemas');
  requireRecord(report.platform, 'platform');
  requireRecord(report.build, 'build');
  return report as VersionReport;
}

export function validateCapabilitiesReport(value: unknown): CapabilitiesReport {
  const report = requireRecord(value, 'capabilities');
  requireProtocol(report, DISCOVERY_PROTOCOL);
  const commands = requireStringArray(report.commands, 'commands');
  for (const required of ['version', 'capabilities', 'doctor', 'ask', 'plan']) {
    if (!commands.includes(required)) {
      incompatible(`Required command ${required} is not advertised.`);
    }
  }
  const output = requireRecord(report.machine_output, 'machine_output');
  for (const field of ['json', 'ndjson', 'streaming', 'cancellation']) {
    if (!requireBoolean(output[field], `machine_output.${field}`)) {
      incompatible(`Required machine output capability ${field} is disabled.`);
    }
  }
  requireArray(report.providers, 'providers');
  requireStringArray(report.context_modes, 'context_modes');
  requireRecord(report.features, 'features');
  return report as CapabilitiesReport;
}

export function validateDoctorReport(value: unknown): DoctorReport {
  const report = requireRecord(value, 'doctor');
  requireProtocol(report, DISCOVERY_PROTOCOL);
  requireBoolean(report.success, 'success');
  requireString(report.workspace, 'workspace');
  requireString(report.profile, 'profile');
  requireString(report.model, 'model');
  requireString(report.provider, 'provider');
  const checks = requireArray(report.checks, 'checks').map((entry, index) => {
    const check = requireRecord(entry, `checks[${index}]`);
    const status = requireString(check.status, `checks[${index}].status`);
    if (!['ok', 'warning', 'error', 'unavailable'].includes(status)) {
      incompatible(`Unknown doctor status ${status}.`);
    }
    requireString(check.id, `checks[${index}].id`);
    requireBoolean(check.required, `checks[${index}].required`);
    requireString(check.summary, `checks[${index}].summary`);
    return check as DoctorCheck;
  });
  return { ...report, checks } as DoctorReport;
}

function contextMode(value: unknown, field: string): ContextMode {
  const mode = requireString(value, field);
  if (!['legacy', 'symbol', 'compare'].includes(mode)) {
    incompatible(`Unknown context mode ${mode}.`);
  }
  return mode as ContextMode;
}

function optionalInteger(value: unknown, field: string): number | undefined {
  return value === undefined || value === null ? undefined : requireInteger(value, field);
}

function completionSummary(value: unknown): AssistantCompletionSummary {
  const summary = requireRecord(value, 'summary');
  const command = requireString(summary.command, 'summary.command');
  if (command !== 'ask' && command !== 'plan') {
    incompatible(`Unknown assistant command ${command}.`);
  }
  const files = requireArray(summary.context_files, 'summary.context_files').map(
    (entry, index): AssistantCompletionContextFile => {
      const file = requireRecord(entry, `summary.context_files[${index}]`);
      return {
        context_mode: contextMode(file.context_mode, `summary.context_files[${index}].context_mode`),
        path: requireString(file.path, `summary.context_files[${index}].path`),
        snippets: requireInteger(file.snippets, `summary.context_files[${index}].snippets`),
        max_score: optionalInteger(file.max_score, `summary.context_files[${index}].max_score`),
      };
    },
  );
  const runs = requireArray(summary.runs, 'summary.runs').map(
    (entry, index): AssistantCompletionRun => {
      const run = requireRecord(entry, `summary.runs[${index}]`);
      return {
        context_mode: contextMode(run.context_mode, `summary.runs[${index}].context_mode`),
        generated: requireBoolean(run.generated, `summary.runs[${index}].generated`),
        estimated_prompt_tokens: requireInteger(
          run.estimated_prompt_tokens,
          `summary.runs[${index}].estimated_prompt_tokens`,
        ),
        client_ms: optionalInteger(run.client_ms, `summary.runs[${index}].client_ms`),
        prompt_tokens: optionalInteger(run.prompt_tokens, `summary.runs[${index}].prompt_tokens`),
        generated_tokens: optionalInteger(
          run.generated_tokens,
          `summary.runs[${index}].generated_tokens`,
        ),
        generated_tokens_per_second:
          run.generated_tokens_per_second === undefined || run.generated_tokens_per_second === null
            ? undefined
            : requireNumber(
                run.generated_tokens_per_second,
                `summary.runs[${index}].generated_tokens_per_second`,
              ),
      };
    },
  );
  const used = summary.used_context_mode;
  return {
    command,
    success: requireBoolean(summary.success, 'summary.success'),
    model: requireString(summary.model, 'summary.model'),
    requested_context_mode: contextMode(
      summary.requested_context_mode,
      'summary.requested_context_mode',
    ),
    used_context_mode: used === null || used === undefined ? undefined : contextMode(used, 'summary.used_context_mode'),
    preparation_duration_us: requireInteger(
      summary.preparation_duration_us,
      'summary.preparation_duration_us',
    ),
    warnings: requireStringArray(summary.warnings, 'summary.warnings'),
    context_files: files,
    runs,
  };
}

function generationResult(value: unknown): GenerationResult {
  const result = requireRecord(value, 'event.result');
  if (requireInteger(result.schema_version, 'event.result.schema_version') !== SUPPORTED_SCHEMA_VERSION) {
    incompatible('Unsupported generation result schema.');
  }
  const usage = requireRecord(result.usage, 'event.result.usage');
  const timings = requireRecord(result.timings, 'event.result.timings');
  return {
    schema_version: SUPPORTED_SCHEMA_VERSION,
    request_id: requireString(result.request_id, 'event.result.request_id'),
    provider: requireString(result.provider, 'event.result.provider'),
    model: requireString(result.model, 'event.result.model'),
    output: requireString(result.output, 'event.result.output'),
    finish_reason: requireString(result.finish_reason, 'event.result.finish_reason'),
    prompt_chars: requireInteger(result.prompt_chars, 'event.result.prompt_chars'),
    usage: {
      prompt_tokens: optionalInteger(usage.prompt_tokens, 'event.result.usage.prompt_tokens'),
      generated_tokens: optionalInteger(
        usage.generated_tokens,
        'event.result.usage.generated_tokens',
      ),
    },
    timings: {
      client_ms: requireInteger(timings.client_ms, 'event.result.timings.client_ms'),
      provider_total_ms: optionalInteger(
        timings.provider_total_ms,
        'event.result.timings.provider_total_ms',
      ),
      load_ms: optionalInteger(timings.load_ms, 'event.result.timings.load_ms'),
      prompt_eval_ms: optionalInteger(
        timings.prompt_eval_ms,
        'event.result.timings.prompt_eval_ms',
      ),
      generation_ms: optionalInteger(
        timings.generation_ms,
        'event.result.timings.generation_ms',
      ),
    },
  };
}

function llmEvent(value: unknown): LlmProtocolEvent {
  const event = requireRecord(value, 'event');
  requireProtocol(event, LLM_PROTOCOL);
  const type = requireString(event.type, 'event.type');
  if (!['started', 'delta', 'completed', 'failed', 'cancelled'].includes(type)) {
    incompatible(`Unknown LLM event type ${type}.`);
  }
  const validated: LlmProtocolEvent = {
    ...event,
    schema_version: SUPPORTED_SCHEMA_VERSION,
    protocol: LLM_PROTOCOL,
    request_id: requireString(event.request_id, 'event.request_id'),
    sequence: requireInteger(event.sequence, 'event.sequence'),
    type: type as LlmProtocolEvent['type'],
  };
  if (type === 'delta') {
    validated.text = requireString(event.text, 'event.text');
  } else if (type === 'completed') {
    validated.result = generationResult(event.result);
    if (validated.result.request_id !== validated.request_id) {
      incompatible('Generation result request ID does not match its LLM event.');
    }
  }
  return validated;
}

export function validateAssistantEvent(
  value: unknown,
  expectedRequestId: string,
): AssistantProtocolEvent {
  const event = requireRecord(value, 'assistant_event');
  requireProtocol(event, ASSISTANT_PROTOCOL);
  const requestId = requireString(event.request_id, 'request_id');
  if (requestId !== expectedRequestId) {
    throw new OpticCodeClientError(
      'request_mismatch',
      `Assistant request ID ${requestId} does not match ${expectedRequestId}.`,
    );
  }
  const type = requireString(event.type, 'type');
  if (
    ![
      'started',
      'context_prepared',
      'provider_event',
      'completed',
      'failed',
      'cancelled',
    ].includes(type)
  ) {
    incompatible(`Unknown assistant event type ${type}.`);
  }
  const validated: AssistantProtocolEvent = {
    ...event,
    schema_version: SUPPORTED_SCHEMA_VERSION,
    protocol: ASSISTANT_PROTOCOL,
    request_id: requestId,
    sequence: requireInteger(event.sequence, 'sequence'),
    type: type as AssistantProtocolEvent['type'],
  };
  if (type === 'provider_event') {
    validated.event = llmEvent(event.event);
    validated.context_mode = contextMode(event.context_mode, 'context_mode');
  } else if (type === 'completed' && event.summary !== undefined) {
    validated.summary = completionSummary(event.summary);
  }
  return validated;
}
