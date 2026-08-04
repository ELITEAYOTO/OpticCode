import * as assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import * as path from 'node:path';
import { performance } from 'node:perf_hooks';
import * as vscode from 'vscode';

import type { OpticCodeExtensionApi } from '../../src/extension';
import type {
  ChatClientTiming,
  ChatGroundingSummary,
  ChatMetrics,
  ChatUiTiming,
} from '../../src/protocol/types';

const STRICT_PLUGIN_PROMPT = [
  'Lis uniquement le fichier plugin.yml joint.',
  '',
  'Retourne seulement :',
  '1. la liste exacte de ses cles de premier niveau ;',
  '2. la ligne exacte contenant api-version, si elle existe ;',
  '3. sinon ecris exactement : "api-version absent".',
  '',
  "N'utilise aucune connaissance generale.",
  'Ne recommande aucune modification.',
  "Ne parle d'aucun autre fichier.",
  'Chaque affirmation doit citer une ligne reellement presente.',
].join('\n');

interface PromptLabRun {
  id: string;
  status: 'passed' | 'refused';
  wall_ms: number;
  context_fingerprint?: string;
  prompt_fingerprint?: string;
  route?: string;
  model_calls?: number;
  prompt_tokens?: number;
  estimated_prompt_tokens?: number;
  output_tokens?: number;
  provider_ms?: number;
  context_build_ms?: number;
  rust_total_ms?: number;
  process_total_ms?: number;
  ui_timing?: ChatUiTiming;
}

interface Invocation {
  result: vscode.ChatResult;
  response: RecordedChatResponse;
  wallMs: number;
  grounding?: ChatGroundingSummary;
}

export async function run(): Promise<void> {
  const mode = process.env['OPTICCODE_PROMPT_LAB_MODE'] ?? 'mock';
  const resultPath = process.env['OPTICCODE_PROMPT_LAB_RESULT'];
  assert.ok(resultPath, 'OPTICCODE_PROMPT_LAB_RESULT is required');
  const workspace = vscode.workspace.workspaceFolders?.[0];
  assert.ok(workspace, 'Prompt Lab fixture workspace is open');
  await configurePromptLab(workspace, mode);

  const extension = vscode.extensions.getExtension<OpticCodeExtensionApi>(
    'opticcode-local.opticcode',
  );
  assert.ok(extension, 'OpticCode extension is installed in the Extension Host');
  const api = await extension.activate();
  assert.equal(api.chatParticipantId, 'opticcode.chat');
  assert.ok(api.promptLabHandler, 'the exact registered Prompt Lab handler is exposed');
  const commands = await vscode.commands.getCommands(true);
  for (const command of [
    'opticcode.selectChatContextScope',
    'opticcode.clearChatSessionContext',
    'opticcode.internal.chat.showInjectedContext',
    'opticcode.internal.chat.showEvidence',
    'opticcode.internal.chat.copyGroundingReport',
  ]) {
    assert.ok(commands.includes(command), `missing Prompt Lab command ${command}`);
  }

  const plugin = vscode.Uri.joinPath(workspace.uri, 'src', 'main', 'resources', 'plugin.yml');
  const initialPlugin = await vscode.workspace.fs.readFile(plugin);
  const runs: PromptLabRun[] = [];
  let p1: Invocation | undefined;
  try {
    p1 = await invoke(api.promptLabHandler, STRICT_PLUGIN_PROMPT, plugin);
    assertSuccessfulDocumentFacts(p1, ['name', 'main', 'version', 'commands', 'api-version absent']);
    assertNoLeak(p1.response.text());
    assertGrounding(p1.grounding, 'document_facts');
    runs.push(runRecord('P1-exact-keys', p1));

    const history = [responseTurn(p1.result)];
    const p2 = await invoke(
      api.promptLabHandler,
      'Utilise uniquement ce fichier. Retourne seulement la valeur de main.',
      plugin,
      history,
    );
    assertSuccessfulDocumentFacts(p2, ['main = dev.example.OutilsEvolutif']);
    assert.equal(p2.grounding?.manifest.fingerprint, p1.grounding?.manifest.fingerprint);
    assert.notEqual(p2.grounding?.prompt_fingerprint, p1.grounding?.prompt_fingerprint);
    runs.push(runRecord('P2-new-task-same-session', p2));

    const changed = Buffer.concat([initialPlugin, Buffer.from('api-version: 1.13\n')]);
    await vscode.workspace.fs.writeFile(plugin, changed);
    const p3 = await invoke(api.promptLabHandler, STRICT_PLUGIN_PROMPT, plugin, history);
    assertSuccessfulDocumentFacts(p3, ['api-version: 1.13']);
    assert.notEqual(p3.grounding?.manifest.fingerprint, p1.grounding?.manifest.fingerprint);
    assert.notEqual(p3.grounding?.prompt_fingerprint, p1.grounding?.prompt_fingerprint);
    runs.push(runRecord('P3-same-path-new-hash', p3));
    await vscode.workspace.fs.writeFile(plugin, initialPlugin);

    const p4 = await invoke(api.promptLabHandler, STRICT_PLUGIN_PROMPT, plugin, history);
    assertSuccessfulDocumentFacts(p4, ['api-version absent']);
    assertNoLeak(p4.response.text());
    runs.push(runRecord('P4-no-java-leak', p4));

    const p5 = await invoke(
      api.promptLabHandler,
      'Utilise uniquement le fichier joint. Indique seulement si api-version existe.',
      plugin,
      history,
    );
    assertSuccessfulDocumentFacts(p5, ['api-version absent']);
    assert.doesNotMatch(p5.response.text(), /recommend|devriez|Bukkit|Spigot/iu);
    runs.push(runRecord('P5-no-general-knowledge', p5));

    if (mode === 'mock') {
      await runMockMatrix(api.promptLabHandler, plugin, history, runs);
    }
    if (mode === 'mock' || mode === 'holdout') {
      await runHoldouts(api.promptLabHandler, workspace, history, runs);
    }
    if (mode === 'qwen') {
      await runQwenCalibration(api.promptLabHandler, workspace, plugin, history, runs);
    }
  } finally {
    await vscode.workspace.fs.writeFile(plugin, initialPlugin);
  }

  const report = {
    schema_version: 1,
    suite: 'GROUNDING-METRICS-001',
    mode,
    integration: {
      registered_participant_tested: true,
      handler_tested_in_extension_host: true,
      actual_cli_transport_tested: true,
      real_model_tested: mode === 'qwen',
      visible_chat_input_automated: false,
    },
    thresholds: {
      task_compliance_percent: 100,
      cross_file_leakage_percent: 0,
      internal_context_leakage_percent: 0,
      invalid_citations_accepted: 0,
      stale_context_reuse: 0,
    },
    runs,
  };
  await fs.mkdir(path.dirname(resultPath), { recursive: true });
  await fs.writeFile(resultPath, `${JSON.stringify(report, null, 2)}\n`, 'utf8');
}

async function configurePromptLab(
  workspace: vscode.WorkspaceFolder,
  mode: string,
): Promise<void> {
  const root = process.env['OPTICCODE_PROMPT_LAB_ROOT'];
  assert.ok(root, 'OPTICCODE_PROMPT_LAB_ROOT is required');
  const executable = path.join(root, 'target', 'release', 'opticcode.exe');
  await fs.access(executable);
  const configuration = vscode.workspace.getConfiguration('opticcode', workspace.uri);
  const target = vscode.ConfigurationTarget.Workspace;
  await configuration.update('executablePath', executable, target);
  await configuration.update('model', mode === 'qwen' ? 'qwen2.5-coder:14b' : 'prompt-lab-mock', target);
  await configuration.update('contextMode', 'legacy', target);
  await configuration.update('chatContextScope', 'referencesOnly', target);
  await configuration.update('evidenceMode', 'required', target);
  await configuration.update('defaultTimeoutSeconds', 180, target);
  await configuration.update('autoCheckOnStartup', false, target);
}

async function runMockMatrix(
  handler: vscode.ChatRequestHandler,
  plugin: vscode.Uri,
  history: readonly vscode.ChatResponseTurn[],
  runs: PromptLabRun[],
): Promise<void> {
  const internal = await invoke(
    handler,
    '[mock:internal-leak] Utilise uniquement ce fichier et reponds a partir du texte Test.',
    plugin,
    history,
  );
  assert.ok(internal.result.errorDetails);
  assert.match(internal.response.text(), /Unauthorized internal context|Grounding refused/iu);
  assert.doesNotMatch(internal.response.text(), /cargo run -q --/iu);
  runs.push(runRecord('P6-internal-command-refused', internal, 'refused'));

  const invalidEvidence = await invoke(
    handler,
    '[mock:invalid-evidence] Utilise uniquement ce fichier et reponds a partir du texte Test.',
    plugin,
    history,
  );
  assert.ok(invalidEvidence.result.errorDetails);
  assert.match(invalidEvidence.response.text(), /Grounding refused|evidence/iu);
  assert.doesNotMatch(invalidEvidence.response.text(), /UnrelatedListener\.java/iu);
  runs.push(runRecord('P7-invalid-evidence-refused', invalidEvidence, 'refused'));

  const timing = await invoke(
    handler,
    '[mock:timing] Utilise uniquement ce fichier et retourne le texte Test.',
    plugin,
    history,
  );
  assert.equal(timing.result.errorDetails, undefined);
  const ui = metadataTiming(timing.result);
  assert.ok(ui.first_token_ms !== null && ui.first_token_ms >= 250);
  assert.ok(ui.visible_response_ms !== null);
  assert.ok(ui.first_token_ms <= ui.visible_response_ms);
  assert.ok(ui.visible_response_ms <= ui.total_pipeline_ms);
  assert.match(timing.response.text(), /First token:/u);
  assert.match(timing.response.text(), /Visible response:/u);
  assert.match(timing.response.text(), /Total pipeline:/u);
  assert.doesNotMatch(timing.response.text(), /\bDuration:/u);
  runs.push(runRecord('P8-bounded-timing', timing));
}

async function runHoldouts(
  handler: vscode.ChatRequestHandler,
  workspace: vscode.WorkspaceFolder,
  history: readonly vscode.ChatResponseTurn[],
  runs: PromptLabRun[],
): Promise<void> {
  const holdouts = vscode.Uri.joinPath(workspace.uri, 'holdouts');
  const cases: Array<{ id: string; file: string; prompt: string; expected: string[] }> = [
    {
      id: 'H1-nested-yaml',
      file: 'nested-commands.yml',
      prompt: 'Use only this file. Return only its top-level keys.',
      expected: ['name', 'metadata', 'enabled'],
    },
    {
      id: 'H2-json-absent',
      file: 'absent-key.json',
      prompt: 'Use only this file. Return its top-level keys and say whether api-version exists.',
      expected: ['name', 'enabled', 'limits', 'api-version absent'],
    },
    {
      id: 'H3-nested-toml',
      file: 'nested-table.toml',
      prompt: 'Use only this file. Return only its top-level keys.',
      expected: ['name', 'server'],
    },
    {
      id: 'H4-unicode-yaml',
      file: 'unicode.yml',
      prompt: 'Use only this file. Return only the value of `message`.',
      expected: ['message = Bienvenue a Volkaria'],
    },
  ];
  for (const testCase of cases) {
    const invocation = await invoke(
      handler,
      testCase.prompt,
      vscode.Uri.joinPath(holdouts, testCase.file),
      history,
    );
    assertSuccessfulDocumentFacts(invocation, testCase.expected, testCase.id === 'H4-unicode-yaml');
    runs.push(runRecord(testCase.id, invocation));
  }

  const selection = vscode.Uri.joinPath(holdouts, 'selection.yml');
  const selectedRange = new vscode.Range(new vscode.Position(1, 0), new vscode.Position(3, 0));
  const selected = await invoke(
    handler,
    'Use only this selection. Return only its top-level keys.',
    new vscode.Location(selection, selectedRange),
    history,
  );
  assertSuccessfulDocumentFacts(selected, ['selected_name', 'selected_value']);
  assert.doesNotMatch(selected.response.text(), /ignored|trailing/iu);
  runs.push(runRecord('H5-bounded-selection', selected));

  const [left, right] = await Promise.all([
    invoke(
      handler,
      'Use only this file. Return only its top-level keys.',
      vscode.Uri.joinPath(holdouts, 'nested-commands.yml'),
      history,
    ),
    invoke(
      handler,
      'Use only this file. Return only its top-level keys.',
      vscode.Uri.joinPath(holdouts, 'absent-key.json'),
      history,
    ),
  ]);
  assert.doesNotMatch(left.response.text(), /limits|ReserveJson/iu);
  assert.doesNotMatch(right.response.text(), /metadata|ReservePlugin/iu);
  assert.notEqual(left.grounding?.manifest.fingerprint, right.grounding?.manifest.fingerprint);
  runs.push(runRecord('P10-parallel-left', left), runRecord('P10-parallel-right', right));
}

async function runQwenCalibration(
  handler: vscode.ChatRequestHandler,
  workspace: vscode.WorkspaceFolder,
  plugin: vscode.Uri,
  history: readonly vscode.ChatResponseTurn[],
  runs: PromptLabRun[],
): Promise<void> {
  const legacy = vscode.Uri.joinPath(
    workspace.uri,
    'src',
    'main',
    'java',
    'dev',
    'example',
    'LegacyMaterial.java',
  );
  const material = await invoke(
    handler,
    [
      'Use only the attached file.',
      'Return only the exact code token containing Material.SULPHUR.',
      'Do not recommend changes and do not use general knowledge.',
    ].join('\n'),
    legacy,
    history,
  );
  assert.equal(material.result.errorDetails, undefined);
  assert.match(material.response.text(), /Material\.SULPHUR/u);
  assertNoLeak(material.response.text());
  assertGrounding(material.grounding, 'reference_llm');
  runs.push(runRecord('Q4-bukkit-sulphur-source-only', material));

  const insufficient = await invoke(
    handler,
    [
      'Use only the attached plugin.yml file.',
      'Which Java class implements the plugin main class?',
      'When the file does not prove the answer, report insufficient evidence.',
      'Do not use general knowledge.',
    ].join('\n'),
    plugin,
    history,
  );
  assert.equal(insufficient.result.errorDetails, undefined);
  assert.match(insufficient.response.text(), /insufficient|evidence|information/iu);
  assertNoLeak(insufficient.response.text());
  assertGrounding(insufficient.grounding, 'reference_llm');
  runs.push(runRecord('Q5-insufficient-evidence', insufficient));
}

async function invoke(
  handler: vscode.ChatRequestHandler,
  prompt: string,
  reference: vscode.Uri | vscode.Location,
  history: readonly vscode.ChatResponseTurn[] = [],
): Promise<Invocation> {
  const response = new RecordedChatResponse();
  const cancellation = new vscode.CancellationTokenSource();
  const started = performance.now();
  try {
    const result = await handler(
      {
        prompt,
        command: 'ask',
        references: [
          {
            id: 'prompt-lab-reference',
            value: reference,
            modelDescription: 'attached by Prompt Lab',
          },
        ],
        toolReferences: [],
      } as unknown as vscode.ChatRequest,
      { history } as vscode.ChatContext,
      response as unknown as vscode.ChatResponseStream,
      cancellation.token,
    );
    assert.ok(result, 'OpticCode handler returned a ChatResult');
    const grounding = response.grounding();
    return {
      result,
      response,
      wallMs: performance.now() - started,
      ...(grounding === undefined ? {} : { grounding }),
    };
  } finally {
    cancellation.dispose();
  }
}

class RecordedChatResponse {
  public readonly markdownValues: string[] = [];
  public readonly progressValues: string[] = [];
  public readonly references: Array<vscode.Uri | vscode.Location> = [];
  public readonly anchors: Array<vscode.Uri | vscode.Location> = [];
  public readonly buttons: vscode.Command[] = [];

  public markdown(value: string | vscode.MarkdownString): void {
    this.markdownValues.push(typeof value === 'string' ? value : value.value);
  }

  public progress(value: string): void {
    this.progressValues.push(value);
  }

  public reference(value: vscode.Uri | vscode.Location): void {
    this.references.push(value);
  }

  public anchor(value: vscode.Uri | vscode.Location): void {
    this.anchors.push(value);
  }

  public button(command: vscode.Command): void {
    this.buttons.push(command);
  }

  public filetree(): void {}
  public push(): void {}

  public text(): string {
    return this.markdownValues.join('');
  }

  public grounding(): ChatGroundingSummary | undefined {
    const button = this.buttons.find((candidate) => candidate.title === 'Show Injected Context');
    return button?.arguments?.[0] as ChatGroundingSummary | undefined;
  }
}

function responseTurn(result: vscode.ChatResult): vscode.ChatResponseTurn {
  const turn = Object.create(vscode.ChatResponseTurn.prototype) as vscode.ChatResponseTurn;
  Object.defineProperties(turn, {
    response: { value: [], enumerable: true },
    result: { value: result, enumerable: true },
    participant: { value: 'opticcode.chat', enumerable: true },
    command: { value: 'ask', enumerable: true },
  });
  return turn;
}

function assertSuccessfulDocumentFacts(
  invocation: Invocation,
  expected: readonly string[],
  stripDiacritics = false,
): void {
  assert.equal(invocation.result.errorDetails, undefined);
  const text = stripDiacritics
    ? invocation.response.text().normalize('NFD').replaceAll(/\p{Diacritic}/gu, '')
    : invocation.response.text();
  for (const value of expected) {
    assert.match(text, new RegExp(escapeRegExp(value), 'iu'));
  }
  assert.match(invocation.response.progressValues.join('\n'), /0 model call\(s\)/u);
  assertGrounding(invocation.grounding, 'document_facts');
}

function assertGrounding(
  grounding: ChatGroundingSummary | undefined,
  route: ChatGroundingSummary['route'],
): asserts grounding is ChatGroundingSummary {
  assert.ok(grounding);
  assert.equal(grounding.route, route);
  assert.equal(grounding.effective_scope, 'references_only');
  assert.equal(grounding.evidence_mode, 'required');
  assert.equal(grounding.selected_references, 1);
  assert.equal(grounding.resolved_references, 1);
  assert.equal(grounding.injected_references, 1);
  assert.equal(grounding.discovered_files, 0);
  assert.equal(grounding.rag_hits, 0);
  assert.equal(grounding.evidence?.valid, true);
  assert.equal(grounding.compliance?.compliant, true);
}

function assertNoLeak(text: string): void {
  assert.doesNotMatch(
    text,
    /UnrelatedListener|DeathProtectionListener|OtherCommand|cargo run|benchmark|RAG-SAFE/iu,
  );
}

function metadataTiming(result: vscode.ChatResult): ChatUiTiming {
  const metadata = result.metadata?.['opticcode'] as Record<string, unknown> | undefined;
  const timing = metadata?.['ui_timing'] as ChatUiTiming | undefined;
  assert.ok(timing);
  return timing;
}

function runRecord(
  id: string,
  invocation: Invocation,
  status: PromptLabRun['status'] = 'passed',
): PromptLabRun {
  const timing = metadataTiming(invocation.result);
  const metrics = metadataMetrics(invocation.result);
  const clientTiming = metadataClientTiming(invocation.result);
  const providerMs = metrics === undefined ? undefined : phaseDuration(metrics, 'provider_total');
  const contextBuildMs = metrics === undefined ? undefined : phaseDuration(metrics, 'context_build');
  return {
    id,
    status,
    wall_ms: Math.round(invocation.wallMs * 100) / 100,
    ...(invocation.grounding === undefined
      ? {}
      : {
          context_fingerprint: invocation.grounding.manifest.fingerprint,
          prompt_fingerprint: invocation.grounding.prompt_fingerprint,
          route: invocation.grounding.route,
          model_calls: invocation.grounding.route === 'document_facts' ? 0 : 1,
        }),
    ...(metrics === undefined
      ? {}
      : {
          prompt_tokens:
            invocation.grounding?.route === 'document_facts'
              ? 0
              : metrics.prompt_tokens ?? metrics.estimated_prompt_tokens,
          estimated_prompt_tokens: metrics.estimated_prompt_tokens,
          output_tokens: metrics.generated_tokens ?? 0,
          rust_total_ms: metrics.total_ms,
        }),
    ...(providerMs === undefined ? {} : { provider_ms: providerMs }),
    ...(contextBuildMs === undefined ? {} : { context_build_ms: contextBuildMs }),
    ...(clientTiming.process_completed_ms === null
      ? {}
      : { process_total_ms: clientTiming.process_completed_ms }),
    ui_timing: timing,
  };
}

function metadataMetrics(result: vscode.ChatResult): ChatMetrics | undefined {
  const metadata = result.metadata?.['opticcode'] as Record<string, unknown> | undefined;
  const metrics = metadata?.['chat_metrics'];
  return metrics === null || metrics === undefined ? undefined : metrics as ChatMetrics;
}

function metadataClientTiming(result: vscode.ChatResult): ChatClientTiming {
  const metadata = result.metadata?.['opticcode'] as Record<string, unknown> | undefined;
  const timing = metadata?.['client_timing'] as ChatClientTiming | undefined;
  assert.ok(timing);
  return timing;
}

function phaseDuration(metrics: ChatMetrics, name: string): number | undefined {
  return metrics.timing?.phases.find((phase) => phase.name === name)?.duration_ms;
}

function escapeRegExp(value: string): string {
  return value.replaceAll(/[.*+?^${}()|[\]\\]/g, '\\$&');
}
