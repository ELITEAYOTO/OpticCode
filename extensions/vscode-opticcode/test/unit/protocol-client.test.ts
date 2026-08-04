import * as assert from 'node:assert/strict';
import * as path from 'node:path';
import { describe, it } from 'node:test';

import {
  createSpawnInvocation,
  OpticCodeProtocolClient,
} from '../../src/protocol/client';
import { OpticCodeClientError } from '../../src/protocol/errors';
import type {
  CancellationLike,
  ChatProtocolRequest,
  JsonObject,
} from '../../src/protocol/types';
import { isRecord } from '../../src/protocol/validation';

const fixture = path.resolve(__dirname, '../../../test/fixtures/fake-opticcode.mjs');

function client(timeoutMs = 2_000, limits: Record<string, number> = {}): OpticCodeProtocolClient {
  return new OpticCodeProtocolClient({
    executablePath: process.execPath,
    workingDirectory: process.cwd(),
    timeoutMs,
    prefixArguments: [fixture],
    limits,
  });
}

function object(value: unknown): JsonObject {
  assert.ok(isRecord(value));
  return value;
}

function chatRequest(requestId: string): ChatProtocolRequest {
  return {
    schema_version: 1,
    protocol: 'opticcode.chat',
    request_id: requestId,
    workspace_id: 'fixture-workspace',
    workspace_root: process.cwd(),
    command: 'ask',
    prompt: 'Explain Plugin.java.',
    profile: 'minecraft-java-1.8',
    provider: 'ollama',
    model: 'fixture-model',
    context_mode: 'symbol',
    references: [],
    history: [],
    budgets: {
      max_history_turns: 12,
      max_history_chars: 32768,
      max_history_tokens: 8192,
      max_references: 24,
      max_reference_bytes: 1048576,
      max_prompt_tokens: 32768,
      rag_hits: 6,
    },
    generation: {
      max_output_tokens: 1024,
      temperature: null,
      seed: null,
      brief: false,
      compare_generate: false,
    },
    security_mode: 'read_only',
    client: {
      name: 'test',
      version: '0.1.0',
      vscode_version: '1.125.0',
      session_id: 'fixture-session',
      locale: 'en',
      recent_run_ids: [],
      previous_repository_state: null,
    },
    expected_protocols: { chat: 1, assistant: 1, discovery: 1, llm: 1 },
  };
}

async function rejectsCode(promise: Promise<unknown>, code: string): Promise<void> {
  await assert.rejects(promise, (error: unknown) => {
    return error instanceof OpticCodeClientError && error.code === code;
  });
}

class TestCancellation implements CancellationLike {
  private listener: (() => void) | undefined;
  public isCancellationRequested = false;

  public onCancellationRequested(listener: () => void): { dispose(): void } {
    this.listener = listener;
    return { dispose: () => (this.listener = undefined) };
  }

  public cancel(): void {
    this.isCancellationRequested = true;
    this.listener?.();
  }
}

describe('OpticCode protocol client', () => {
  it('uses shell false and preserves spaces and Unicode arguments', async () => {
    const values = ['C:\\Project With Spaces\\plugin', 'méthodeÉté'];
    const invocation = createSpawnInvocation('C:\\Optic Code\\opticcode.exe', values, 'C:\\Work Space');
    assert.equal(invocation.options.shell, false);
    assert.deepEqual(invocation.arguments, values);

    const response = await client().runJson(['echo', ...values], object);
    assert.deepEqual(response.argv, values);
  });

  it('accepts one valid JSON document', async () => {
    const response = await client().runJson(['json-valid'], object);
    assert.equal(response.ok, true);
  });

  it('rejects invalid JSON and stdout parasites', async () => {
    await rejectsCode(client().runJson(['json-invalid'], object), 'invalid_json');
    await rejectsCode(client().runJson(['stdout-parasite'], object), 'invalid_json');
  });

  it('bounds JSON output', async () => {
    await rejectsCode(
      client(2_000, { jsonBytes: 128 }).runJson(['large-json'], object),
      'output_limit',
    );
  });

  it('parses fragmented NDJSON and reconstructs output', async () => {
    const result = await client().runAssistantStream(['fragmented-stream'], 'request-fragmented');
    assert.equal(result.status, 'completed');
    assert.equal(result.response, 'hello world');
    assert.equal(result.events.length, 7);
    assert.equal(result.summary?.runs[0]?.prompt_tokens, 30);
  });

  it('rejects outer and nested sequence errors', async () => {
    await rejectsCode(
      client().runAssistantStream(['bad-sequence'], 'request-sequence'),
      'sequence_mismatch',
    );
    await rejectsCode(
      client().runAssistantStream(['nested-bad-sequence'], 'request-nested'),
      'sequence_mismatch',
    );
  });

  it('rejects outer and context-local request ID changes', async () => {
    await rejectsCode(
      client().runAssistantStream(['request-mismatch'], 'request-outer'),
      'request_mismatch',
    );
    await rejectsCode(
      client().runAssistantStream(['nested-request-mismatch'], 'request-nested-id'),
      'request_mismatch',
    );
  });

  it('rejects missing and double terminal events', async () => {
    await rejectsCode(
      client().runAssistantStream(['missing-terminal'], 'request-missing'),
      'terminal_missing',
    );
    await rejectsCode(
      client().runAssistantStream(['double-terminal'], 'request-double'),
      'terminal_duplicate',
    );
    await rejectsCode(
      client().runAssistantStream(['nested-missing-terminal'], 'request-nested-terminal'),
      'terminal_missing',
    );
  });

  it('rejects incompatible protocols and invalid NDJSON', async () => {
    await rejectsCode(
      client().runAssistantStream(['incompatible'], 'request-incompatible'),
      'protocol_incompatible',
    );
    await rejectsCode(
      client().runAssistantStream(['invalid-ndjson'], 'request-invalid'),
      'invalid_ndjson',
    );
  });

  it('bounds NDJSON line size', async () => {
    await rejectsCode(
      client(2_000, { ndjsonLineBytes: 128 }).runAssistantStream(
        ['large-line'],
        'request-large',
      ),
      'output_limit',
    );
  });

  it('distinguishes timeout and interrupted processes', async () => {
    await rejectsCode(
      client(100).runAssistantStream(['timeout'], 'request-timeout'),
      'timeout',
    );
    await rejectsCode(
      client().runAssistantStream(['interrupted'], 'request-interrupted'),
      'process_interrupted',
    );
  });

  it('confirms cancellation only through the terminal event', async () => {
    const cancellation = new TestCancellation();
    const promise = client().runAssistantStream(['cancel'], 'request-cancel', undefined, cancellation);
    setTimeout(() => cancellation.cancel(), 50);
    const result = await promise;
    assert.equal(result.status, 'cancelled');
    assert.equal(result.cancellationConfirmed, true);
    assert.equal(result.terminal.type, 'cancelled');
  });

  it('streams fragmented chat NDJSON with references, markdown, and metrics', async () => {
    const request = chatRequest('chat-fragmented-request');
    const result = await client().runChatStream(['chat-fragmented'], request);
    assert.equal(result.status, 'completed');
    assert.equal(result.response, 'hello **chat**');
    assert.equal(result.events.length, 11);
    assert.equal(result.summary?.references[0]?.path, 'src/main/java/Plugin.java');
    assert.equal(result.summary?.metrics.prompt_tokens, 40);
  });

  it('validates chat request IDs and outer sequences', async () => {
    await rejectsCode(
      client().runChatStream(['chat-bad-sequence'], chatRequest('chat-sequence')),
      'sequence_mismatch',
    );
    await rejectsCode(
      client().runChatStream(['chat-request-mismatch'], chatRequest('chat-request-id')),
      'request_mismatch',
    );
  });

  it('rejects missing and duplicate chat terminals', async () => {
    await rejectsCode(
      client().runChatStream(['chat-missing-terminal'], chatRequest('chat-missing')),
      'terminal_missing',
    );
    await rejectsCode(
      client().runChatStream(['chat-double-terminal'], chatRequest('chat-double')),
      'terminal_duplicate',
    );
  });

  it('distinguishes chat timeout, confirmed cancellation, and forced kill', async () => {
    await rejectsCode(
      client(100).runChatStream(['chat-timeout'], chatRequest('chat-timeout')),
      'timeout',
    );

    const cleanCancellation = new TestCancellation();
    const cleanPromise = client().runChatStream(
      ['chat-cancel'],
      chatRequest('chat-clean-cancel'),
      undefined,
      cleanCancellation,
    );
    setTimeout(() => cleanCancellation.cancel(), 50);
    const cleanResult = await cleanPromise;
    assert.equal(cleanResult.status, 'cancelled');
    assert.equal(cleanResult.cancellationConfirmed, true);

    const forcedCancellation = new TestCancellation();
    const forcedPromise = client(5_000).runChatStream(
      ['chat-ignore-cancel'],
      chatRequest('chat-forced-cancel'),
      undefined,
      forcedCancellation,
    );
    setTimeout(() => forcedCancellation.cancel(), 50);
    await rejectsCode(forcedPromise, 'cancellation_forced');
  });

  it('bounds the initial structured chat request', async () => {
    const request = chatRequest('chat-input-limit');
    request.prompt = 'x'.repeat(4096);
    await rejectsCode(
      client(2_000, { ndjsonLineBytes: 512 }).runChatStream(['chat-valid'], request),
      'input_limit',
    );
  });
});
