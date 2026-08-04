import type {
  ChatCompletionSummary,
  ChatProtocolEvent,
  ChatResolvedReference,
  ChatTextRange,
} from '../protocol/types';

export type ChatRenderOperation =
  | { kind: 'progress'; text: string }
  | { kind: 'markdown'; text: string }
  | { kind: 'reference'; path: string; range?: ChatTextRange | undefined }
  | { kind: 'anchor'; path: string; title: string; range?: ChatTextRange | undefined }
  | { kind: 'filetree'; paths: string[] }
  | { kind: 'button'; command: string; title: string; arguments?: unknown[] | undefined };

export class ChatEventPresenter {
  private readonly acceptedReferences = new Map<string, ChatResolvedReference>();
  private readonly discoveredPaths = new Set<string>();
  private retrievalHits = 0;

  public accept(event: ChatProtocolEvent): ChatRenderOperation[] {
    switch (event.type) {
      case 'request_accepted':
        return [{ kind: 'progress', text: `OpticCode accepted /${event.command}.` }];
      case 'references_resolving':
        return event.count === 0
          ? []
          : [{ kind: 'progress', text: `Resolving ${event.count} reference(s)...` }];
      case 'references_resolved':
        for (const reference of event.accepted) {
          this.acceptedReferences.set(reference.reference_id, reference);
        }
        return [
          ...event.accepted.flatMap((reference): ChatRenderOperation[] =>
            reference.path === null
              ? []
              : [
                  {
                    kind: 'reference',
                    path: reference.path,
                    ...(reference.range === null ? {} : { range: reference.range }),
                  },
                ],
          ),
          ...event.rejected.map(
            (reference): ChatRenderOperation => ({
              kind: 'markdown',
              text: `\n> **Reference refused (${escapeMarkdown(reference.rule_id)}):** ${escapeMarkdown(reference.reason)}\n`,
            }),
          ),
        ];
      case 'context_started':
        return [{ kind: 'progress', text: `Building ${event.requested_mode} context...` }];
      case 'context_ready':
        for (const file of event.files) {
          this.discoveredPaths.add(file.path);
        }
        return [
          {
            kind: 'progress',
            text: `Context ready: ${event.files.length} file(s), about ${event.estimated_tokens} tokens.`,
          },
        ];
      case 'retrieval_progress':
        this.retrievalHits = event.hit_count;
        return [
          {
            kind: 'progress',
            text: `Local RAG: ${event.hit_count} hit(s) from ${event.query_count} query term(s).`,
          },
        ];
      case 'provider_started':
        return [{ kind: 'progress', text: `Generating locally with ${event.model}...` }];
      case 'token_delta':
        return [{ kind: 'markdown', text: event.text }];
      case 'finding':
        return [
          {
            kind: 'markdown',
            text: `\n> **${escapeMarkdown(event.severity)}:** ${escapeMarkdown(event.message)}\n`,
          },
          {
            kind: 'anchor',
            path: event.path,
            title: event.path,
            ...(event.range === undefined || event.range === null ? {} : { range: event.range }),
          },
        ];
      case 'warning':
        return [
          {
            kind: 'markdown',
            text: `\n> **Warning (${escapeMarkdown(event.code)}):** ${escapeMarkdown(event.message)}\n`,
          },
        ];
      case 'metrics':
        return [];
      case 'edit_plan_ready':
        return [
          { kind: 'markdown', text: `\n**Edit plan:** ${escapeMarkdown(event.summary)}\n` },
        ];
      case 'verification_started':
        return [{ kind: 'progress', text: 'Verifying proposal in a disposable worktree...' }];
      case 'verification_completed':
        return [
          {
            kind: 'markdown',
            text: `\n**Verification:** ${event.success ? 'passed' : 'failed'}; build ${escapeMarkdown(event.build)}; tests ${escapeMarkdown(event.tests)}.\n`,
          },
        ];
      case 'diff_ready':
        return [
          {
            kind: 'markdown',
            text: `\n**Verified diff:** ${event.files} file(s), +${event.additions} / -${event.deletions}.\n`,
          },
          {
            kind: 'button',
            command: 'opticcode.internal.chat.showDiff',
            title: 'Show Diff',
            arguments: [event.proposal_id],
          },
        ];
      case 'approval_required':
        return [
          {
            kind: 'button',
            command: 'opticcode.internal.chat.applyProposal',
            title: 'Apply Verified Changes',
            arguments: [event.proposal_id, event.approval_request_id],
          },
        ];
      case 'apply_started':
        return [{ kind: 'progress', text: 'Applying the approved transaction...' }];
      case 'apply_completed':
        return [
          {
            kind: 'markdown',
            text: `\n**Apply transaction:** ${event.success ? 'completed' : 'failed'}.\n`,
          },
        ];
      case 'rollback_available':
        return [
          {
            kind: 'button',
            command: 'opticcode.internal.chat.rollbackTransaction',
            title: 'Rollback Transaction',
            arguments: [event.transaction_id],
          },
        ];
      case 'completed':
        return this.completed(event.summary);
      case 'cancelled':
        return [{ kind: 'markdown', text: `\n\n_OpticCode cancelled: ${escapeMarkdown(event.reason)}_` }];
      case 'failed':
        return [
          {
            kind: 'markdown',
            text: `\n\n**OpticCode failed (${escapeMarkdown(event.error.code)}):** ${escapeMarkdown(event.error.message)}`,
          },
        ];
    }
  }

  private completed(summary: ChatCompletionSummary): ChatRenderOperation[] {
    const operations: ChatRenderOperation[] = [];
    const userReferences = summary.references.filter((reference) => reference.path !== null);
    if (userReferences.length !== 0) {
      operations.push({ kind: 'markdown', text: '\n\n**User references**\n\n' });
      for (const reference of userReferences) {
        if (reference.path !== null) {
          operations.push({
            kind: 'anchor',
            path: reference.path,
            title: referenceLabel(reference),
            ...(reference.range === null ? {} : { range: reference.range }),
          });
          operations.push({
            kind: 'markdown',
            text: ` — ${escapeMarkdown(reference.inclusion_reason)}\n\n`,
          });
        }
      }
    }
    const discovered = summary.context_files.map((file) => file.path);
    if (discovered.length !== 0) {
      operations.push({ kind: 'markdown', text: '**Discovered context**\n\n' });
      for (const file of summary.context_files.slice(0, 32)) {
        operations.push({ kind: 'anchor', path: file.path, title: file.path });
        operations.push({
          kind: 'markdown',
          text: ` — ${escapeMarkdown(file.provenance)}, ${file.snippets} snippet(s)\n\n`,
        });
      }
      operations.push({ kind: 'filetree', paths: discovered.slice(0, 128) });
    }
    operations.push({
      kind: 'markdown',
      text: metricsMarkdown(summary, this.retrievalHits),
    });
    operations.push(
      {
        kind: 'button',
        command: 'opticcode.internal.chat.showContext',
        title: 'Show Context',
        arguments: [summary.run_id],
      },
      {
        kind: 'button',
        command: 'opticcode.internal.chat.showReport',
        title: 'Show Full Report',
        arguments: [summary.run_id],
      },
      {
        kind: 'button',
        command: 'opticcode.openOutput',
        title: 'Open Output',
      },
      {
        kind: 'button',
        command: 'opticcode.refreshStatus',
        title: 'Refresh Status',
      },
    );
    return operations;
  }
}

function metricsMarkdown(summary: ChatCompletionSummary, retrievalHits: number): string {
  const metrics = summary.metrics;
  return [
    '',
    '---',
    '',
    `Context: **${summary.used_context_mode ?? summary.requested_context_mode}**  `,
    `Prompt: **${metrics.prompt_tokens ?? metrics.estimated_prompt_tokens} tokens**  `,
    `Output: **${metrics.generated_tokens ?? 0} tokens**  `,
    `Duration: **${(metrics.total_ms / 1000).toFixed(2)} s**  `,
    `Local RAG: **${retrievalHits} hit(s)**`,
    '',
  ].join('\n');
}

function referenceLabel(reference: ChatResolvedReference): string {
  if (reference.path === null || reference.range === null) {
    return reference.path ?? reference.reference_id;
  }
  return `${reference.path}:${reference.range.start.line + 1}-${reference.range.end.line + 1}`;
}

function escapeMarkdown(value: string): string {
  return value.replaceAll(/[\\`*_{}[\]()#+.!|>-]/g, '\\$&');
}
