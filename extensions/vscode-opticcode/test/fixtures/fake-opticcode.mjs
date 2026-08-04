const scenario = process.argv[2] ?? 'json-valid';
const argumentsAfterScenario = process.argv.slice(3);
const requestIndex = argumentsAfterScenario.indexOf('--request-id');
const requestId = requestIndex >= 0 ? argumentsAfterScenario[requestIndex + 1] : 'fixture-request';

async function initialChatRequest() {
  const { createInterface } = await import('node:readline');
  const lines = createInterface({ input: process.stdin, crlfDelay: Infinity });
  const iterator = lines[Symbol.asyncIterator]();
  const first = await iterator.next();
  if (first.done || typeof first.value !== 'string') {
    throw new Error('missing chat request');
  }
  return { request: JSON.parse(first.value), iterator, lines };
}

function chat(id, sequence, type, extra = {}) {
  return {
    schema_version: 1,
    protocol: 'opticcode.chat',
    request_id: id,
    sequence,
    elapsed_ms: sequence * 2,
    type,
    ...extra,
  };
}

function accepted(command, requestedSecurityMode = 'read_only') {
  return {
    command,
    requested_security_mode: requestedSecurityMode,
    security_mode: 'read_only',
    effective_security_mode: 'read_only',
    policy_version: 'opticcode.default.v1',
    policy_decision: 'allow',
    policy_rule_id: command === 'status' ? 'git.read_allowlist' : 'analysis.context_read_only',
  };
}

function validChatEvents(id) {
  const metrics = {
    preparation_ms: 12,
    total_ms: 40,
    estimated_prompt_tokens: 42,
    prompt_tokens: 40,
    generated_tokens: 3,
    generated_tokens_per_second: 25,
  };
  const reference = {
    reference_id: 'attached-file-0',
    kind: 'range',
    path: 'src/main/java/Plugin.java',
    range: { start: { line: 1, character: 2 }, end: { line: 2, character: 4 } },
    inclusion_reason: 'attached by user',
    provenance: 'user_reference',
    bytes: 24,
    content_hash: 'fixture-hash',
  };
  return [
    chat(id, 0, 'request_accepted', accepted('ask')),
    chat(id, 1, 'references_resolving', { count: 1 }),
    chat(id, 2, 'references_resolved', { accepted: [reference], rejected: [] }),
    chat(id, 3, 'context_started', { requested_mode: 'symbol' }),
    chat(id, 4, 'context_ready', {
      requested_mode: 'symbol',
      used_mode: 'symbol',
      analysis_complete: true,
      estimated_tokens: 42,
      files: [
        { path: 'src/main/java/Plugin.java', snippets: 1, provenance: 'context_discovery' },
      ],
    }),
    chat(id, 5, 'retrieval_progress', { query_count: 2, hit_count: 1 }),
    chat(id, 6, 'provider_started', {
      provider: 'ollama',
      model: 'fixture-model',
      context_mode: 'symbol',
    }),
    chat(id, 7, 'token_delta', { text: 'hello ' }),
    chat(id, 8, 'token_delta', { text: '**chat**' }),
    chat(id, 9, 'metrics', { metrics }),
    chat(id, 10, 'completed', {
      summary: {
        command: 'ask',
        success: true,
        model: 'fixture-model',
        requested_context_mode: 'symbol',
        used_context_mode: 'symbol',
        references: [reference],
        rejected_references: 0,
        context_files: [
          { path: 'src/main/java/Plugin.java', snippets: 1, provenance: 'context_discovery' },
        ],
        warnings: [],
        metrics,
        repository_state: 'fixture-state',
        run_id: 'fixture-chat-run',
      },
    }),
  ];
}

function assistant(sequence, type, extra = {}) {
  return {
    schema_version: 1,
    protocol: 'opticcode.assistant',
    request_id: requestId,
    sequence,
    type,
    ...extra,
  };
}

function llm(sequence, type, extra = {}) {
  return {
    schema_version: 1,
    protocol: 'opticcode.llm',
    request_id: `${requestId}:legacy`,
    sequence,
    type,
    ...extra,
  };
}

function generation() {
  return {
    schema_version: 1,
    request_id: `${requestId}:legacy`,
    provider: 'ollama',
    model: 'fixture-model',
    output: 'hello world',
    finish_reason: 'stop',
    prompt_chars: 120,
    usage: { prompt_tokens: 30, generated_tokens: 2 },
    timings: { client_ms: 25, provider_total_ms: 20, generation_ms: 10 },
  };
}

function validEvents() {
  return [
    assistant(0, 'started', {
      command: 'ask',
      provider: 'ollama',
      model: 'fixture-model',
      requested_context_mode: 'legacy',
    }),
    assistant(1, 'context_prepared', {
      requested_context_mode: 'legacy',
      used_context_mode: 'legacy',
      analysis_complete: true,
      fallback_applied: false,
      variant_count: 1,
    }),
    assistant(2, 'provider_event', { context_mode: 'legacy', event: llm(0, 'started') }),
    assistant(3, 'provider_event', {
      context_mode: 'legacy',
      event: llm(1, 'delta', { text: 'hello ' }),
    }),
    assistant(4, 'provider_event', {
      context_mode: 'legacy',
      event: llm(2, 'delta', { text: 'world' }),
    }),
    assistant(5, 'provider_event', {
      context_mode: 'legacy',
      event: llm(3, 'completed', { result: generation() }),
    }),
    assistant(6, 'completed', {
      report_schema_version: 1,
      generated_runs: 1,
      summary: {
        command: 'ask',
        success: true,
        model: 'fixture-model',
        requested_context_mode: 'legacy',
        used_context_mode: 'legacy',
        preparation_duration_us: 100,
        warnings: [],
        context_files: [
          { context_mode: 'legacy', path: 'src/main/java/Plugin.java', snippets: 1 },
        ],
        runs: [
          {
            context_mode: 'legacy',
            generated: true,
            estimated_prompt_tokens: 32,
            client_ms: 25,
            prompt_tokens: 30,
            generated_tokens: 2,
            generated_tokens_per_second: 20,
          },
        ],
      },
    }),
  ];
}

function writeEvents(events) {
  process.stdout.write(`${events.map((event) => JSON.stringify(event)).join('\n')}\n`);
}

switch (scenario) {
  case 'echo':
    process.stdout.write(JSON.stringify({ argv: argumentsAfterScenario }));
    break;
  case 'json-valid':
    process.stdout.write(JSON.stringify({ ok: true }));
    break;
  case 'json-invalid':
    process.stdout.write('{invalid');
    break;
  case 'stdout-parasite':
    process.stdout.write('debug output\n{}');
    break;
  case 'large-json':
    process.stdout.write(JSON.stringify({ value: 'x'.repeat(8192) }));
    break;
  case 'valid-stream':
    writeEvents(validEvents());
    break;
  case 'fragmented-stream': {
    const payload = `${validEvents().map((event) => JSON.stringify(event)).join('\n')}\n`;
    for (let index = 0; index < payload.length; index += 7) {
      process.stdout.write(payload.slice(index, index + 7));
    }
    break;
  }
  case 'bad-sequence':
    writeEvents([assistant(1, 'started')]);
    break;
  case 'nested-bad-sequence':
    writeEvents([
      assistant(0, 'started'),
      assistant(1, 'provider_event', { context_mode: 'legacy', event: llm(2, 'started') }),
    ]);
    break;
  case 'request-mismatch':
    process.stdout.write(
      `${JSON.stringify({ ...assistant(0, 'started'), request_id: 'different-request' })}\n`,
    );
    break;
  case 'nested-request-mismatch': {
    const changed = { ...llm(0, 'delta', { text: 'bad' }), request_id: `${requestId}:other` };
    writeEvents([
      assistant(0, 'started'),
      assistant(1, 'provider_event', { context_mode: 'legacy', event: llm(0, 'started') }),
      assistant(2, 'provider_event', { context_mode: 'legacy', event: changed }),
    ]);
    break;
  }
  case 'nested-missing-terminal':
    writeEvents([
      assistant(0, 'started'),
      assistant(1, 'provider_event', { context_mode: 'legacy', event: llm(0, 'started') }),
      assistant(2, 'completed'),
    ]);
    break;
  case 'missing-terminal':
    writeEvents([assistant(0, 'started')]);
    break;
  case 'double-terminal':
    writeEvents([
      assistant(0, 'started'),
      assistant(1, 'completed'),
      assistant(2, 'failed', { errors: [] }),
    ]);
    break;
  case 'incompatible':
    process.stdout.write(
      `${JSON.stringify({ ...assistant(0, 'started'), schema_version: 2 })}\n`,
    );
    break;
  case 'invalid-ndjson':
    process.stdout.write('not-json\n');
    break;
  case 'large-line':
    process.stdout.write(`${'x'.repeat(8192)}\n`);
    break;
  case 'interrupted':
    writeEvents([assistant(0, 'started')]);
    process.exitCode = 7;
    break;
  case 'timeout':
    setTimeout(() => {}, 10_000);
    break;
  case 'cancel':
    writeEvents([assistant(0, 'started'), assistant(1, 'context_prepared')]);
    process.stdin.setEncoding('utf8');
    process.stdin.once('data', (value) => {
      if (value === 'cancel\n') {
        writeEvents([
          assistant(2, 'cancelled', {
            errors: [{ code: 'generation_cancelled', stage: 'generation', message: 'cancelled' }],
          }),
        ]);
        process.exitCode = 2;
      }
    });
    break;
  case 'chat-valid': {
    const input = await initialChatRequest();
    writeEvents(validChatEvents(input.request.request_id));
    input.lines.close();
    break;
  }
  case 'chat-fragmented': {
    const input = await initialChatRequest();
    const payload = `${validChatEvents(input.request.request_id).map((event) => JSON.stringify(event)).join('\n')}\n`;
    for (let index = 0; index < payload.length; index += 5) {
      process.stdout.write(payload.slice(index, index + 5));
    }
    input.lines.close();
    break;
  }
  case 'chat-bad-sequence': {
    const input = await initialChatRequest();
    writeEvents([
      chat(input.request.request_id, 1, 'request_accepted', accepted(input.request.command)),
    ]);
    input.lines.close();
    break;
  }
  case 'chat-request-mismatch': {
    const input = await initialChatRequest();
    writeEvents([
      chat('another-chat-request', 0, 'request_accepted', accepted(input.request.command)),
    ]);
    input.lines.close();
    break;
  }
  case 'chat-missing-terminal': {
    const input = await initialChatRequest();
    writeEvents([
      chat(input.request.request_id, 0, 'request_accepted', accepted(input.request.command)),
    ]);
    input.lines.close();
    break;
  }
  case 'chat-double-terminal': {
    const input = await initialChatRequest();
    const id = input.request.request_id;
    writeEvents([
      chat(id, 0, 'cancelled', { reason: 'fixture cancellation' }),
      chat(id, 1, 'failed', {
        error: { code: 'late', stage: 'fixture', message: 'late terminal', retriable: false },
      }),
    ]);
    input.lines.close();
    process.exitCode = 2;
    break;
  }
  case 'chat-late-metrics': {
    const input = await initialChatRequest();
    const events = validChatEvents(input.request.request_id);
    const metrics = events.find((event) => event.type === 'metrics').metrics;
    writeEvents([
      ...events,
      chat(input.request.request_id, events.length, 'metrics', { metrics }),
    ]);
    input.lines.close();
    break;
  }
  case 'chat-timeout': {
    await initialChatRequest();
    setTimeout(() => {}, 10_000);
    break;
  }
  case 'chat-cancel': {
    const input = await initialChatRequest();
    const id = input.request.request_id;
    writeEvents([
      chat(id, 0, 'request_accepted', accepted(input.request.command)),
    ]);
    const control = await input.iterator.next();
    const message = control.done ? undefined : JSON.parse(control.value);
    if (message?.protocol === 'opticcode.chat.control' && message?.type === 'cancel') {
      writeEvents([chat(id, 1, 'cancelled', { reason: 'cancelled by fixture client' })]);
      process.exitCode = 2;
    }
    input.lines.close();
    break;
  }
  case 'chat-ignore-cancel': {
    const input = await initialChatRequest();
    writeEvents([
      chat(
        input.request.request_id,
        0,
        'request_accepted',
        accepted(input.request.command),
      ),
    ]);
    await input.iterator.next();
    setTimeout(() => {}, 10_000);
    break;
  }
  default:
    process.stderr.write(`unknown fixture scenario: ${scenario}\n`);
    process.exitCode = 9;
}
