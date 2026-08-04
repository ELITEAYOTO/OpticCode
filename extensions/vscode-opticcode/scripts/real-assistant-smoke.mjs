import path from 'node:path';

import { OpticCodeProtocolClient } from '../out/src/protocol/client.js';

const repository = process.env.OPTICCODE_REPO_ROOT ?? path.resolve(process.cwd(), '..', '..');
const executable =
  process.env.OPTICCODE_EXE ??
  path.join(repository, 'target', 'release', process.platform === 'win32' ? 'opticcode.exe' : 'opticcode');
const model = process.env.OPTICCODE_MODEL ?? 'qwen2.5-coder:14b';
const client = new OpticCodeProtocolClient({
  executablePath: executable,
  workingDirectory: repository,
  timeoutMs: 300_000,
});

for (const command of ['ask', 'plan']) {
  const requestId = `vscode-real-${command}-${Date.now()}`;
  const result = await client.runAssistantStream(
    [
      command,
      command === 'ask'
        ? 'Reply with only the Java version required by this project.'
        : 'Give one concise step to inspect plugin.yml.',
      '--path',
      path.join(repository, 'benchmarks', 'java-index-mini'),
      '--profile',
      'none',
      '--no-memory',
      '--no-rag',
      '--model',
      model,
      '--context-mode',
      'legacy',
      '--temperature',
      '0',
      '--seed',
      '42',
      '--max-tokens',
      '32',
      '--http-timeout-ms',
      '300000',
    ],
    requestId,
  );
  if (result.status !== 'completed' || result.response.trim() === '') {
    throw new Error(`${command} real stream did not complete with model output`);
  }
  const providerEvents = result.events.filter((event) => event.type === 'provider_event');
  if (providerEvents.length < 2 || result.summary === undefined || result.generation === undefined) {
    throw new Error(`${command} real stream did not expose complete metrics and protocol events`);
  }
  process.stdout.write(
    `${command}: completed, events=${result.events.length}, prompt_tokens=${String(result.generation.usage.prompt_tokens)}, generated_tokens=${String(result.generation.usage.generated_tokens)}\n`,
  );
}
