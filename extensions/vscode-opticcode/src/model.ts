import * as path from 'node:path';

import type {
  AssistantStreamResult,
  ContextMode,
  DoctorReport,
  JsonObject,
} from './protocol/types';
import { isRecord } from './protocol/validation';

export type FindingKind = 'safe_fix' | 'ambiguity' | 'information' | 'refused' | 'build_error';
export type FindingDecision = 'accepted' | 'refused' | 'informational';

export interface TextPoint {
  line: number;
  // Tree-sitter columns are UTF-8 byte offsets until the VS Code boundary.
  character: number;
}

export interface TextRange {
  start: TextPoint;
  end: TextPoint;
}

export interface Finding {
  id: string;
  kind: FindingKind;
  file: string;
  range: TextRange;
  message: string;
  symbol?: string | undefined;
  rule?: string | undefined;
  confidence?: string | undefined;
  decision: FindingDecision;
  reason: string;
  verification?: string | undefined;
}

export type RunStatus = 'running' | 'completed' | 'failed' | 'cancelled' | 'interrupted';

export interface RunRecord {
  command: string;
  requestId: string;
  status: RunStatus;
  durationMs: number;
  context?: ContextMode | undefined;
  promptTokens?: number | undefined;
  generatedTokens?: number | undefined;
  model?: string | undefined;
  build?: string | undefined;
  worktree?: string | undefined;
  reportPath?: string | undefined;
}

export interface StatusSnapshot {
  executablePath?: string | undefined;
  executableSource?: string | undefined;
  opticcodeVersion?: string | undefined;
  protocolCompatible: boolean;
  doctor?: DoctorReport | undefined;
  error?: string | undefined;
}

function integer(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0
    ? value
    : undefined;
}

function string(value: unknown): string | undefined {
  return typeof value === 'string' ? value : undefined;
}

export function utf16ColumnFromUtf8(line: string, byteColumn: number): number {
  let utf8Bytes = 0;
  let utf16Units = 0;
  for (const scalar of line) {
    const scalarBytes = Buffer.byteLength(scalar, 'utf8');
    if (utf8Bytes + scalarBytes > byteColumn) {
      break;
    }
    utf8Bytes += scalarBytes;
    utf16Units += scalar.length;
  }
  return utf16Units;
}

function sourceRange(value: unknown): TextRange {
  const range = isRecord(value) ? value : {};
  const start = isRecord(range.start) ? range.start : {};
  const end = isRecord(range.end) ? range.end : {};
  const startLine = integer(start.row) ?? 0;
  const startCharacter = integer(start.column) ?? 0;
  let endLine = integer(end.row) ?? startLine;
  let endCharacter = integer(end.column) ?? startCharacter + 1;
  if (endLine < startLine || (endLine === startLine && endCharacter <= startCharacter)) {
    endLine = startLine;
    endCharacter = startCharacter + 1;
  }
  return {
    start: { line: startLine, character: startCharacter },
    end: { line: endLine, character: endCharacter },
  };
}

function absoluteFile(root: string, file: string): string {
  return path.isAbsolute(file) ? path.normalize(file) : path.resolve(root, file);
}

export function findingsFromJavaSyntax(report: JsonObject, workspace: string): Finding[] {
  const root = string(report.root) ?? workspace;
  const files = Array.isArray(report.files) ? report.files : [];
  const findings: Finding[] = [];
  for (const [fileIndex, candidate] of files.entries()) {
    if (!isRecord(candidate)) {
      continue;
    }
    const file = string(candidate.path);
    if (file === undefined) {
      continue;
    }
    const diagnostics = Array.isArray(candidate.diagnostics) ? candidate.diagnostics : [];
    for (const [diagnosticIndex, diagnosticCandidate] of diagnostics.entries()) {
      if (!isRecord(diagnosticCandidate)) {
        continue;
      }
      const kind = string(diagnosticCandidate.kind) ?? 'syntax_error';
      const message = string(diagnosticCandidate.message) ?? 'Invalid Java syntax.';
      findings.push({
        id: `syntax:${fileIndex}:${diagnosticIndex}`,
        kind: 'build_error',
        file: absoluteFile(root, file),
        range: sourceRange(diagnosticCandidate.range),
        message,
        rule: kind,
        decision: 'refused',
        reason: string(diagnosticCandidate.node_kind) ?? kind,
      });
    }
  }
  return findings;
}

export function findingsFromJavaEdits(report: JsonObject, workspace: string): Finding[] {
  const root = string(report.root) ?? workspace;
  const findings: Finding[] = [];
  const proposals = Array.isArray(report.proposals) ? report.proposals : [];
  for (const [index, candidate] of proposals.entries()) {
    if (!isRecord(candidate)) {
      continue;
    }
    const file = string(candidate.file);
    if (file === undefined) {
      continue;
    }
    const expected = string(candidate.expected_content) ?? 'legacy symbol';
    const replacement = string(candidate.replacement) ?? 'compatible symbol';
    findings.push({
      id: string(candidate.id) ?? `proposal:${index}`,
      kind: 'safe_fix',
      file: absoluteFile(root, file),
      range: sourceRange(candidate.edit_range),
      message: `${expected} -> ${replacement}`,
      symbol: string(candidate.target_id),
      rule: string(candidate.rule_id),
      confidence: string(candidate.confidence),
      decision: 'accepted',
      reason: string(candidate.reason) ?? 'Verified syntax-targeted legacy replacement.',
    });
  }
  const rejections = Array.isArray(report.rejections) ? report.rejections : [];
  for (const [index, candidate] of rejections.entries()) {
    if (!isRecord(candidate)) {
      continue;
    }
    const file = string(candidate.file);
    if (file === undefined) {
      continue;
    }
    findings.push({
      id: `rejection:${index}:${string(candidate.reference_id) ?? 'unknown'}`,
      kind: 'refused',
      file: absoluteFile(root, file),
      range: { start: { line: 0, character: 0 }, end: { line: 0, character: 1 } },
      message: string(candidate.message) ?? 'Legacy edit refused.',
      rule: string(candidate.rule_id),
      decision: 'refused',
      reason: string(candidate.kind) ?? 'controlled_refusal',
    });
  }
  return findings;
}

export function findingsFromJavaContext(report: JsonObject, workspace: string): Finding[] {
  const root = string(report.root) ?? workspace;
  const snippets = Array.isArray(report.snippets) ? report.snippets : [];
  return snippets.flatMap((candidate, index): Finding[] => {
    if (!isRecord(candidate)) {
      return [];
    }
    const file = string(candidate.file);
    if (file === undefined) {
      return [];
    }
    return [
      {
        id: string(candidate.id) ?? `context:${index}`,
        kind: 'information',
        file: absoluteFile(root, file),
        range: sourceRange(candidate.content_range),
        message: string(candidate.role) ?? 'selected context',
        symbol: string(candidate.symbol_id),
        confidence: integer(candidate.score)?.toString(),
        decision: 'informational',
        reason: Array.isArray(candidate.selection_reasons)
          ? candidate.selection_reasons.filter((entry): entry is string => typeof entry === 'string').join('; ')
          : 'Selected by OpticCode context ranking.',
      },
    ];
  });
}

function inline(value: unknown): string {
  return String(value ?? 'n/a').replaceAll('|', '\\|').replaceAll('`', "'");
}

export function assistantMarkdown(
  title: string,
  result: AssistantStreamResult,
): string {
  const summary = result.summary;
  const generatedRun = summary?.runs.find((run) => run.generated);
  const referencedFiles = [
    ...new Set(summary?.context_files.map((file) => file.path) ?? []),
  ];
  const warnings = summary?.warnings ?? [];
  const response = result.response || result.generation?.output || '*No model output was produced.*';
  const lines = [
    `# ${title}`,
    '',
    response,
    '',
    '## Run',
    '',
    '| Field | Value |',
    '| --- | --- |',
    `| Request | \`${inline(result.requestId)}\` |`,
    `| Status | ${inline(result.status)} |`,
    `| Context | ${inline(summary?.used_context_mode ?? summary?.requested_context_mode)} |`,
    `| Model | \`${inline(summary?.model ?? result.generation?.model)}\` |`,
    `| Estimated prompt tokens | ${inline(generatedRun?.estimated_prompt_tokens)} |`,
    `| Actual prompt tokens | ${inline(result.generation?.usage.prompt_tokens ?? generatedRun?.prompt_tokens)} |`,
    `| Generated tokens | ${inline(result.generation?.usage.generated_tokens ?? generatedRun?.generated_tokens)} |`,
    `| Duration | ${(result.durationMs / 1000).toFixed(3)} s |`,
    '',
    '## Warnings',
    '',
    ...(warnings.length === 0 ? ['- None'] : warnings.map((warning) => `- ${warning}`)),
    '',
    '## Referenced Files',
    '',
    ...(referencedFiles.length === 0
      ? ['- Not exposed by this producer.']
      : referencedFiles.map((file) => `- \`${inline(file)}\``)),
    '',
  ];
  return lines.join('\n');
}

function jsonSection(title: string, value: unknown): string[] {
  return [`## ${title}`, '', '```json', JSON.stringify(value ?? null, null, 2), '```', ''];
}

export function worktreeMarkdown(report: JsonObject): string {
  const worktree = isRecord(report.worktree) ? report.worktree : {};
  return [
    '# OpticCode Worktree Verification',
    '',
    `Overall status: **${inline(report.status)}**`,
    '',
    ...jsonSection('Edit Result', {
      source_analysis: report.source_analysis,
      revalidation: report.revalidation,
      materialization: report.materialization,
      post_write_validation: report.post_write_validation,
    }),
    ...jsonSection('Build Result', worktree.build),
    ...jsonSection('Diff', worktree.diff),
    ...jsonSection('Cleanup Result', {
      cleanup_success: report.cleanup_success,
      lease_recovery_required: report.lease_recovery_required,
      cleanup: worktree.cleanup,
    }),
  ].join('\n');
}
