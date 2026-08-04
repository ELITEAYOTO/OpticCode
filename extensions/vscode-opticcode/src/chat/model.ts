import { createHash, randomUUID } from 'node:crypto';
import * as path from 'node:path';

import type {
  ChatBudgets,
  ChatCommand,
  ChatExpectedProtocols,
  ChatGenerationOptions,
  ChatHistoryTurn,
  ChatProtocolRequest,
  ChatReference,
  ChatSecurityMode,
  ChatTextRange,
  ContextMode,
} from '../protocol/types';

export const CHAT_PARTICIPANT_ID = 'opticcode.chat';
export const CHAT_PROTOCOL = 'opticcode.chat';
export const CHAT_SCHEMA_VERSION = 1;
export const CHAT_COMMANDS: readonly ChatCommand[] = [
  'ask',
  'plan',
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

export const CHAT_BUDGETS: Readonly<ChatBudgets> = {
  max_history_turns: 12,
  max_history_chars: 32 * 1024,
  max_history_tokens: 8 * 1024,
  max_references: 24,
  max_reference_bytes: 1024 * 1024,
  max_prompt_tokens: 32 * 1024,
  rag_hits: 6,
};

export const CHAT_GENERATION: Readonly<ChatGenerationOptions> = {
  max_output_tokens: 1024,
  temperature: null,
  seed: null,
  brief: false,
  compare_generate: false,
};

const MAX_PROMPT_CHARS = 64 * 1024;
const MAX_HISTORY_TURN_CHARS = 8 * 1024;
const MAX_REFERENCE_REASON_CHARS = 512;
const MAX_RECENT_RUNS = 16;

export interface NeutralHistoryTurn {
  role: 'user' | 'assistant';
  content: unknown;
  command?: unknown;
  resultId?: unknown;
}

export interface ReferenceCandidate {
  referenceId: string;
  inclusionReason: string;
  kind: ChatReference['kind'];
  path?: string;
  range?: ChatTextRange;
  symbol?: string;
  findingId?: string;
  runId?: string;
  proposalId?: string;
}

export interface ReferenceCollection {
  accepted: ChatReference[];
  rejected: Array<{ referenceId: string; reason: string }>;
}

export interface ChatRequestInput {
  command: ChatCommand;
  prompt: string;
  workspaceRoot: string;
  profile: string;
  model: string;
  contextMode: ContextMode;
  sessionId: string;
  clientVersion: string;
  vscodeVersion: string;
  locale: string;
  references: readonly ChatReference[];
  history: readonly ChatHistoryTurn[];
  recentRunIds: readonly string[];
  previousRepositoryState?: string | undefined;
  expectedProtocols: ChatExpectedProtocols;
  securityMode?: ChatSecurityMode | undefined;
}

export class ChatRequestBuildError extends Error {
  public constructor(message: string) {
    super(message);
    this.name = 'ChatRequestBuildError';
  }
}

export function parseChatCommand(value: string | undefined): ChatCommand | undefined {
  const normalized = value?.trim().toLocaleLowerCase('en-US');
  if (normalized === undefined || normalized === '') {
    return 'ask';
  }
  return CHAT_COMMANDS.includes(normalized as ChatCommand)
    ? (normalized as ChatCommand)
    : undefined;
}

export function createChatRequestId(command: ChatCommand): string {
  return `vscode-chat-${command}-${randomUUID()}`;
}

export function workspaceIdentity(workspaceRoot: string): string {
  return `workspace-v1-${digest(canonicalWorkspace(workspaceRoot)).slice(0, 32)}`;
}

export function sessionNamespace(
  workspaceRoot: string,
  repositoryState: string | undefined,
  participantSession: string,
): string {
  const material = [
    'opticcode-chat-session-v1',
    canonicalWorkspace(workspaceRoot),
    repositoryState ?? 'repository-state-unknown',
    participantSession,
    String(CHAT_SCHEMA_VERSION),
  ].join('\n');
  return `session-v1-${digest(material).slice(0, 40)}`;
}

export function boundChatHistory(
  candidates: readonly NeutralHistoryTurn[],
  budgets: Readonly<ChatBudgets> = CHAT_BUDGETS,
): ChatHistoryTurn[] {
  const bounded: ChatHistoryTurn[] = [];
  let characters = 0;
  let tokens = 0;
  const tail = candidates.slice(-Math.min(budgets.max_history_turns, 32));
  for (let index = tail.length - 1; index >= 0; index -= 1) {
    const candidate = tail[index];
    if (candidate === undefined || typeof candidate.content !== 'string') {
      continue;
    }
    const content = sanitizeHistoryContent(candidate.content);
    if (content === '') {
      continue;
    }
    const estimatedTokens = estimateTokens(content);
    if (
      characters + content.length > budgets.max_history_chars ||
      tokens + estimatedTokens > budgets.max_history_tokens
    ) {
      continue;
    }
    const command =
      typeof candidate.command === 'string'
        ? parseChatCommand(candidate.command)
        : undefined;
    const resultId = boundedIdentifier(candidate.resultId);
    bounded.unshift({
      role: candidate.role,
      content,
      ...(command === undefined ? {} : { command }),
      ...(resultId === undefined ? {} : { result_id: resultId }),
    });
    characters += content.length;
    tokens += estimatedTokens;
  }
  return bounded;
}

export function collectWorkspaceReferences(
  workspaceRoot: string,
  candidates: readonly ReferenceCandidate[],
  maxReferences = CHAT_BUDGETS.max_references,
): ReferenceCollection {
  const accepted: ChatReference[] = [];
  const rejected: Array<{ referenceId: string; reason: string }> = [];
  const seen = new Set<string>();
  for (const candidate of candidates) {
    if (accepted.length >= Math.min(maxReferences, 64)) {
      rejected.push({
        referenceId: candidate.referenceId,
        reason: 'Reference limit reached.',
      });
      continue;
    }
    try {
      const reference = workspaceReference(workspaceRoot, candidate);
      const key = JSON.stringify(reference);
      if (!seen.has(key)) {
        seen.add(key);
        accepted.push(reference);
      }
    } catch (error) {
      rejected.push({
        referenceId: candidate.referenceId,
        reason: error instanceof Error ? error.message : String(error),
      });
    }
  }
  return { accepted, rejected };
}

export function buildChatRequest(input: ChatRequestInput): ChatProtocolRequest {
  if (input.prompt.length > MAX_PROMPT_CHARS) {
    throw new ChatRequestBuildError(
      `Chat prompt exceeds the ${MAX_PROMPT_CHARS}-character limit.`,
    );
  }
  if (requiresPrompt(input.command) && input.prompt.trim() === '') {
    throw new ChatRequestBuildError(`/${input.command} requires a prompt.`);
  }
  const recentRunIds = input.recentRunIds
    .map((value) => boundedIdentifier(value))
    .filter((value): value is string => value !== undefined)
    .slice(0, MAX_RECENT_RUNS);
  const requestId = createChatRequestId(input.command);
  return {
    schema_version: CHAT_SCHEMA_VERSION,
    protocol: CHAT_PROTOCOL,
    request_id: requestId,
    workspace_id: workspaceIdentity(input.workspaceRoot),
    workspace_root: input.workspaceRoot,
    command: input.command,
    prompt: input.prompt,
    profile: input.profile,
    provider: 'ollama',
    model: input.model,
    context_mode: input.contextMode,
    references: input.references.slice(0, CHAT_BUDGETS.max_references),
    history: input.history.slice(-CHAT_BUDGETS.max_history_turns),
    budgets: { ...CHAT_BUDGETS },
    generation: { ...CHAT_GENERATION },
    security_mode: input.securityMode ?? 'read_only',
    client: {
      name: 'opticcode-vscode',
      version: input.clientVersion,
      vscode_version: input.vscodeVersion,
      session_id: input.sessionId,
      locale: input.locale.slice(0, 64),
      recent_run_ids: recentRunIds,
      previous_repository_state: input.previousRepositoryState ?? null,
    },
    expected_protocols: { ...input.expectedProtocols },
  };
}

function workspaceReference(
  workspaceRoot: string,
  candidate: ReferenceCandidate,
): ChatReference {
  const referenceId = requiredIdentifier(candidate.referenceId, 'reference ID');
  const inclusionReason = candidate.inclusionReason
    .trim()
    .slice(0, MAX_REFERENCE_REASON_CHARS);
  if (inclusionReason === '') {
    throw new ChatRequestBuildError('Reference inclusion reason is empty.');
  }
  if (candidate.kind === 'run') {
    return {
      reference_id: referenceId,
      inclusion_reason: inclusionReason,
      kind: 'run',
      run_id: requiredIdentifier(candidate.runId, 'run ID'),
    };
  }
  if (candidate.kind === 'diff') {
    return {
      reference_id: referenceId,
      inclusion_reason: inclusionReason,
      kind: 'diff',
      proposal_id: requiredIdentifier(candidate.proposalId, 'proposal ID'),
    };
  }
  const relativePath = safeRelativePath(workspaceRoot, candidate.path);
  switch (candidate.kind) {
    case 'file':
      return { reference_id: referenceId, inclusion_reason: inclusionReason, kind: 'file', path: relativePath };
    case 'active_file':
      return {
        reference_id: referenceId,
        inclusion_reason: inclusionReason,
        kind: 'active_file',
        path: relativePath,
      };
    case 'range':
    case 'selection':
      return {
        reference_id: referenceId,
        inclusion_reason: inclusionReason,
        kind: candidate.kind,
        path: relativePath,
        range: validRange(candidate.range),
      };
    case 'symbol':
      return {
        reference_id: referenceId,
        inclusion_reason: inclusionReason,
        kind: 'symbol',
        path: relativePath,
        symbol: requiredIdentifier(candidate.symbol, 'symbol'),
        ...(candidate.range === undefined ? {} : { range: validRange(candidate.range) }),
      };
    case 'finding':
      return {
        reference_id: referenceId,
        inclusion_reason: inclusionReason,
        kind: 'finding',
        finding_id: requiredIdentifier(candidate.findingId, 'finding ID'),
        path: relativePath,
        ...(candidate.range === undefined ? {} : { range: validRange(candidate.range) }),
      };
  }
}

function safeRelativePath(workspaceRoot: string, candidatePath: string | undefined): string {
  if (candidatePath === undefined || candidatePath.trim() === '') {
    throw new ChatRequestBuildError('File reference has no path.');
  }
  const root = path.resolve(workspaceRoot);
  const absolute = path.isAbsolute(candidatePath)
    ? path.resolve(candidatePath)
    : path.resolve(root, candidatePath);
  const relative = path.relative(root, absolute);
  if (relative === '' || relative === '..' || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) {
    throw new ChatRequestBuildError('File reference is outside the selected workspace.');
  }
  return relative.split(path.sep).join('/');
}

function validRange(value: ChatTextRange | undefined): ChatTextRange {
  if (value === undefined) {
    throw new ChatRequestBuildError('Range reference has no range.');
  }
  const { start, end } = value;
  if (
    !Number.isSafeInteger(start.line) ||
    !Number.isSafeInteger(start.character) ||
    !Number.isSafeInteger(end.line) ||
    !Number.isSafeInteger(end.character) ||
    start.line < 0 ||
    start.character < 0 ||
    end.line < start.line ||
    (end.line === start.line && end.character < start.character)
  ) {
    throw new ChatRequestBuildError('Reference range is invalid.');
  }
  return value;
}

function sanitizeHistoryContent(value: string): string {
  let content = value
    .replace(/-----BEGIN [^-]*PRIVATE KEY-----[\s\S]*?-----END [^-]*PRIVATE KEY-----/giu, '[private key redacted]')
    .replace(/\b(api[_-]?key|access[_-]?token|password|secret)\b\s*[:=]\s*[^\s,;]+/giu, '$1=[redacted]')
    .replace(/```(?:diff|json)[\s\S]*?```/giu, (block) =>
      block.length > 2_048 ? '[large prior diff/report omitted]' : block,
    )
    .trim();
  if (content.length > MAX_HISTORY_TURN_CHARS) {
    const omitted = content.length - MAX_HISTORY_TURN_CHARS;
    content = `${content.slice(0, MAX_HISTORY_TURN_CHARS - 64)}\n[${omitted} characters omitted]`;
  }
  return content;
}

function estimateTokens(value: string): number {
  return Math.ceil(value.length / 4);
}

function requiresPrompt(command: ChatCommand): boolean {
  return command === 'ask' || command === 'plan' || command === 'context';
}

function boundedIdentifier(value: unknown): string | undefined {
  if (typeof value !== 'string') {
    return undefined;
  }
  const trimmed = value.trim();
  return /^[a-zA-Z0-9._:-]{1,128}$/.test(trimmed) ? trimmed : undefined;
}

function requiredIdentifier(value: unknown, name: string): string {
  const identifier = boundedIdentifier(value);
  if (identifier === undefined) {
    throw new ChatRequestBuildError(`Invalid ${name}.`);
  }
  return identifier;
}

function canonicalWorkspace(workspaceRoot: string): string {
  const normalized = path.resolve(workspaceRoot);
  return process.platform === 'win32' ? normalized.toLocaleLowerCase('en-US') : normalized;
}

function digest(value: string): string {
  return createHash('sha256').update(value, 'utf8').digest('hex');
}
