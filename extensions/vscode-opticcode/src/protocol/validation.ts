import { OpticCodeClientError } from './errors';
import type {
  AssistantCompletionContextFile,
  AssistantCompletionRun,
  AssistantCompletionSummary,
  AssistantProtocolEvent,
  CapabilitiesReport,
  ChatCommand,
  ChatCompletionSummary,
  ChatContextScope,
  ChatContextFile,
  ChatEditReviewFile,
  ChatMetrics,
  ChatGroundingSummary,
  ChatProtocolEvent,
  ChatRejectedReference,
  ChatResolvedReference,
  ChatSecurityMode,
  ChatTimingReport,
  ContextManifest,
  ComplianceReport,
  ChatTextRange,
  ContextMode,
  DoctorCheck,
  DoctorReport,
  EvidenceValidationReport,
  GenerationResult,
  JsonObject,
  LlmProtocolEvent,
  VersionReport,
} from './types';

export const DISCOVERY_PROTOCOL = 'opticcode.discovery';
export const ASSISTANT_PROTOCOL = 'opticcode.assistant';
export const CHAT_PROTOCOL = 'opticcode.chat';
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
    ['chat', CHAT_PROTOCOL],
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
  for (const required of ['version', 'capabilities', 'doctor', 'ask', 'plan', 'chat']) {
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
  const features = requireRecord(report.features, 'features');
  if (features.chat !== true) {
    incompatible('Required chat capability is disabled.');
  }
  if (features.policy !== undefined) {
    requireBoolean(features.policy, 'features.policy');
  }
  if (report.policy_runtime !== undefined) {
    if (features.policy !== true) {
      incompatible('Policy runtime is advertised without the matching feature capability.');
    }
    const policy = requireRecord(report.policy_runtime, 'policy_runtime');
    requireInteger(policy.schema_version, 'policy_runtime.schema_version');
    requireString(policy.policy_version, 'policy_runtime.policy_version');
    requireBoolean(policy.engine, 'policy_runtime.engine');
    requireStringArray(policy.modes, 'policy_runtime.modes');
    for (const field of ['audit', 'approvals', 'cli', 'chat_read_only', 'chat_write']) {
      requireBoolean(policy[field], `policy_runtime.${field}`);
    }
  } else if (features.policy === true) {
    incompatible('Policy feature is enabled without policy_runtime capabilities.');
  }
  return report as CapabilitiesReport;
}

const CHAT_COMMANDS: readonly ChatCommand[] = [
  'ask',
  'plan',
  'inspect',
  'context',
  'analyze',
  'index',
  'legacy',
  'fix',
  'verify',
  'diff',
  'apply',
  'rollback',
  'status',
  'runs',
  'help',
];

const CHAT_TERMINALS = ['completed', 'failed', 'cancelled'] as const;

function chatCommand(value: unknown, field: string): ChatCommand {
  const command = requireString(value, field);
  if (!CHAT_COMMANDS.includes(command as ChatCommand)) {
    incompatible(`Unknown chat command ${command}.`);
  }
  return command as ChatCommand;
}

function chatSecurityMode(value: unknown, field: string): ChatSecurityMode {
  const mode = requireString(value, field);
  if (!['read_only', 'worktree_edit', 'approved_apply'].includes(mode)) {
    incompatible(`Unknown chat security mode ${mode}.`);
  }
  return mode as ChatSecurityMode;
}

function chatContextScope(value: unknown, field: string): ChatContextScope {
  const scope = requireString(value, field);
  if (!['automatic', 'references_preferred', 'references_only'].includes(scope)) {
    incompatible(`Unknown chat context scope ${scope}.`);
  }
  return scope as ChatContextScope;
}

function chatEvidenceMode(value: unknown, field: string): 'optional' | 'required' {
  const mode = requireString(value, field);
  if (mode !== 'optional' && mode !== 'required') {
    incompatible(`Unknown chat evidence mode ${mode}.`);
  }
  return mode;
}

function groundingRoute(
  value: unknown,
  field: string,
): 'automatic_assistant' | 'reference_llm' | 'document_facts' {
  const route = requireString(value, field);
  if (!['automatic_assistant', 'reference_llm', 'document_facts'].includes(route)) {
    incompatible(`Unknown grounding route ${route}.`);
  }
  return route as 'automatic_assistant' | 'reference_llm' | 'document_facts';
}

function nullableString(value: unknown, field: string): string | null {
  return value === null ? null : requireString(value, field);
}

function nullableNumber(value: unknown, field: string): number | null {
  return value === null ? null : requireNumber(value, field);
}

function chatPosition(value: unknown, field: string): { line: number; character: number } {
  const position = requireRecord(value, field);
  return {
    line: requireInteger(position.line, `${field}.line`),
    character: requireInteger(position.character, `${field}.character`),
  };
}

function chatRange(value: unknown, field: string): ChatTextRange {
  const range = requireRecord(value, field);
  return {
    start: chatPosition(range.start, `${field}.start`),
    end: chatPosition(range.end, `${field}.end`),
  };
}

function nullableRange(value: unknown, field: string): ChatTextRange | null {
  return value === null ? null : chatRange(value, field);
}

function chatResolvedReference(value: unknown, field: string): ChatResolvedReference {
  const reference = requireRecord(value, field);
  const resolved: ChatResolvedReference = {
    reference_id: requireString(reference.reference_id, `${field}.reference_id`),
    kind: requireString(reference.kind, `${field}.kind`),
    path: nullableString(reference.path, `${field}.path`),
    range: nullableRange(reference.range, `${field}.range`),
    inclusion_reason: requireString(reference.inclusion_reason, `${field}.inclusion_reason`),
    provenance: requireString(reference.provenance, `${field}.provenance`),
    bytes: requireInteger(reference.bytes, `${field}.bytes`),
    content_hash: nullableString(reference.content_hash, `${field}.content_hash`),
  };
  if (reference.origin !== undefined) {
    resolved.origin = requireString(reference.origin, `${field}.origin`);
  }
  if (reference.resolution !== undefined) {
    resolved.resolution = requireString(reference.resolution, `${field}.resolution`);
  }
  if (reference.security_decision !== undefined) {
    resolved.security_decision = requireString(
      reference.security_decision,
      `${field}.security_decision`,
    );
  }
  if (reference.injection !== undefined) {
    resolved.injection = requireString(reference.injection, `${field}.injection`);
  }
  if (reference.bytes_injected !== undefined) {
    resolved.bytes_injected = requireInteger(
      reference.bytes_injected,
      `${field}.bytes_injected`,
    );
  }
  if (reference.reason !== undefined) {
    resolved.reason = requireString(reference.reason, `${field}.reason`);
  }
  if (reference.full_content_hash !== undefined) {
    resolved.full_content_hash = nullableString(
      reference.full_content_hash,
      `${field}.full_content_hash`,
    );
  }
  return resolved;
}

function chatRejectedReference(value: unknown, field: string): ChatRejectedReference {
  const reference = requireRecord(value, field);
  const rejected: ChatRejectedReference = {
    reference_id: requireString(reference.reference_id, `${field}.reference_id`),
    kind: requireString(reference.kind, `${field}.kind`),
    rule_id: requireString(reference.rule_id, `${field}.rule_id`),
    reason: requireString(reference.reason, `${field}.reason`),
  };
  if (reference.path !== undefined) {
    rejected.path = nullableString(reference.path, `${field}.path`);
  }
  if (reference.origin !== undefined) {
    rejected.origin = requireString(reference.origin, `${field}.origin`);
  }
  if (reference.injection !== undefined) {
    rejected.injection = requireString(reference.injection, `${field}.injection`);
  }
  if (reference.reason_code !== undefined) {
    rejected.reason_code = requireString(reference.reason_code, `${field}.reason_code`);
  }
  return rejected;
}

function chatContextFile(value: unknown, field: string): ChatContextFile {
  const file = requireRecord(value, field);
  return {
    path: requireString(file.path, `${field}.path`),
    snippets: requireInteger(file.snippets, `${field}.snippets`),
    provenance: requireString(file.provenance, `${field}.provenance`),
  };
}

function chatEditReviewFile(value: unknown, field: string): ChatEditReviewFile {
  const file = requireRecord(value, field);
  const status = requireString(file.status, `${field}.status`);
  const lineEnding = requireString(file.line_ending, `${field}.line_ending`);
  if (status !== 'modified' && status !== 'created') {
    incompatible(`Unknown edit review status ${status}.`);
  }
  if (!['none', 'lf', 'crlf'].includes(lineEnding)) {
    incompatible(`Unknown edit review line ending ${lineEnding}.`);
  }
  const baseContent = file.base_content;
  const baseHash = file.base_hash;
  const proposedContent = requireString(file.proposed_content, `${field}.proposed_content`);
  if (
    proposedContent.length > 512 * 1024 ||
    (baseContent !== undefined && requireString(baseContent, `${field}.base_content`).length > 512 * 1024)
  ) {
    incompatible('Edit review snapshot exceeds its per-file bound.');
  }
  return {
    path: requireString(file.path, `${field}.path`),
    status,
    line_ending: lineEnding as ChatEditReviewFile['line_ending'],
    ...(baseContent === undefined
      ? {}
      : { base_content: requireString(baseContent, `${field}.base_content`) }),
    ...(baseHash === undefined
      ? {}
      : { base_hash: requireString(baseHash, `${field}.base_hash`) }),
    proposed_content: proposedContent,
    proposed_hash: requireString(file.proposed_hash, `${field}.proposed_hash`),
    proposed_bytes: requireInteger(file.proposed_bytes, `${field}.proposed_bytes`),
    additions: requireInteger(file.additions, `${field}.additions`),
    deletions: requireInteger(file.deletions, `${field}.deletions`),
    hunks: requireInteger(file.hunks, `${field}.hunks`),
  };
}

function chatMetrics(value: unknown, field: string): ChatMetrics {
  const metrics = requireRecord(value, field);
  const validated: ChatMetrics = {
    preparation_ms: requireInteger(metrics.preparation_ms, `${field}.preparation_ms`),
    total_ms: requireInteger(metrics.total_ms, `${field}.total_ms`),
    estimated_prompt_tokens: requireInteger(
      metrics.estimated_prompt_tokens,
      `${field}.estimated_prompt_tokens`,
    ),
    prompt_tokens: nullableNumber(metrics.prompt_tokens, `${field}.prompt_tokens`),
    generated_tokens: nullableNumber(metrics.generated_tokens, `${field}.generated_tokens`),
    generated_tokens_per_second: nullableNumber(
      metrics.generated_tokens_per_second,
      `${field}.generated_tokens_per_second`,
    ),
  };
  if (metrics.timing !== undefined && metrics.timing !== null) {
    validated.timing = chatTimingReport(metrics.timing, `${field}.timing`);
  }
  if (metrics.route !== undefined) {
    validated.route = requireString(metrics.route, `${field}.route`);
  }
  return validated;
}

function chatTimingReport(value: unknown, field: string): ChatTimingReport {
  const timing = requireRecord(value, field);
  const phases = requireArray(timing.phases, `${field}.phases`);
  if (phases.length > 128) {
    incompatible('Chat timing report exceeds its phase limit.');
  }
  return {
    schema_version: requireInteger(timing.schema_version, `${field}.schema_version`),
    ...(timing.request_id === undefined
      ? {}
      : { request_id: requireString(timing.request_id, `${field}.request_id`) }),
    ...(timing.run_id === undefined
      ? {}
      : { run_id: requireString(timing.run_id, `${field}.run_id`) }),
    ...(timing.workspace_id === undefined
      ? {}
      : { workspace_id: requireString(timing.workspace_id, `${field}.workspace_id`) }),
    ...(timing.command === undefined
      ? {}
      : { command: requireString(timing.command, `${field}.command`) }),
    unit: requireString(timing.unit, `${field}.unit`),
    clock: requireString(timing.clock, `${field}.clock`),
    phases: phases.map((phase, index) => {
      const entry = requireRecord(phase, `${field}.phases[${index}]`);
      return {
        name: requireString(entry.name, `${field}.phases[${index}].name`),
        duration_ms: requireInteger(
          entry.duration_ms,
          `${field}.phases[${index}].duration_ms`,
        ),
        measured_by: requireString(
          entry.measured_by,
          `${field}.phases[${index}].measured_by`,
        ),
        includes: requireStringArray(entry.includes, `${field}.phases[${index}].includes`),
      };
    }),
  };
}

function contextManifest(value: unknown, field: string): ContextManifest {
  const manifest = requireRecord(value, field);
  const entries = requireArray(manifest.entries, `${field}.entries`);
  if (entries.length > 64) {
    incompatible('Context manifest exceeds its entry limit.');
  }
  const parsedEntries = entries.map((value, entryIndex) => {
    const entryField = `${field}.entries[${entryIndex}]`;
    const entry = requireRecord(value, entryField);
    const ranges = requireArray(entry.ranges, `${entryField}.ranges`);
    if (ranges.length > 128) {
      incompatible('Context manifest entry exceeds its range limit.');
    }
    return {
      reference_id: requireString(entry.reference_id, `${entryField}.reference_id`),
      path: requireString(entry.path, `${entryField}.path`),
      origin: requireString(entry.origin, `${entryField}.origin`),
      hash: requireString(entry.hash, `${entryField}.hash`),
      injected_hash: requireString(entry.injected_hash, `${entryField}.injected_hash`),
      size_bytes: requireInteger(entry.size_bytes, `${entryField}.size_bytes`),
      encoding: requireString(entry.encoding, `${entryField}.encoding`),
      line_ending: requireString(entry.line_ending, `${entryField}.line_ending`),
      ranges: ranges.map((value, rangeIndex) => {
        const rangeField = `${entryField}.ranges[${rangeIndex}]`;
        const range = requireRecord(value, rangeField);
        return {
          start_line: requireInteger(range.start_line, `${rangeField}.start_line`),
          end_line: requireInteger(range.end_line, `${rangeField}.end_line`),
          start_byte: requireInteger(range.start_byte, `${rangeField}.start_byte`),
          end_byte: requireInteger(range.end_byte, `${rangeField}.end_byte`),
        };
      }),
      bytes_injected: requireInteger(entry.bytes_injected, `${entryField}.bytes_injected`),
      reason: requireString(entry.reason, `${entryField}.reason`),
      git_state: requireString(entry.git_state, `${entryField}.git_state`),
      workspace_id: requireString(entry.workspace_id, `${entryField}.workspace_id`),
    };
  });
  return {
    schema_version: requireInteger(manifest.schema_version, `${field}.schema_version`),
    context_scope: chatContextScope(manifest.context_scope, `${field}.context_scope`),
    workspace_id: requireString(manifest.workspace_id, `${field}.workspace_id`),
    request_id: requireString(manifest.request_id, `${field}.request_id`),
    prompt_version: requireString(manifest.prompt_version, `${field}.prompt_version`),
    profile: requireString(manifest.profile, `${field}.profile`),
    entries: parsedEntries,
    total_bytes: requireInteger(manifest.total_bytes, `${field}.total_bytes`),
    estimated_tokens: requireInteger(
      manifest.estimated_tokens,
      `${field}.estimated_tokens`,
    ),
    fingerprint: requireString(manifest.fingerprint, `${field}.fingerprint`),
  };
}

function evidenceReport(value: unknown, field: string): EvidenceValidationReport {
  const report = requireRecord(value, field);
  return {
    valid: requireBoolean(report.valid, `${field}.valid`),
    claims_checked: requireInteger(report.claims_checked, `${field}.claims_checked`),
    citations_checked: requireInteger(
      report.citations_checked,
      `${field}.citations_checked`,
    ),
    errors: requireStringArray(report.errors, `${field}.errors`),
  };
}

function complianceReport(value: unknown, field: string): ComplianceReport {
  const report = requireRecord(value, field);
  return {
    compliant: requireBoolean(report.compliant, `${field}.compliant`),
    internal_context_leak: requireBoolean(
      report.internal_context_leak,
      `${field}.internal_context_leak`,
    ),
    cross_file_leak: requireBoolean(report.cross_file_leak, `${field}.cross_file_leak`),
    task_format_violation: requireBoolean(
      report.task_format_violation,
      `${field}.task_format_violation`,
    ),
    errors: requireStringArray(report.errors, `${field}.errors`),
  };
}

function groundedResponse(value: unknown, field: string): void {
  const response = requireRecord(value, field);
  requireInteger(response.schema_version, `${field}.schema_version`);
  requireString(response.answer, `${field}.answer`);
  requireStringArray(response.missing_information, `${field}.missing_information`);
  requireBoolean(response.used_general_knowledge, `${field}.used_general_knowledge`);
  const claims = requireArray(response.claims, `${field}.claims`);
  if (claims.length > 128) {
    incompatible('Grounded response exceeds its claim limit.');
  }
  for (const [claimIndex, value] of claims.entries()) {
    const claimField = `${field}.claims[${claimIndex}]`;
    const claim = requireRecord(value, claimField);
    requireString(claim.claim_id, `${claimField}.claim_id`);
    requireString(claim.text, `${claimField}.text`);
    const classification = requireString(claim.classification, `${claimField}.classification`);
    if (!['observed', 'inferred', 'general_knowledge', 'insufficient_evidence'].includes(classification)) {
      incompatible(`Unknown evidence classification ${classification}.`);
    }
    const citations = requireArray(claim.evidence, `${claimField}.evidence`);
    if (citations.length > 32) {
      incompatible('Grounded claim exceeds its citation limit.');
    }
    for (const [citationIndex, value] of citations.entries()) {
      const citationField = `${claimField}.evidence[${citationIndex}]`;
      const citation = requireRecord(value, citationField);
      requireString(citation.path, `${citationField}.path`);
      requireInteger(citation.start_line, `${citationField}.start_line`);
      requireInteger(citation.end_line, `${citationField}.end_line`);
      requireString(citation.content_hash, `${citationField}.content_hash`);
    }
  }
}

function chatGroundingSummary(value: unknown, field: string): ChatGroundingSummary {
  const grounding = requireRecord(value, field);
  const reason = requireString(grounding.scope_reason, `${field}.scope_reason`);
  if (!['explicit_setting', 'explicit_prompt_restriction', 'default_setting', 'server_downgrade'].includes(reason)) {
    incompatible(`Unknown scope reason ${reason}.`);
  }
  if (grounding.response !== undefined && grounding.response !== null) {
    groundedResponse(grounding.response, `${field}.response`);
  }
  const evidence =
    grounding.evidence === undefined || grounding.evidence === null
      ? undefined
      : evidenceReport(grounding.evidence, `${field}.evidence`);
  const compliance =
    grounding.compliance === undefined || grounding.compliance === null
      ? undefined
      : complianceReport(grounding.compliance, `${field}.compliance`);
  return {
    schema_version: requireInteger(grounding.schema_version, `${field}.schema_version`),
    route: groundingRoute(grounding.route, `${field}.route`),
    requested_scope: chatContextScope(grounding.requested_scope, `${field}.requested_scope`),
    effective_scope: chatContextScope(grounding.effective_scope, `${field}.effective_scope`),
    scope_reason: reason as ChatGroundingSummary['scope_reason'],
    evidence_mode: chatEvidenceMode(grounding.evidence_mode, `${field}.evidence_mode`),
    selected_references: requireInteger(
      grounding.selected_references,
      `${field}.selected_references`,
    ),
    resolved_references: requireInteger(
      grounding.resolved_references,
      `${field}.resolved_references`,
    ),
    injected_references: requireInteger(
      grounding.injected_references,
      `${field}.injected_references`,
    ),
    refused_references: requireInteger(
      grounding.refused_references,
      `${field}.refused_references`,
    ),
    discovered_files: requireInteger(grounding.discovered_files, `${field}.discovered_files`),
    rag_hits: requireInteger(grounding.rag_hits, `${field}.rag_hits`),
    historical_turns: requireInteger(
      grounding.historical_turns,
      `${field}.historical_turns`,
    ),
    prompt_fingerprint: requireString(
      grounding.prompt_fingerprint,
      `${field}.prompt_fingerprint`,
    ),
    manifest: contextManifest(grounding.manifest, `${field}.manifest`),
    ...(grounding.response === undefined || grounding.response === null
      ? {}
      : { response: grounding.response as ChatGroundingSummary['response'] }),
    ...(evidence === undefined ? {} : { evidence }),
    ...(compliance === undefined ? {} : { compliance }),
  };
}

function chatCompletionSummary(value: unknown): ChatCompletionSummary {
  const summary = requireRecord(value, 'summary');
  const references = requireArray(summary.references, 'summary.references');
  const files = requireArray(summary.context_files, 'summary.context_files');
  if (references.length > 64 || files.length > 4096) {
    incompatible('Chat completion summary exceeds its collection limits.');
  }
  const used = summary.used_context_mode;
  const validated: ChatCompletionSummary = {
    command: chatCommand(summary.command, 'summary.command'),
    success: requireBoolean(summary.success, 'summary.success'),
    model: requireString(summary.model, 'summary.model'),
    requested_context_mode: contextMode(
      summary.requested_context_mode,
      'summary.requested_context_mode',
    ),
    used_context_mode: used === null ? null : contextMode(used, 'summary.used_context_mode'),
    references: references.map((entry, index) =>
      chatResolvedReference(entry, `summary.references[${index}]`),
    ),
    rejected_references: requireInteger(
      summary.rejected_references,
      'summary.rejected_references',
    ),
    context_files: files.map((entry, index) =>
      chatContextFile(entry, `summary.context_files[${index}]`),
    ),
    warnings: requireStringArray(summary.warnings, 'summary.warnings'),
    metrics: chatMetrics(summary.metrics, 'summary.metrics'),
    repository_state: requireString(summary.repository_state, 'summary.repository_state'),
    run_id: requireString(summary.run_id, 'summary.run_id'),
  };
  if (summary.grounding !== undefined && summary.grounding !== null) {
    validated.grounding = chatGroundingSummary(summary.grounding, 'summary.grounding');
  }
  return validated;
}

export function validateChatEvent(
  value: unknown,
  expectedRequestId: string,
): ChatProtocolEvent {
  const event = requireRecord(value, 'chat_event');
  requireProtocol(event, CHAT_PROTOCOL);
  const requestId = requireString(event.request_id, 'request_id');
  if (requestId !== expectedRequestId) {
    throw new OpticCodeClientError(
      'request_mismatch',
      `Chat request ID ${requestId} does not match ${expectedRequestId}.`,
    );
  }
  requireInteger(event.sequence, 'sequence');
  requireInteger(event.elapsed_ms, 'elapsed_ms');
  const type = requireString(event.type, 'type');
  switch (type) {
    case 'request_accepted':
      chatCommand(event.command, 'command');
      chatSecurityMode(event.security_mode, 'security_mode');
      if (event.requested_security_mode !== undefined) {
        chatSecurityMode(event.requested_security_mode, 'requested_security_mode');
      }
      if (event.effective_security_mode !== undefined) {
        chatSecurityMode(event.effective_security_mode, 'effective_security_mode');
      }
      if (
        event.policy_version !== undefined ||
        event.policy_decision !== undefined ||
        event.policy_rule_id !== undefined
      ) {
        requireString(event.policy_version, 'policy_version');
        if (!['allow', 'require_approval', 'deny'].includes(requireString(event.policy_decision, 'policy_decision'))) {
          incompatible('Unknown policy decision in chat acceptance event.');
        }
        requireString(event.policy_rule_id, 'policy_rule_id');
      }
      break;
    case 'references_resolving':
      requireInteger(event.count, 'count');
      break;
    case 'references_resolved': {
      const accepted = requireArray(event.accepted, 'accepted');
      const rejected = requireArray(event.rejected, 'rejected');
      if (accepted.length > 64 || rejected.length > 64) {
        incompatible('Chat reference event exceeds its bounded collection size.');
      }
      accepted.forEach((entry, index) => chatResolvedReference(entry, `accepted[${index}]`));
      rejected.forEach((entry, index) => chatRejectedReference(entry, `rejected[${index}]`));
      break;
    }
    case 'reference_selected':
      requireString(event.reference_id, 'reference_id');
      requireString(event.kind, 'kind');
      if (event.path !== undefined && event.path !== null) {
        requireString(event.path, 'path');
      }
      requireString(event.origin, 'origin');
      break;
    case 'reference_resolved':
    case 'reference_injected':
      chatResolvedReference(event.reference, 'reference');
      break;
    case 'reference_refused':
      chatRejectedReference(event.reference, 'reference');
      break;
    case 'context_manifest_ready': {
      const manifest = contextManifest(event.manifest, 'manifest');
      if (manifest.request_id !== expectedRequestId) {
        incompatible('Context manifest request ID does not match the active request.');
      }
      requireString(event.prompt_fingerprint, 'prompt_fingerprint');
      break;
    }
    case 'context_started':
      contextMode(event.requested_mode, 'requested_mode');
      break;
    case 'context_ready': {
      contextMode(event.requested_mode, 'requested_mode');
      if (event.used_mode !== null) {
        contextMode(event.used_mode, 'used_mode');
      }
      requireBoolean(event.analysis_complete, 'analysis_complete');
      requireInteger(event.estimated_tokens, 'estimated_tokens');
      const files = requireArray(event.files, 'files');
      if (files.length > 4096) {
        incompatible('Chat context file list exceeds its bounded size.');
      }
      files.forEach((entry, index) => chatContextFile(entry, `files[${index}]`));
      break;
    }
    case 'retrieval_progress':
      requireInteger(event.query_count, 'query_count');
      requireInteger(event.hit_count, 'hit_count');
      break;
    case 'provider_started':
      if (requireString(event.provider, 'provider') !== 'ollama') {
        incompatible('Unsupported chat provider.');
      }
      requireString(event.model, 'model');
      contextMode(event.context_mode, 'context_mode');
      break;
    case 'token_delta':
      requireString(event.text, 'text');
      break;
    case 'finding':
      requireString(event.finding_id, 'finding_id');
      requireString(event.severity, 'severity');
      requireString(event.message, 'message');
      requireString(event.path, 'path');
      if (event.range !== undefined && event.range !== null) {
        chatRange(event.range, 'range');
      }
      break;
    case 'warning':
      requireString(event.code, 'code');
      requireString(event.message, 'message');
      break;
    case 'metrics':
    case 'timing_metrics': {
      const metrics = chatMetrics(event.metrics, 'metrics');
      if (
        metrics.timing?.request_id !== undefined &&
        metrics.timing.request_id !== expectedRequestId
      ) {
        incompatible('Chat timing request ID does not match the active request.');
      }
      break;
    }
    case 'grounding_validation_started':
      groundingRoute(event.route, 'route');
      chatEvidenceMode(event.evidence_mode, 'evidence_mode');
      break;
    case 'grounding_validation_completed':
      evidenceReport(event.evidence, 'evidence');
      complianceReport(event.compliance, 'compliance');
      break;
    case 'task_compliance_failed':
      if (requireStringArray(event.errors, 'errors').length > 128) {
        incompatible('Task compliance diagnostics exceed their bounded count.');
      }
      break;
    case 'internal_context_leak_detected':
      if (requireStringArray(event.markers, 'markers').length > 128) {
        incompatible('Internal leak diagnostics exceed their bounded count.');
      }
      break;
    case 'document_inspection_completed':
      requireString(event.format, 'format');
      requireInteger(event.facts, 'facts');
      requireInteger(event.model_calls, 'model_calls');
      break;
    case 'edit_plan_started':
      requireString(event.plan_id, 'plan_id');
      break;
    case 'edit_plan_ready':
      requireString(event.plan_id, 'plan_id');
      requireString(event.summary, 'summary');
      requireInteger(event.file_count, 'file_count');
      break;
    case 'policy_decision':
      if (event.proposal_id !== undefined) {
        requireString(event.proposal_id, 'proposal_id');
      }
      requireString(event.stage, 'stage');
      requireString(event.action_kind, 'action_kind');
      requireString(event.decision, 'decision');
      requireString(event.rule_id, 'rule_id');
      if (event.audit_event_id !== undefined) {
        requireString(event.audit_event_id, 'audit_event_id');
      }
      break;
    case 'proposal_stored':
      requireString(event.proposal_id, 'proposal_id');
      requireString(event.state, 'state');
      requireInteger(event.expires_at_unix_ms, 'expires_at_unix_ms');
      break;
    case 'verification_started':
      requireString(event.proposal_id, 'proposal_id');
      break;
    case 'worktree_created':
      requireString(event.proposal_id, 'proposal_id');
      requireString(event.run_id, 'run_id');
      break;
    case 'edit_applied_in_worktree':
      requireString(event.proposal_id, 'proposal_id');
      requireBoolean(event.success, 'success');
      break;
    case 'build_started':
      requireString(event.proposal_id, 'proposal_id');
      requireBoolean(event.offline, 'offline');
      break;
    case 'build_completed':
      requireString(event.proposal_id, 'proposal_id');
      requireBoolean(event.success, 'success');
      requireString(event.build, 'build');
      requireString(event.tests, 'tests');
      break;
    case 'verification_completed':
      requireString(event.proposal_id, 'proposal_id');
      requireBoolean(event.success, 'success');
      requireString(event.build, 'build');
      requireString(event.tests, 'tests');
      break;
    case 'diff_ready':
      requireString(event.proposal_id, 'proposal_id');
      requireInteger(event.files, 'files');
      requireInteger(event.additions, 'additions');
      requireInteger(event.deletions, 'deletions');
      if (
        event.display_patch !== undefined ||
        event.display_truncated !== undefined ||
        event.changes !== undefined
      ) {
        const displayPatch = requireString(event.display_patch, 'display_patch');
        requireBoolean(event.display_truncated, 'display_truncated');
        const changes = requireArray(event.changes, 'changes');
        if (displayPatch.length > 1024 * 1024 || changes.length > 5) {
          incompatible('Chat diff review event exceeds its bounded payload limits.');
        }
        let snapshotCharacters = 0;
        for (const [index, change] of changes.entries()) {
          const parsed = chatEditReviewFile(change, `changes[${index}]`);
          snapshotCharacters +=
            parsed.proposed_content.length + (parsed.base_content?.length ?? 0);
        }
        if (snapshotCharacters > 4 * 1024 * 1024) {
          incompatible('Chat diff review snapshots exceed their aggregate bound.');
        }
      }
      break;
    case 'approval_required':
      requireString(event.proposal_id, 'proposal_id');
      requireString(event.approval_request_id, 'approval_request_id');
      if (event.operation !== undefined && !['apply', 'rollback'].includes(requireString(event.operation, 'operation'))) {
        incompatible('Unknown approval operation.');
      }
      requireString(event.summary, 'summary');
      break;
    case 'apply_started':
      requireString(event.proposal_id, 'proposal_id');
      requireString(event.transaction_id, 'transaction_id');
      break;
    case 'apply_completed':
      requireString(event.proposal_id, 'proposal_id');
      requireString(event.transaction_id, 'transaction_id');
      requireBoolean(event.success, 'success');
      break;
    case 'rollback_available':
      if (event.proposal_id !== undefined) {
        requireString(event.proposal_id, 'proposal_id');
      }
      requireString(event.transaction_id, 'transaction_id');
      break;
    case 'rollback_started':
      requireString(event.proposal_id, 'proposal_id');
      requireString(event.transaction_id, 'transaction_id');
      break;
    case 'rollback_completed':
      requireString(event.proposal_id, 'proposal_id');
      requireString(event.transaction_id, 'transaction_id');
      requireBoolean(event.success, 'success');
      requireBoolean(event.already_rolled_back, 'already_rolled_back');
      break;
    case 'proposal_discarded':
      requireString(event.proposal_id, 'proposal_id');
      break;
    case 'completed': {
      const summary = chatCompletionSummary(event.summary);
      if (
        summary.metrics.timing?.request_id !== undefined &&
        summary.metrics.timing.request_id !== expectedRequestId
      ) {
        incompatible('Completed timing belongs to another request.');
      }
      if (
        summary.grounding !== undefined &&
        summary.grounding !== null &&
        summary.grounding.manifest.request_id !== expectedRequestId
      ) {
        incompatible('Completed grounding manifest belongs to another request.');
      }
      break;
    }
    case 'cancelled':
      requireString(event.reason, 'reason');
      break;
    case 'failed': {
      const error = requireRecord(event.error, 'error');
      requireString(error.code, 'error.code');
      requireString(error.stage, 'error.stage');
      requireString(error.message, 'error.message');
      requireBoolean(error.retriable, 'error.retriable');
      break;
    }
    default:
      incompatible(`Unknown chat event type ${type}.`);
  }
  if (CHAT_TERMINALS.includes(type as (typeof CHAT_TERMINALS)[number])) {
    // The shared stream parser enforces terminal uniqueness and ordering.
  }
  return event as ChatProtocolEvent;
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
