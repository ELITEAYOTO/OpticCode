import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';

import {
  buildChatRequest,
  collectWorkspaceReferences,
} from '../out/src/chat/model.js';
import { OpticCodeProtocolClient } from '../out/src/protocol/client.js';

const SOURCE_PATH = 'src/main/java/test/Plugin.java';
const SOURCE_BEFORE = [
  'package test;',
  'public final class Plugin {',
  '    public String message() { return "before"; }',
  '}',
  '',
].join('\n');
const SOURCE_AFTER = [
  'package test;',
  'public final class Plugin {',
  '    public String message() { return "after"; }',
  '}',
  '',
].join('\n');
const POM = [
  '<project xmlns="http://maven.apache.org/POM/4.0.0">',
  '  <modelVersion>4.0.0</modelVersion>',
  '  <groupId>test</groupId>',
  '  <artifactId>opticcode-real-edit-smoke</artifactId>',
  '  <version>1.0.0</version>',
  '  <properties>',
  '    <maven.compiler.source>1.8</maven.compiler.source>',
  '    <maven.compiler.target>1.8</maven.compiler.target>',
  '    <project.build.sourceEncoding>UTF-8</project.build.sourceEncoding>',
  '  </properties>',
  '</project>',
  '',
].join('\n');

const repository =
  process.env.OPTICCODE_REPO_ROOT ?? path.resolve(process.cwd(), '..', '..');
const executable =
  process.env.OPTICCODE_EXE ??
  path.join(
    repository,
    'target',
    'release',
    process.platform === 'win32' ? 'opticcode.exe' : 'opticcode',
  );
const model = process.env.OPTICCODE_MODEL ?? 'qwen2.5-coder:14b';
const ollamaUrl = process.env.OPTICCODE_OLLAMA_URL ?? 'http://127.0.0.1:11434';
const timeoutMs = 600_000;

await requireRegularFile(executable, 'OpticCode release executable');

const temporaryRoot = await fs.mkdtemp(
  path.join(os.tmpdir(), 'opticcode-real-edit-smoke-'),
);
const workspace = path.join(temporaryRoot, 'workspace');

try {
  await initializeFixture(workspace);

  const referenceCollection = collectWorkspaceReferences(workspace, [
    {
      referenceId: 'real-edit-source',
      inclusionReason: 'explicit real edit smoke source',
      kind: 'file',
      path: path.join(workspace, SOURCE_PATH),
    },
  ]);
  assert.deepEqual(referenceCollection.rejected, []);
  assert.equal(referenceCollection.accepted.length, 1);

  const request = buildChatRequest({
    command: 'fix',
    prompt:
      'Replace only the exact string literal content "before" with "after" in test.Plugin.message(). ' +
      'Modify only the explicitly referenced file. Do not create files and do not change formatting or any other text.',
    workspaceRoot: workspace,
    profile: 'none',
    model,
    contextMode: 'legacy',
    contextScope: 'references_only',
    scopeReason: 'explicit_setting',
    evidenceMode: 'optional',
    sessionId: `real-edit-${randomUUID()}`,
    clientVersion: '0.2.1',
    vscodeVersion: '1.125.0',
    locale: 'en',
    references: referenceCollection.accepted,
    history: [],
    recentRunIds: [],
    expectedProtocols: {
      chat: 1,
      assistant: 1,
      discovery: 1,
      llm: 1,
    },
    securityMode: 'read_only',
  });
  request.budgets.rag_hits = 0;
  request.generation.temperature = 0;
  request.generation.seed = 42;
  request.generation.brief = true;

  const client = new OpticCodeProtocolClient({
    executablePath: executable,
    workingDirectory: repository,
    timeoutMs,
  });

  const result = await client.runChatStream(
    [
      'chat',
      '--ollama-url',
      ollamaUrl,
      '--rag-index',
      path.join(repository, 'data', 'index'),
      '--http-timeout-ms',
      String(timeoutMs),
    ],
    request,
  );

  if (result.status !== 'completed') {
    throw new Error(
      `real /fix did not complete: ${JSON.stringify(result.terminal)}`,
    );
  }

  assertOrderedEvents(result.events, [
    'request_accepted',
    'references_resolved',
    'edit_intent_started',
    'edit_intent_ready',
    'edit_plan_started',
    'provider_started',
    'edit_plan_ready',
    'proposal_stored',
    'verification_started',
    'worktree_created',
    'edit_applied_in_worktree',
    'build_started',
    'build_completed',
    'diff_ready',
    'verification_completed',
    'approval_required',
    'completed',
  ]);

  const referencesResolved = requireEvent(result.events, 'references_resolved');
  assert.equal(referencesResolved.accepted.length, 1);
  assert.equal(referencesResolved.rejected.length, 0);
  assert.equal(referencesResolved.accepted[0]?.path, SOURCE_PATH);
  assert.equal(referencesResolved.accepted[0]?.reference_id, 'real-edit-source');

  const intentStarted = requireEvent(result.events, 'edit_intent_started');
  const intentReady = requireEvent(result.events, 'edit_intent_ready');
  assert.equal(intentReady.intent_id, intentStarted.intent_id);
  assert.equal(intentReady.intent_schema_version, 1);
  assert.match(intentReady.intent_hash, /^[0-9a-f]{64}$/u);
  assert.equal(intentReady.selection_mode, 'explicit_references');
  assert.equal(intentReady.target_count, 1);

  const planReady = requireEvent(result.events, 'edit_plan_ready');
  assert.equal(planReady.file_count, 1);

  const proposalStored = requireEvent(result.events, 'proposal_stored');
  assert.equal(proposalStored.intent_id, intentReady.intent_id);
  assert.equal(proposalStored.intent_schema_version, 1);
  assert.equal(proposalStored.intent_hash, intentReady.intent_hash);

  const diffReady = requireEvent(result.events, 'diff_ready');
  assert.equal(diffReady.proposal_id, proposalStored.proposal_id);
  assert.equal(diffReady.files, 1);
  assert.equal(diffReady.changes?.length, 1);
  const change = diffReady.changes?.[0];
  assert.ok(change !== undefined);
  assert.equal(change.path, SOURCE_PATH);
  assert.equal(change.status, 'modified');
  assert.equal(change.line_ending, 'lf');
  assert.equal(change.base_content, SOURCE_BEFORE);
  assert.equal(change.proposed_content, SOURCE_AFTER);
  assert.equal(change.additions, 1);
  assert.equal(change.deletions, 1);
  assert.match(diffReady.display_patch ?? '', /before/u);
  assert.match(diffReady.display_patch ?? '', /after/u);

  const verification = requireEvent(result.events, 'verification_completed');
  assert.equal(verification.proposal_id, proposalStored.proposal_id);
  assert.equal(verification.success, true);

  const approval = requireEvent(result.events, 'approval_required');
  assert.equal(approval.proposal_id, proposalStored.proposal_id);
  assert.equal(approval.operation, 'apply');
  assert.ok(approval.approval_request_id.length > 0);

  for (const forbiddenType of [
    'apply_started',
    'apply_completed',
    'rollback_started',
    'rollback_completed',
  ]) {
    assert.equal(
      result.events.some((event) => event.type === forbiddenType),
      false,
      `real /fix smoke must not emit ${forbiddenType}`,
    );
  }

  assert.equal(await fs.readFile(path.join(workspace, SOURCE_PATH), 'utf8'), SOURCE_BEFORE);
  assert.equal(git(workspace, ['status', '--porcelain=v1']).trim(), '');
  assert.equal(worktreeCount(workspace), 1);

  assert.equal(result.summary?.command, 'fix');
  assert.equal(result.summary?.success, true);
  assert.ok((result.summary?.metrics.prompt_tokens ?? 0) > 0);
  assert.ok((result.summary?.metrics.generated_tokens ?? 0) > 0);

  const corrected = result.events.some(
    (event) =>
      event.type === 'warning' && event.code === 'edit_plan_format_corrected',
  );

  process.stdout.write(
    [
      'fix: completed',
      `events=${String(result.events.length)}`,
      `proposal=${proposalStored.proposal_id}`,
      `intent=${intentReady.intent_id}`,
      `prompt_tokens=${String(result.summary?.metrics.prompt_tokens ?? 0)}`,
      `generated_tokens=${String(result.summary?.metrics.generated_tokens ?? 0)}`,
      `format_corrected=${String(corrected)}`,
      'source_unchanged=true',
      'worktrees=1',
    ].join(', ') + '\n',
  );
} finally {
  try {
    if (await pathExists(workspace)) {
      git(workspace, ['worktree', 'prune']);
    }
  } catch {
    // Best-effort cleanup only; assertions above are authoritative.
  }
  await fs.rm(temporaryRoot, {
    recursive: true,
    force: true,
    maxRetries: 5,
    retryDelay: 100,
  });
}

async function initializeFixture(workspace) {
  await fs.mkdir(path.join(workspace, 'src', 'main', 'java', 'test'), {
    recursive: true,
  });
  await fs.writeFile(path.join(workspace, SOURCE_PATH), SOURCE_BEFORE, 'utf8');
  await fs.writeFile(path.join(workspace, 'pom.xml'), POM, 'utf8');
  await fs.writeFile(
    path.join(workspace, '.gitignore'),
    ['target/', '.gradle/', 'build/', '.opticcode/', ''].join('\n'),
    'utf8',
  );
  await fs.writeFile(
    path.join(workspace, '.gitattributes'),
    ['* text eol=lf', ''].join('\n'),
    'utf8',
  );

  git(workspace, ['init', '--quiet']);
  git(workspace, ['config', 'core.autocrlf', 'false']);
  git(workspace, ['config', 'core.eol', 'lf']);
  git(workspace, ['add', '--all']);
  git(workspace, [
    '-c',
    'user.name=OpticCode Real Edit Smoke',
    '-c',
    'user.email=opticcode@example.invalid',
    'commit',
    '--quiet',
    '-m',
    'fixture',
  ]);

  assert.equal(git(workspace, ['status', '--porcelain=v1']).trim(), '');
  assert.equal(worktreeCount(workspace), 1);
}

function git(cwd, args) {
  return execFileSync('git', args, {
    cwd,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
    windowsHide: true,
  });
}

function worktreeCount(workspace) {
  return git(workspace, ['worktree', 'list', '--porcelain'])
    .split(/\r?\n/u)
    .filter((line) => line.startsWith('worktree ')).length;
}

function requireEvent(events, type) {
  const event = events.find((candidate) => candidate.type === type);
  assert.ok(event !== undefined, `missing required chat event ${type}`);
  return event;
}

function assertOrderedEvents(events, orderedTypes) {
  let previousIndex = -1;
  for (const type of orderedTypes) {
    const index = events.findIndex(
      (event, candidateIndex) =>
        candidateIndex > previousIndex && event.type === type,
    );
    assert.ok(index > previousIndex, `missing or out-of-order chat event ${type}`);
    previousIndex = index;
  }
}

async function requireRegularFile(candidate, label) {
  let stat;
  try {
    stat = await fs.stat(candidate);
  } catch {
    throw new Error(`${label} is missing: ${candidate}`);
  }
  if (!stat.isFile()) {
    throw new Error(`${label} is not a regular file: ${candidate}`);
  }
}

async function pathExists(candidate) {
  try {
    await fs.access(candidate);
    return true;
  } catch {
    return false;
  }
}