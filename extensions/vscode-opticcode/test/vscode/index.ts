import * as assert from 'node:assert/strict';
import * as vscode from 'vscode';

import {
  OpticCodeChatHandler,
  type ChatReportSink,
  type ChatRuntimeService,
  type ChatStateSink,
} from '../../src/chat/participant';
import { ChatSessionStore, type MementoLike } from '../../src/chat/session';
import type { Finding } from '../../src/model';
import type {
  ChatCompletionSummary,
  ChatProtocolEvent,
  ChatProtocolRequest,
  ChatStreamResult,
} from '../../src/protocol/types';
import type { Connection } from '../../src/service';
import { SessionState } from '../../src/state';
import { RunsProvider, StatusProvider } from '../../src/views';

const PUBLIC_COMMANDS = [
  'opticcode.checkInstallation',
  'opticcode.refreshStatus',
  'opticcode.selectProfile',
  'opticcode.analyzeJavaProject',
  'opticcode.buildJavaSymbolIndex',
  'opticcode.buildSmartContext',
  'opticcode.proposeLegacyFixes',
  'opticcode.verifyProposedFixes',
  'opticcode.askQwen',
  'opticcode.planQwen',
  'opticcode.showLastReport',
  'opticcode.recoverWorktrees',
  'opticcode.openOutput',
] as const;

export async function run(): Promise<void> {
  const extension = vscode.extensions.getExtension('opticcode-local.opticcode');
  assert.ok(extension, 'OpticCode extension is installed in the development host');
  const api = (await extension.activate()) as { chatParticipantId?: unknown };
  assert.equal(extension.isActive, true);
  assert.equal(api.chatParticipantId, 'opticcode.chat');
  const commands = await vscode.commands.getCommands(true);
  for (const command of PUBLIC_COMMANDS) {
    assert.ok(commands.includes(command), `missing command ${command}`);
  }

  const state = new SessionState();
  const status = new StatusProvider(state);
  const runs = new RunsProvider(state);
  try {
    state.setStatus({
      executablePath: 'C:\\OpticCode\\opticcode.exe',
      opticcodeVersion: '0.1.0',
      protocolCompatible: true,
      doctor: {
        schema_version: 1,
        protocol: 'opticcode.discovery',
        success: false,
        workspace: 'C:\\workspace',
        profile: 'minecraft-java-1.8',
        model: 'qwen2.5-coder:14b',
        provider: 'ollama',
        checks: [
          {
            id: 'ollama_provider',
            status: 'error',
            required: true,
            summary: 'provider unavailable',
          },
        ],
      },
    });
    const statusItems = status.getChildren();
    assert.ok(statusItems.some((item) => item.label === 'Version' && item.value === '0.1.0'));
    assert.ok(
      statusItems.some(
        (item) => item.label === 'ollama provider' && item.value === 'provider unavailable',
      ),
    );

    state.addRun({
      command: 'Ask Qwen',
      requestId: 'ask-host-test',
      status: 'completed',
      durationMs: 1250,
      context: 'legacy',
      promptTokens: 100,
      generatedTokens: 20,
      model: 'qwen2.5-coder:14b',
      reportPath: 'C:\\reports\\ask.md',
    });
    const runItems = runs.getChildren();
    assert.equal(runItems.length, 1);
    const [runItemNode] = runItems;
    assert.ok(runItemNode);
    const runItem = runs.getTreeItem(runItemNode);
    assert.equal(runItem.label, 'Ask Qwen');
    assert.match(String(runItem.tooltip), /ask-host-test/);

    const workspace = vscode.workspace.workspaceFolders?.[0];
    assert.ok(workspace, 'test workspace is open');
    const file = vscode.Uri.joinPath(
      workspace.uri,
      'src',
      'main',
      'java',
      'dev',
      'opticcode',
      'app',
      'Plugin.java',
    );
    const finding: Finding = {
      id: 'host-open-range',
      kind: 'information',
      file: file.fsPath,
      range: {
        start: { line: 1, character: 0 },
        end: { line: 1, character: 4 },
      },
      message: 'host range',
      decision: 'informational',
      reason: 'extension host verification',
    };
    await vscode.commands.executeCommand('opticcode.openFinding', finding);
    const editor = vscode.window.activeTextEditor;
    assert.ok(editor, 'finding opens an editor');
    assert.equal(editor.document.uri.fsPath, file.fsPath);
    assert.equal(editor.selection.start.line, 1);
    assert.equal(editor.selection.start.character, 0);

    await testChatHandler(workspace);
  } finally {
    runs.dispose();
    status.dispose();
    state.dispose();
  }
}

class HostMemoryMemento implements MementoLike {
  private readonly values = new Map<string, unknown>();

  public get<T>(key: string, defaultValue: T): T {
    return (this.values.get(key) as T | undefined) ?? defaultValue;
  }

  public async update(key: string, value: unknown): Promise<void> {
    this.values.set(key, value);
  }
}

class FailingMemento implements MementoLike {
  public get<T>(_key: string, defaultValue: T): T {
    void _key;
    return defaultValue;
  }

  public async update(): Promise<void> {
    throw new Error('injected metadata failure');
  }
}

class HostChatResponse {
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
}

async function testChatHandler(workspace: vscode.WorkspaceFolder): Promise<void> {
  const requests: ChatProtocolRequest[] = [];
  const service: ChatRuntimeService = {
    connectForWorkspace: async () => fixtureConnection(workspace.uri.fsPath),
    runChat: async (request, _workspace, onEvent) => {
      requests.push(request);
      const result = fixtureChatResult(request);
      for (const event of result.events) {
        onEvent(event);
      }
      return result;
    },
  };
  const recordedRuns: string[] = [];
  const state: ChatStateSink = {
    setLastReport: () => {},
    addRun: (run) => recordedRuns.push(run.requestId),
  };
  const reports: ChatReportSink = {
    write: async (name) => `C:\\fixture-reports\\${name}`,
  };
  const output = { appendLine: (): void => {} };
  const handler = new OpticCodeChatHandler(
    '0.1.0',
    service,
    state,
    output,
    reports,
    new ChatSessionStore(new HostMemoryMemento()),
  );

  const cases: Array<{ command: string | undefined; prompt: string; expected: string }> = [
    { command: 'help', prompt: '', expected: 'help' },
    { command: 'status', prompt: '', expected: 'status' },
    { command: 'context', prompt: 'Locate Plugin.', expected: 'context' },
    { command: undefined, prompt: 'Explain Plugin.', expected: 'ask' },
  ];
  for (const testCase of cases) {
    const response = new HostChatResponse();
    const token = new vscode.CancellationTokenSource();
    try {
      const result = await handler.handle(
        {
          prompt: testCase.prompt,
          command: testCase.command,
          references: [],
          toolReferences: [],
        } as unknown as vscode.ChatRequest,
        { history: [] },
        response as unknown as vscode.ChatResponseStream,
        token.token,
      );
      assert.equal(result.errorDetails, undefined);
      assert.ok(response.markdownValues.join('').includes(`/${testCase.expected}`));
      assert.ok(response.progressValues.length > 0);
      assert.ok(response.buttons.some((button) => button.title === 'Show Full Report'));
      const protocolRequest = requests.at(-1);
      assert.equal(protocolRequest?.command, testCase.expected);
      assert.equal(protocolRequest?.security_mode, 'read_only');
      assert.equal(protocolRequest?.workspace_root, workspace.uri.fsPath);
    } finally {
      token.dispose();
    }
  }
  assert.equal(requests.length, 4);
  assert.equal(recordedRuns.length, 4);

  const failureResponse = new HostChatResponse();
  const failureHandler = new OpticCodeChatHandler(
    '0.1.0',
    service,
    state,
    output,
    { write: async () => Promise.reject(new Error('injected report failure')) },
    new ChatSessionStore(new FailingMemento()),
  );
  const failureToken = new vscode.CancellationTokenSource();
  try {
    const result = await failureHandler.handle(
      {
        prompt: '',
        command: 'help',
        references: [],
        toolReferences: [],
      } as unknown as vscode.ChatRequest,
      { history: [] },
      failureResponse as unknown as vscode.ChatResponseStream,
      failureToken.token,
    );
    assert.equal(result.errorDetails, undefined);
    assert.ok(failureResponse.markdownValues.join('').includes('response completed'));
  } finally {
    failureToken.dispose();
  }
}

function fixtureConnection(workspace: string): Connection {
  return {
    executable: {
      path: 'C:\\fixture\\opticcode.exe',
      workingDirectory: 'C:\\fixture',
      source: 'configured',
    },
    settings: {
      executablePath: 'C:\\fixture\\opticcode.exe',
      profile: 'minecraft-java-1.8',
      model: 'fixture-model',
      contextMode: 'symbol',
      timeoutSeconds: 30,
      showDebugOutput: false,
      autoCheckOnStartup: false,
    },
    workspace,
    version: {
      schema_version: 1,
      protocol: 'opticcode.discovery',
      opticcode_version: '0.1.0',
      protocols: {
        assistant: { id: 'opticcode.assistant', schema_version: 1 },
        chat: { id: 'opticcode.chat', schema_version: 1 },
        discovery: { id: 'opticcode.discovery', schema_version: 1 },
        llm: { id: 'opticcode.llm', schema_version: 1 },
      },
      schemas: {},
      platform: { os: 'windows', architecture: 'x86_64' },
      build: { kind: 'test' },
    },
    capabilities: {
      schema_version: 1,
      protocol: 'opticcode.discovery',
      commands: ['chat'],
      providers: [],
      context_modes: ['legacy', 'symbol', 'compare'],
      machine_output: { json: true, ndjson: true, streaming: true, cancellation: true },
      features: {
        chat: true,
        policy: true,
        rag: true,
        java: true,
        worktrees: true,
        verified_edits: true,
        evaluation: true,
      },
      policy_runtime: {
        schema_version: 1,
        policy_version: 'opticcode.default.v1',
        engine: true,
        modes: ['read_only', 'worktree_edit', 'approved_apply'],
        audit: true,
        approvals: true,
        cli: true,
        chat_read_only: true,
        chat_write: false,
      },
    },
    client: undefined as unknown as Connection['client'],
  };
}

function fixtureChatResult(request: ChatProtocolRequest): ChatStreamResult {
  const summary: ChatCompletionSummary = {
    command: request.command,
    success: true,
    model: request.model,
    requested_context_mode: request.context_mode,
    used_context_mode: request.context_mode,
    references: [],
    rejected_references: 0,
    context_files: [],
    warnings: [],
    metrics: {
      preparation_ms: 1,
      total_ms: 2,
      estimated_prompt_tokens: 3,
      prompt_tokens: 3,
      generated_tokens: 4,
      generated_tokens_per_second: 5,
    },
    repository_state: `state-${request.command}`,
    run_id: `run-${request.command}`,
  };
  const events: ChatProtocolEvent[] = [
    {
      schema_version: 1,
      protocol: 'opticcode.chat',
      request_id: request.request_id,
      sequence: 0,
      elapsed_ms: 0,
      type: 'request_accepted',
      command: request.command,
      requested_security_mode: 'read_only',
      security_mode: 'read_only',
      effective_security_mode: 'read_only',
      policy_version: 'opticcode.default.v1',
      policy_decision: 'allow',
      policy_rule_id: 'analysis.context_read_only',
    },
    {
      schema_version: 1,
      protocol: 'opticcode.chat',
      request_id: request.request_id,
      sequence: 1,
      elapsed_ms: 1,
      type: 'token_delta',
      text: `/${request.command} fixture response`,
    },
    {
      schema_version: 1,
      protocol: 'opticcode.chat',
      request_id: request.request_id,
      sequence: 2,
      elapsed_ms: 2,
      type: 'completed',
      summary,
    },
  ];
  const terminal = events[2];
  assert.ok(terminal);
  return {
    requestId: request.request_id,
    status: 'completed',
    response: `/${request.command} fixture response`,
    events,
    terminal,
    summary,
    durationMs: 2,
    exitCode: 0,
    stderr: '',
    cancellationConfirmed: false,
  };
}
