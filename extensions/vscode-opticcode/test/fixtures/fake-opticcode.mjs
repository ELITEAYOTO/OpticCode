const scenario = process.argv[2] ?? 'json-valid';
const argumentsAfterScenario = process.argv.slice(3);
const requestIndex = argumentsAfterScenario.indexOf('--request-id');
const requestId = requestIndex >= 0 ? argumentsAfterScenario[requestIndex + 1] : 'fixture-request';

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
  default:
    process.stderr.write(`unknown fixture scenario: ${scenario}\n`);
    process.exitCode = 9;
}
