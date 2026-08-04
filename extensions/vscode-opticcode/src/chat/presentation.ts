import type {
  ChatCompletionSummary,
  ChatGroundingSummary,
  ChatProtocolEvent,
  ChatResolvedReference,
  ChatTextRange,
  ChatUiTiming,
} from '../protocol/types';

export type ChatRenderOperation =
  | { kind: 'progress'; text: string }
  | { kind: 'markdown'; text: string }
  | { kind: 'answer'; text: string }
  | { kind: 'reference'; path: string; range?: ChatTextRange | undefined }
  | { kind: 'anchor'; path: string; title: string; range?: ChatTextRange | undefined }
  | { kind: 'filetree'; paths: string[] }
  | { kind: 'button'; command: string; title: string; arguments?: unknown[] | undefined };

export class ChatEventPresenter {
  private readonly injectedReferences = new Map<string, ChatResolvedReference>();
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
        return [
          ...event.rejected.map(
            (reference): ChatRenderOperation => ({
              kind: 'markdown',
              text: `\n> **Reference refused (${escapeMarkdown(reference.rule_id)}):** ${escapeMarkdown(reference.reason)}\n`,
            }),
          ),
        ];
      case 'reference_selected':
      case 'reference_resolved':
        return [];
      case 'reference_injected':
        this.injectedReferences.set(event.reference.reference_id, event.reference);
        return event.reference.path === null
          ? []
          : [
              {
                kind: 'reference',
                path: event.reference.path,
                ...(event.reference.range === null ? {} : { range: event.reference.range }),
              },
            ];
      case 'reference_refused':
        return [
          {
            kind: 'markdown',
            text: `\n> **Reference refused (${escapeMarkdown(event.reference.reason_code ?? event.reference.rule_id)}):** ${escapeMarkdown(event.reference.reason)}\n`,
          },
        ];
      case 'context_manifest_ready':
        return [
          {
            kind: 'progress',
            text: `Grounded context ready: ${event.manifest.entries.length} injected reference(s).`,
          },
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
        return [{ kind: 'answer', text: event.text }];
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
      case 'timing_metrics':
        return [];
      case 'grounding_validation_started':
        return [{ kind: 'progress', text: 'Validating answer evidence and task compliance...' }];
      case 'grounding_validation_completed':
        return event.evidence.valid && event.compliance.compliant
          ? [{ kind: 'progress', text: 'Grounding validation passed.' }]
          : [];
      case 'task_compliance_failed':
        return [
          {
            kind: 'markdown',
            text: '\n> **Grounding refused:** The generated answer did not satisfy the requested task or evidence contract.\n',
          },
        ];
      case 'internal_context_leak_detected':
        return [
          {
            kind: 'markdown',
            text: '\n> **Grounding refused:** Unauthorized internal context was detected and was not shown.\n',
          },
        ];
      case 'document_inspection_completed':
        return [
          {
            kind: 'progress',
            text: `DocumentFacts inspected ${event.facts} fact(s) with ${event.model_calls} model call(s).`,
          },
        ];
      case 'edit_plan_started':
        return [{ kind: 'progress', text: 'Generating a bounded structured edit plan...' }];
      case 'edit_plan_ready':
        return [
          { kind: 'markdown', text: `\n**Edit plan:** ${escapeMarkdown(event.summary)}\n` },
        ];
      case 'policy_decision':
        return [
          {
            kind: 'progress',
            text: `Policy ${event.decision}: ${event.stage} (${event.rule_id}).`,
          },
        ];
      case 'proposal_stored':
        return [{ kind: 'progress', text: `Proposal stored as ${event.proposal_id}.` }];
      case 'verification_started':
        return [{ kind: 'progress', text: 'Verifying proposal in a disposable worktree...' }];
      case 'worktree_created':
        return [{ kind: 'progress', text: 'Detached disposable worktree created.' }];
      case 'edit_applied_in_worktree':
        return [
          {
            kind: 'progress',
            text: event.success
              ? 'Validated snapshots applied in the isolated worktree.'
              : 'Isolated worktree apply failed.',
          },
        ];
      case 'build_started':
        return [{ kind: 'progress', text: 'Running the allowlisted offline build...' }];
      case 'build_completed':
        return [
          {
            kind: 'progress',
            text: `Offline build ${event.build}; tests ${event.tests}.`,
          },
        ];
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
          {
            kind: 'button',
            command: 'opticcode.internal.chat.showAllChanges',
            title: 'Show All Changes',
            arguments: [event.proposal_id],
          },
          {
            kind: 'button',
            command: 'opticcode.internal.chat.discardProposal',
            title: 'Discard Proposal',
            arguments: [event.proposal_id],
          },
        ];
      case 'approval_required':
        return [
          {
            kind: 'button',
            command:
              event.operation === 'rollback'
                ? 'opticcode.internal.chat.rollbackTransaction'
                : 'opticcode.internal.chat.applyProposal',
            title:
              event.operation === 'rollback'
                ? 'Rollback Transaction'
                : 'Apply Verified Changes',
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
            arguments: [event.proposal_id, event.transaction_id],
          },
        ];
      case 'rollback_started':
        return [{ kind: 'progress', text: 'Rolling back the exact OpticCode transaction...' }];
      case 'rollback_completed':
        return [
          {
            kind: 'markdown',
            text: `\n**Rollback:** ${event.success ? 'completed' : 'failed'}${event.already_rolled_back ? ' (already restored)' : ''}.\n`,
          },
        ];
      case 'proposal_discarded':
        return [
          {
            kind: 'markdown',
            text: `\nProposal \`${escapeMarkdown(event.proposal_id)}\` discarded.\n`,
          },
        ];
      case 'completed':
        return [];
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

  public complete(
    summary: ChatCompletionSummary | undefined,
    timing: ChatUiTiming,
  ): ChatRenderOperation[] {
    return summary === undefined ? [] : this.completed(summary, timing);
  }

  private completed(summary: ChatCompletionSummary, timing: ChatUiTiming): ChatRenderOperation[] {
    const operations: ChatRenderOperation[] = [];
    const grounding = summary.grounding ?? undefined;
    if (grounding === undefined) {
      operations.push({
        kind: 'markdown',
        text: '\n\n> **Grounding status:** This runtime did not provide the strict grounding report.\n',
      });
    } else {
      operations.push({ kind: 'markdown', text: groundingMarkdown(grounding) });
      if (grounding.manifest.entries.length !== 0) {
        operations.push({ kind: 'markdown', text: '**Injected references**\n\n' });
        for (const entry of grounding.manifest.entries) {
          const resolved = this.injectedReferences.get(entry.reference_id);
          operations.push({
            kind: 'anchor',
            path: entry.path,
            title: referenceLabelFromManifest(entry.path, entry.ranges[0]),
            ...(resolved?.range === undefined || resolved.range === null
              ? {}
              : { range: resolved.range }),
          });
          operations.push({
            kind: 'markdown',
            text: ` — ${entry.bytes_injected} bytes, ${escapeMarkdown(entry.reason)}\n\n`,
          });
        }
      }
    }
    const discovered =
      grounding !== undefined && grounding.route !== 'automatic_assistant'
        ? []
        : summary.context_files.map((file) => file.path);
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
      text: metricsMarkdown(summary, timing, this.retrievalHits),
    });
    if (grounding !== undefined) {
      operations.push(
        {
          kind: 'button',
          command: 'opticcode.internal.chat.showInjectedContext',
          title: 'Show Injected Context',
          arguments: [grounding],
        },
        {
          kind: 'button',
          command: 'opticcode.internal.chat.showEvidence',
          title: 'Show Evidence',
          arguments: [grounding],
        },
        {
          kind: 'button',
          command: 'opticcode.internal.chat.copyGroundingReport',
          title: 'Copy Grounding Report',
          arguments: [grounding],
        },
      );
    }
    operations.push(
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

function groundingMarkdown(grounding: ChatGroundingSummary): string {
  const fingerprint = grounding.manifest.fingerprint.slice(0, 12);
  return [
    '',
    '---',
    '',
    `Scope: **${grounding.effective_scope}**  `,
    `Evidence: **${grounding.evidence_mode}**  `,
    `Answer source: **${grounding.route}**  `,
    `User references: **${grounding.selected_references}**  `,
    `Injected references: **${grounding.injected_references}**  `,
    `Discovered files: **${grounding.discovered_files}**  `,
    `RAG hits: **${grounding.rag_hits}**  `,
    `Context fingerprint: **${fingerprint}...**`,
    '',
  ].join('\n');
}

function metricsMarkdown(
  summary: ChatCompletionSummary,
  timing: ChatUiTiming,
  retrievalHits: number,
): string {
  const metrics = summary.metrics;
  const modelMs = phaseDuration(metrics, 'provider_total') ?? phaseDuration(metrics, 'provider_client');
  const contextMs = phaseDuration(metrics, 'context_build') ??
    (phaseDuration(metrics, 'reference_resolution') ?? 0) +
      (phaseDuration(metrics, 'prompt_build') ?? 0);
  const ragHits = summary.grounding?.rag_hits ?? retrievalHits;
  const deterministic = summary.grounding?.route === 'document_facts';
  const promptTokens = deterministic
    ? '0 model tokens'
    : `${metrics.prompt_tokens ?? metrics.estimated_prompt_tokens} tokens`;
  const outputTokens = deterministic
    ? '0 model tokens'
    : `${metrics.generated_tokens ?? 0} tokens`;
  return [
    '',
    '---',
    '',
    `Context: **${summary.used_context_mode ?? summary.requested_context_mode}**  `,
    `Prompt: **${promptTokens}**  `,
    `Output: **${outputTokens}**  `,
    `First token: **${formatDuration(timing.first_token_ms)}**  `,
    `Answer streaming: **${formatDuration(timing.answer_streaming_ms)}**  `,
    `Visible response: **${formatDuration(timing.visible_response_ms)}**  `,
    `Total pipeline: **${formatDuration(timing.total_pipeline_ms)}**  `,
    ...(modelMs === undefined ? [] : [`Model: **${formatDuration(modelMs)}**  `]),
    `Context build: **${formatDuration(contextMs)}**  `,
    ...(timing.post_processing_ms > 0
      ? [`Post-processing: **${formatDuration(timing.post_processing_ms)}**  `]
      : []),
    `Local RAG: **${ragHits} hit(s)**`,
    '',
  ].join('\n');
}

function phaseDuration(metrics: ChatCompletionSummary['metrics'], name: string): number | undefined {
  return metrics.timing?.phases.find((phase) => phase.name === name)?.duration_ms;
}

function formatDuration(value: number | null): string {
  if (value === null || !Number.isFinite(value) || value < 0) {
    return 'n/a';
  }
  return `${(value / 1000).toFixed(2)} s`;
}

function referenceLabelFromManifest(
  path: string,
  range: { start_line: number; end_line: number } | undefined,
): string {
  return range === undefined ? path : `${path}:${range.start_line}-${range.end_line}`;
}

function escapeMarkdown(value: string): string {
  return value.replaceAll(/[\\`*_{}[\]()#+.!|>-]/g, '\\$&');
}
