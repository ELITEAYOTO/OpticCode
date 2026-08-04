import { promises as fs } from 'node:fs';
import { Buffer } from 'node:buffer';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { runTests } from '@vscode/test-electron';

const directory = path.dirname(fileURLToPath(import.meta.url));
const extensionDevelopmentPath = path.resolve(directory, '..');
const root = path.resolve(extensionDevelopmentPath, '..', '..');
const extensionTestsPath = path.join(
  extensionDevelopmentPath,
  'out',
  'test',
  'vscode',
  'promptLab.js',
);
const mode = process.env.OPTICCODE_PROMPT_LAB_MODE ?? 'mock';
if (!['mock', 'holdout', 'qwen'].includes(mode)) {
  throw new Error(`Unsupported Prompt Lab mode: ${mode}`);
}
const sourceFixture = path.join(root, 'benchmarks', 'grounding-plugin');
const temporaryWorkspace = await fs.mkdtemp(path.join(os.tmpdir(), 'opticcode-prompt-lab-'));
const resultPath = process.env.OPTICCODE_PROMPT_LAB_RESULT ??
  path.join(root, 'benchmarks', 'runs', `prompt-lab-${mode}.json`);
let mock;

try {
  await fs.cp(sourceFixture, temporaryWorkspace, { recursive: true });
  process.env.OPTICCODE_PROMPT_LAB = '1';
  process.env.OPTICCODE_PROMPT_LAB_MODE = mode;
  process.env.OPTICCODE_PROMPT_LAB_ROOT = root;
  process.env.OPTICCODE_PROMPT_LAB_RESULT = resultPath;
  if (mode === 'mock') {
    mock = await startMockOllama();
    process.env.OPTICCODE_PROMPT_LAB_OLLAMA_URL = mock.url;
  } else {
    delete process.env.OPTICCODE_PROMPT_LAB_OLLAMA_URL;
  }
  await runTests({
    extensionDevelopmentPath,
    extensionTestsPath,
    launchArgs: [temporaryWorkspace, '--disable-extensions'],
  });
  if (mock !== undefined) {
    const report = JSON.parse(await fs.readFile(resultPath, 'utf8'));
    report.mock_provider = mock.stats;
    await fs.writeFile(resultPath, `${JSON.stringify(report, null, 2)}\n`, 'utf8');
  }
  process.stdout.write(`Prompt Lab ${mode} report: ${resultPath}\n`);
} finally {
  if (mock !== undefined) {
    await mock.close();
  }
  await removeTemporaryWorkspace(temporaryWorkspace);
}

async function startMockOllama() {
  const stats = {
    model_list_requests: 0,
    generation_requests: 0,
    scenarios: {},
    retained_prompt_content: false,
  };
  const server = http.createServer(async (request, response) => {
    try {
      if (request.method === 'GET' && request.url === '/api/tags') {
        stats.model_list_requests += 1;
        sendJson(response, {
          models: [
            {
              name: 'prompt-lab-mock',
              model: 'prompt-lab-mock',
              size: 1,
              digest: 'prompt-lab',
              details: {
                family: 'deterministic',
                parameter_size: '0B',
                quantization_level: 'none',
              },
            },
          ],
        });
        return;
      }
      if (request.method === 'POST' && request.url === '/api/generate') {
        const body = JSON.parse(await readBoundedBody(request, 2 * 1024 * 1024));
        const prompt = String(body.prompt ?? '');
        const scenario = classifyScenario(prompt);
        stats.generation_requests += 1;
        stats.scenarios[scenario] = (stats.scenarios[scenario] ?? 0) + 1;
        const grounded = groundedMockResponse(prompt, scenario);
        const delayMs = scenario === 'timing' ? 350 : 0;
        if (delayMs !== 0) {
          await new Promise((resolve) => setTimeout(resolve, delayMs));
        }
        sendJson(response, {
          model: 'prompt-lab-mock',
          created_at: new Date(0).toISOString(),
          response: JSON.stringify(grounded),
          done: true,
          done_reason: 'stop',
          total_duration: Math.max(1, delayMs) * 1_000_000,
          load_duration: 1_000_000,
          prompt_eval_count: 128,
          prompt_eval_duration: 2_000_000,
          eval_count: 24,
          eval_duration: Math.max(1, delayMs - 3) * 1_000_000,
        });
        return;
      }
      response.writeHead(404).end();
    } catch (error) {
      sendJson(response, { error: error instanceof Error ? error.message : String(error) }, 400);
    }
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  const address = server.address();
  if (address === null || typeof address === 'string') {
    throw new Error('Prompt Lab mock did not bind a TCP port.');
  }
  return {
    url: `http://127.0.0.1:${address.port}`,
    stats,
    close: () => new Promise((resolve, reject) => {
      server.close((error) => error === undefined ? resolve() : reject(error));
    }),
  };
}

function classifyScenario(prompt) {
  if (prompt.includes('[mock:internal-leak]')) {
    return 'internal_leak';
  }
  if (prompt.includes('[mock:invalid-evidence]')) {
    return 'invalid_evidence';
  }
  if (prompt.includes('[mock:timing]')) {
    return 'timing';
  }
  return 'valid';
}

function groundedMockResponse(prompt, scenario) {
  const manifestMatch = prompt.match(
    /\[CONTEXT_MANIFEST\]\n(.+?)\n\n\[AUTHORITATIVE_REFERENCES\]/su,
  );
  if (manifestMatch === null) {
    throw new Error('Grounded prompt has no machine-readable manifest.');
  }
  const manifest = JSON.parse(manifestMatch[1]);
  const entry = manifest.entries[0];
  const range = entry.ranges[0];
  const pathValue = scenario === 'invalid_evidence'
    ? 'src/main/java/dev/example/UnrelatedListener.java'
    : entry.path;
  const answer = scenario === 'internal_leak'
    ? 'cargo run -q -- inspect --path benchmarks/mini-bukkit-plugin'
    : 'Test';
  return {
    schema_version: 1,
    answer,
    claims: [
      {
        claim_id: 'mock-claim-1',
        text: 'Test',
        classification: 'observed',
        evidence: [
          {
            path: pathValue,
            start_line: range.start_line,
            end_line: range.start_line,
            content_hash: entry.injected_hash,
          },
        ],
      },
    ],
    missing_information: [],
    used_general_knowledge: false,
  };
}

async function readBoundedBody(request, limit) {
  const chunks = [];
  let total = 0;
  for await (const chunk of request) {
    total += chunk.length;
    if (total > limit) {
      throw new Error('Mock Ollama request exceeded its size limit.');
    }
    chunks.push(chunk);
  }
  return Buffer.concat(chunks).toString('utf8');
}

function sendJson(response, value, status = 200) {
  const body = JSON.stringify(value);
  response.writeHead(status, {
    'Content-Type': 'application/json',
    'Content-Length': Buffer.byteLength(body),
    Connection: 'close',
  });
  response.end(body);
}

async function removeTemporaryWorkspace(candidate) {
  const resolved = path.resolve(candidate);
  const temporaryRoot = `${path.resolve(os.tmpdir())}${path.sep}`;
  if (!resolved.startsWith(temporaryRoot) || !path.basename(resolved).startsWith('opticcode-prompt-lab-')) {
    throw new Error(`Refusing to remove unexpected Prompt Lab path: ${resolved}`);
  }
  await fs.rm(resolved, { recursive: true, force: true, maxRetries: 3 });
}
