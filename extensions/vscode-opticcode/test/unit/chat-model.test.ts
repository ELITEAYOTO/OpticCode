import * as assert from 'node:assert/strict';
import * as path from 'node:path';
import { describe, it } from 'node:test';

import {
  boundChatHistory,
  buildChatRequest,
  ChatRequestBuildError,
  collectWorkspaceReferences,
  parseChatCommand,
  promptRequestsReferencesOnly,
  requestedGroundingScope,
  sessionNamespace,
  workspaceIdentity,
} from '../../src/chat/model';
import { ChatEventPresenter } from '../../src/chat/presentation';
import type {
  ChatCompletionSummary,
  ChatProtocolEvent,
  ChatUiTiming,
} from '../../src/protocol/types';

const workspace = path.resolve('C:\fixture', 'Optic Code Unicode');

function requestInput() {
  return {
    command: 'ask' as const,
    prompt: 'Explain this selection.',
    workspaceRoot: workspace,
    profile: 'minecraft-java-1.8',
    model: 'qwen2.5-coder:14b',
    contextMode: 'symbol' as const,
    sessionId: 'session-test',
    clientVersion: '0.1.0',
    vscodeVersion: '1.125.0',
    locale: 'fr',
    references: [],
    history: [],
    recentRunIds: [],
    expectedProtocols: { chat: 1, assistant: 1, discovery: 1, llm: 1 },
  };
}

function baseEvent(sequence: number): {
  schema_version: number;
  protocol: string;
  request_id: string;
  sequence: number;
  elapsed_ms: number;
} {
  return {
    schema_version: 1,
    protocol: 'opticcode.chat',
    request_id: 'request-test',
    sequence,
    elapsed_ms: sequence,
  };
}

function completionSummary(): ChatCompletionSummary {
  return {
    command: 'ask',
    success: true,
    model: 'fixture-model',
    requested_context_mode: 'symbol',
    used_context_mode: 'symbol',
    references: [
      {
        reference_id: 'selection',
        kind: 'selection',
        path: 'src/Plugin.java',
        range: { start: { line: 2, character: 1 }, end: { line: 3, character: 4 } },
        inclusion_reason: 'selected by user',
        provenance: 'user_reference',
        bytes: 20,
        content_hash: 'hash',
      },
    ],
    rejected_references: 0,
    context_files: [
      { path: 'src/Helper.java', snippets: 2, provenance: 'context_discovery' },
    ],
    warnings: [],
    metrics: {
      preparation_ms: 10,
      total_ms: 5200,
      estimated_prompt_tokens: 1188,
      prompt_tokens: 1100,
      generated_tokens: 107,
      generated_tokens_per_second: 20,
      route: 'reference_llm',
      timing: {
        schema_version: 1,
        request_id: 'request-test',
        run_id: 'run-test',
        workspace_id: 'workspace-test',
        command: 'ask',
        unit: 'milliseconds',
        clock: 'std::time::Instant',
        phases: [
          { name: 'reference_resolution', duration_ms: 80, measured_by: 'rust', includes: [] },
          { name: 'prompt_build', duration_ms: 20, measured_by: 'rust', includes: [] },
          { name: 'provider_total', duration_ms: 3500, measured_by: 'ollama', includes: [] },
        ],
      },
    },
    repository_state: 'state',
    run_id: 'run-test',
    grounding: {
      schema_version: 1,
      route: 'reference_llm',
      requested_scope: 'references_only',
      effective_scope: 'references_only',
      scope_reason: 'explicit_prompt_restriction',
      evidence_mode: 'required',
      selected_references: 1,
      resolved_references: 1,
      injected_references: 1,
      refused_references: 0,
      discovered_files: 0,
      rag_hits: 0,
      historical_turns: 0,
      prompt_fingerprint: 'b'.repeat(64),
      manifest: {
        schema_version: 1,
        context_scope: 'references_only',
        workspace_id: 'workspace-test',
        request_id: 'request-test',
        prompt_version: 'opticcode-grounding-prompt-v2',
        profile: 'minecraft-java-1.8',
        entries: [{
          reference_id: 'selection',
          path: 'src/Plugin.java',
          origin: 'user_attachment',
          hash: 'a'.repeat(64),
          injected_hash: 'a'.repeat(64),
          size_bytes: 20,
          encoding: 'utf-8',
          line_ending: 'lf',
          ranges: [{ start_line: 3, end_line: 4, start_byte: 10, end_byte: 30 }],
          bytes_injected: 20,
          reason: 'explicit_user_reference',
          git_state: 'clean',
          workspace_id: 'workspace-test',
        }],
        total_bytes: 20,
        estimated_tokens: 5,
        fingerprint: 'c'.repeat(64),
      },
      evidence: { valid: true, claims_checked: 1, citations_checked: 1, errors: [] },
      compliance: {
        compliant: true,
        internal_context_leak: false,
        cross_file_leak: false,
        task_format_violation: false,
        errors: [],
      },
    },
  };
}

function uiTiming(): ChatUiTiming {
  return {
    schema_version: 1,
    request_id: 'request-test',
    clock: 'performance.now',
    first_token_ms: 820,
    answer_streaming_ms: 3410,
    visible_response_ms: 4230,
    total_pipeline_ms: 4580,
    post_processing_ms: 350,
    terminal_rendered_ms: 4580,
  };
}

describe('OpticCode chat model', () => {
  it('defaults to ask and rejects unknown slash commands', () => {
    assert.equal(parseChatCommand(undefined), 'ask');
    assert.equal(parseChatCommand(' PLAN '), 'plan');
    assert.equal(parseChatCommand('unknown'), undefined);
  });

  it('detects explicit reference-only intent in French and English', () => {
    assert.equal(promptRequestsReferencesOnly('Lis uniquement le fichier joint.'), true);
    assert.equal(promptRequestsReferencesOnly('Use only the attached file.'), true);
    assert.deepEqual(
      requestedGroundingScope('automatic', 'Ne lis aucun autre fichier.'),
      { scope: 'references_only', reason: 'explicit_prompt_restriction' },
    );
    assert.deepEqual(requestedGroundingScope('references_preferred', 'Explain this project.'), {
      scope: 'references_preferred',
      reason: 'default_setting',
    });
  });

  it('bounds malformed history, redacts secrets, and omits large prior diffs', () => {
    const history = boundChatHistory([
      { role: 'user', content: 42 },
      { role: 'user', content: 'password=super-secret' },
      {
        role: 'assistant',
        content: `before\n\`\`\`diff\n${'x'.repeat(3000)}\n\`\`\`\nafter`,
        command: 'ask',
        resultId: 'run-1',
      },
      ...Array.from({ length: 20 }, (_, index) => ({
        role: 'user' as const,
        content: `turn-${index}`,
      })),
    ]);
    assert.ok(history.length <= 12);
    assert.ok(history.every((turn) => turn.content.length <= 8192));
    assert.ok(history.every((turn) => !turn.content.includes('super-secret')));
  });

  it('normalizes file, Unicode range, and metadata references inside one workspace', () => {
    const collected = collectWorkspaceReferences(workspace, [
      {
        referenceId: 'file-1',
        inclusionReason: 'attached by user',
        kind: 'file',
        path: path.join(workspace, 'src', 'Plugin Été.java'),
      },
      {
        referenceId: 'selection-1',
        inclusionReason: 'active selection',
        kind: 'selection',
        path: path.join(workspace, 'src', 'Emoji.java'),
        range: { start: { line: 1, character: 4 }, end: { line: 1, character: 8 } },
      },
      {
        referenceId: 'run-1',
        inclusionReason: 'prior run metadata',
        kind: 'run',
        runId: 'run-valid',
      },
    ]);
    assert.equal(collected.rejected.length, 0);
    assert.equal(collected.accepted.length, 3);
    const file = collected.accepted[0];
    const selection = collected.accepted[1];
    assert.equal(file?.kind, 'file');
    if (file?.kind === 'file') {
      assert.match(file.path, /Plugin Été\.java$/u);
    }
    assert.equal(selection?.kind, 'selection');
    if (selection?.kind === 'selection') {
      assert.deepEqual(selection.range, {
        start: { line: 1, character: 4 },
        end: { line: 1, character: 8 },
      });
    }
  });

  it('rejects traversal and prevents workspace/session identity collisions', () => {
    const outside = collectWorkspaceReferences(workspace, [
      {
        referenceId: 'outside',
        inclusionReason: 'attached by user',
        kind: 'file',
        path: path.resolve(workspace, '..', 'secret.env'),
      },
    ]);
    assert.equal(outside.accepted.length, 0);
    assert.match(outside.rejected[0]?.reason ?? '', /outside/u);
    assert.notEqual(workspaceIdentity(workspace), workspaceIdentity(`${workspace}-other`));
    assert.notEqual(
      sessionNamespace(workspace, 'head-a', 'session'),
      sessionNamespace(workspace, 'head-b', 'session'),
    );
  });

  it('builds a versioned read-only request and requires prompts only where needed', () => {
    const request = buildChatRequest(requestInput());
    assert.equal(request.protocol, 'opticcode.chat');
    assert.equal(request.security_mode, 'read_only');
    assert.equal(request.command, 'ask');
    assert.equal(request.generation.max_output_tokens, 1024);
    assert.equal(request.context_scope, 'references_preferred');
    assert.equal(request.evidence_mode, 'required');
    assert.match(request.request_id, /^vscode-chat-ask-/u);

    assert.throws(
      () => buildChatRequest({ ...requestInput(), prompt: '' }),
      ChatRequestBuildError,
    );
    assert.throws(
      () => buildChatRequest({ ...requestInput(), command: 'inspect', prompt: '' }),
      ChatRequestBuildError,
    );
    assert.doesNotThrow(() =>
      buildChatRequest({ ...requestInput(), command: 'help', prompt: '' }),
    );
    assert.throws(
      () => buildChatRequest({ ...requestInput(), command: 'fix', prompt: '' }),
      ChatRequestBuildError,
    );
    const fix = buildChatRequest({
      ...requestInput(),
      command: 'fix',
      prompt: 'Add a guard to the selected method.',
    });
    assert.equal(fix.generation.max_output_tokens, 4096);
    const apply = buildChatRequest({
      ...requestInput(),
      command: 'apply',
      prompt: '',
      edit: {
        proposal_id: 'plan-1',
        native_confirmation: {
          client: 'opticcode-vscode',
          confirmation_id: 'vscode-modal-1',
          approval_request_id: 'apply-confirmation-1',
        },
      },
    });
    assert.equal(apply.edit?.proposal_id, 'plan-1');
    assert.equal(apply.security_mode, 'read_only');
  });
});

describe('OpticCode chat presentation', () => {
  it('renders streamed Markdown, clickable references, buttons, and metrics', () => {
    const presenter = new ChatEventPresenter();
    const delta: ChatProtocolEvent = {
      ...baseEvent(0),
      type: 'token_delta',
      text: '**answer**',
    };
    assert.deepEqual(presenter.accept(delta), [{ kind: 'answer', text: '**answer**' }]);

    presenter.accept({
      ...baseEvent(1),
      type: 'reference_injected',
      reference: completionSummary().references[0]!,
    });

    const completed: ChatProtocolEvent = {
      ...baseEvent(2),
      type: 'completed',
      summary: completionSummary(),
    };
    assert.deepEqual(presenter.accept(completed), []);
    const operations = presenter.complete(completionSummary(), uiTiming());
    assert.ok(operations.some((operation) => operation.kind === 'anchor'));
    assert.equal(operations.some((operation) => operation.kind === 'filetree'), false);
    assert.ok(
      operations.some(
        (operation) =>
          operation.kind === 'button' && operation.title === 'Show Injected Context',
      ),
    );
    assert.ok(
      operations.some(
        (operation) => operation.kind === 'button' && operation.title === 'Show Full Report',
      ),
    );
    assert.ok(
      operations.some(
        (operation) =>
          operation.kind === 'markdown' &&
          operation.text.includes('First token') &&
          operation.text.includes('Total pipeline') &&
          !operation.text.includes('Duration:'),
      ),
    );
  });

  it('does not expose apply before an approval event exists', () => {
    const presenter = new ChatEventPresenter();
    presenter.accept({
      ...baseEvent(0),
      type: 'completed',
      summary: completionSummary(),
    });
    const operations = presenter.complete(completionSummary(), uiTiming());
    assert.equal(
      operations.some(
        (operation) => operation.kind === 'button' && operation.title.includes('Apply'),
      ),
      false,
    );
  });

  it('labels DocumentFacts as zero model-token work', () => {
    const summary = completionSummary();
    summary.metrics.prompt_tokens = null;
    summary.metrics.generated_tokens = null;
    summary.grounding!.route = 'document_facts';
    const operations = new ChatEventPresenter().complete(summary, uiTiming());
    const markdown = operations
      .filter((operation) => operation.kind === 'markdown')
      .map((operation) => operation.text)
      .join('');
    assert.match(markdown, /Prompt: \*\*0 model tokens\*\*/u);
    assert.match(markdown, /Output: \*\*0 model tokens\*\*/u);
  });
});
