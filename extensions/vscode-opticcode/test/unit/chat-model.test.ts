import * as assert from 'node:assert/strict';
import * as path from 'node:path';
import { describe, it } from 'node:test';

import {
  boundChatHistory,
  buildChatRequest,
  ChatRequestBuildError,
  collectWorkspaceReferences,
  parseChatCommand,
  sessionNamespace,
  workspaceIdentity,
} from '../../src/chat/model';
import { ChatEventPresenter } from '../../src/chat/presentation';
import type {
  ChatCompletionSummary,
  ChatProtocolEvent,
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
    },
    repository_state: 'state',
    run_id: 'run-test',
  };
}

describe('OpticCode chat model', () => {
  it('defaults to ask and rejects unknown slash commands', () => {
    assert.equal(parseChatCommand(undefined), 'ask');
    assert.equal(parseChatCommand(' PLAN '), 'plan');
    assert.equal(parseChatCommand('unknown'), undefined);
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
    assert.match(request.request_id, /^vscode-chat-ask-/u);

    assert.throws(
      () => buildChatRequest({ ...requestInput(), prompt: '' }),
      ChatRequestBuildError,
    );
    assert.doesNotThrow(() =>
      buildChatRequest({ ...requestInput(), command: 'help', prompt: '' }),
    );
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
    assert.deepEqual(presenter.accept(delta), [{ kind: 'markdown', text: '**answer**' }]);

    const completed: ChatProtocolEvent = {
      ...baseEvent(1),
      type: 'completed',
      summary: completionSummary(),
    };
    const operations = presenter.accept(completed);
    assert.ok(operations.some((operation) => operation.kind === 'anchor'));
    assert.ok(operations.some((operation) => operation.kind === 'filetree'));
    assert.ok(
      operations.some(
        (operation) => operation.kind === 'button' && operation.title === 'Show Full Report',
      ),
    );
    assert.ok(
      operations.some(
        (operation) => operation.kind === 'markdown' && operation.text.includes('5.20'),
      ),
    );
  });

  it('does not expose apply before an approval event exists', () => {
    const operations = new ChatEventPresenter().accept({
      ...baseEvent(0),
      type: 'completed',
      summary: completionSummary(),
    });
    assert.equal(
      operations.some(
        (operation) => operation.kind === 'button' && operation.title.includes('Apply'),
      ),
      false,
    );
  });
});
