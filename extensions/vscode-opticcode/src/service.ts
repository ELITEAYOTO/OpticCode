import * as path from 'node:path';
import * as vscode from 'vscode';

import { readSettings, type ExtensionSettings } from './configuration';
import {
  resolveOpticCodeExecutable,
  type ResolvedExecutable,
} from './executable';
import { OpticCodeClientError } from './protocol/errors';
import { OpticCodeProtocolClient } from './protocol/client';
import type {
  AssistantProtocolEvent,
  AssistantStreamResult,
  CapabilitiesReport,
  CancellationLike,
  ChatProtocolEvent,
  ChatProtocolRequest,
  ChatStreamResult,
  DoctorReport,
  JsonObject,
  VersionReport,
} from './protocol/types';
import {
  validateCapabilitiesReport,
  validateDoctorReport,
  validateVersionReport,
} from './protocol/validation';

export interface Connection {
  executable: ResolvedExecutable;
  settings: ExtensionSettings;
  workspace: string;
  version: VersionReport;
  capabilities: CapabilitiesReport;
  client: OpticCodeProtocolClient;
}

export class OpticCodeService {
  private readonly connections = new Map<string, Connection>();

  public constructor(
    private readonly extensionPath: string,
    private readonly output: vscode.OutputChannel,
  ) {}

  public invalidate(): void {
    this.connections.clear();
  }

  public async connect(force = false): Promise<Connection> {
    const folders = vscode.workspace.workspaceFolders ?? [];
    const primary = folders[0];
    if (primary === undefined) {
      throw new OpticCodeClientError(
        'executable_not_found',
        'Open a Java project folder before using OpticCode.',
      );
    }
    return await this.connectForWorkspace(primary.uri, force);
  }

  public async connectForWorkspace(
    workspace: vscode.Uri,
    force = false,
  ): Promise<Connection> {
    const folders = vscode.workspace.workspaceFolders ?? [];
    const folder = vscode.workspace.getWorkspaceFolder(workspace);
    if (folder === undefined) {
      throw new OpticCodeClientError(
        'protocol_incompatible',
        'The selected file is not inside an open workspace folder.',
      );
    }
    const workspaceKey = normalizedWorkspaceKey(folder.uri.fsPath);
    const cached = this.connections.get(workspaceKey);
    if (!force && cached !== undefined) {
      return cached;
    }
    const settings = readSettings(folder.uri);
    if (settings.profile === '' || settings.model === '') {
      throw new OpticCodeClientError(
        'protocol_incompatible',
        'OpticCode profile and model settings must not be empty.',
      );
    }
    const executable = await resolveOpticCodeExecutable(
      settings.executablePath,
      folders.map((folder) => folder.uri.fsPath),
      this.extensionPath,
    );
    const client = new OpticCodeProtocolClient({
      executablePath: executable.path,
      workingDirectory: executable.workingDirectory,
      timeoutMs: settings.timeoutSeconds * 1000,
      logger: this.output,
      debug: settings.showDebugOutput,
    });
    const version = await client.runJson(['version', '--json'], validateVersionReport);
    const capabilities = await client.runJson(
      ['capabilities', '--json'],
      validateCapabilitiesReport,
    );
    const connection = {
      executable,
      settings,
      workspace: folder.uri.fsPath,
      version,
      capabilities,
      client,
    };
    this.connections.set(workspaceKey, connection);
    return connection;
  }

  public async doctor(): Promise<DoctorReport> {
    const connection = await this.connect();
    return await connection.client.runJson(
      [
        'doctor',
        '--json',
        '--path',
        connection.workspace,
        '--profile',
        connection.settings.profile,
        '--model',
        connection.settings.model,
        '--rag-index',
        path.join(connection.executable.workingDirectory, 'data', 'index'),
        '--timeout-ms',
        String(Math.min(connection.settings.timeoutSeconds * 1000, 30_000)),
      ],
      validateDoctorReport,
    );
  }

  public async runJson(
    args: readonly string[],
    cancellation?: CancellationLike,
    acceptNonZero = false,
  ): Promise<JsonObject> {
    const connection = await this.connect();
    return await connection.client.runJsonObject(args, cancellation, acceptNonZero);
  }

  public async runAssistant(
    command: 'ask' | 'plan',
    request: string,
    requestId: string,
    onEvent: (event: AssistantProtocolEvent) => void,
    cancellation?: CancellationLike,
  ): Promise<AssistantStreamResult> {
    const connection = await this.connect();
    return await connection.client.runAssistantStream(
      [
        command,
        request,
        '--path',
        connection.workspace,
        '--profile',
        connection.settings.profile,
        '--model',
        connection.settings.model,
        '--context-mode',
        connection.settings.contextMode,
        '--rag-index',
        path.join(connection.executable.workingDirectory, 'data', 'index'),
        '--http-timeout-ms',
        String(connection.settings.timeoutSeconds * 1000),
      ],
      requestId,
      onEvent,
      cancellation,
    );
  }

  public async runChat(
    request: ChatProtocolRequest,
    workspace: vscode.Uri,
    onEvent: (event: ChatProtocolEvent) => void,
    cancellation?: CancellationLike,
  ): Promise<ChatStreamResult> {
    const connection = await this.connectForWorkspace(workspace);
    if (
      normalizedWorkspaceKey(request.workspace_root) !==
      normalizedWorkspaceKey(connection.workspace)
    ) {
      throw new OpticCodeClientError(
        'protocol_incompatible',
        'Chat request workspace does not match its OpticCode connection.',
      );
    }
    return await connection.client.runChatStream(
      [
        'chat',
        ...promptLabOllamaArguments(),
        '--rag-index',
        path.join(connection.executable.workingDirectory, 'data', 'index'),
        '--http-timeout-ms',
        String(connection.settings.timeoutSeconds * 1000),
      ],
      request,
      onEvent,
      cancellation,
    );
  }
}

function promptLabOllamaArguments(): string[] {
  const value = process.env['OPTICCODE_PROMPT_LAB_OLLAMA_URL'];
  if (process.env['OPTICCODE_PROMPT_LAB'] !== '1' || value === undefined) {
    return [];
  }
  let endpoint: URL;
  try {
    endpoint = new URL(value);
  } catch {
    throw new OpticCodeClientError(
      'protocol_incompatible',
      'The Prompt Lab Ollama endpoint is not a valid URL.',
    );
  }
  const loopback = new Set(['localhost', '127.0.0.1', '::1']);
  if (endpoint.protocol !== 'http:' || !loopback.has(endpoint.hostname)) {
    throw new OpticCodeClientError(
      'protocol_incompatible',
      'The Prompt Lab Ollama endpoint must use HTTP on the local loopback interface.',
    );
  }
  return ['--ollama-url', endpoint.toString().replace(/\/$/u, '')];
}

function normalizedWorkspaceKey(workspace: string): string {
  const normalized = path.resolve(workspace);
  return process.platform === 'win32' ? normalized.toLocaleLowerCase('en-US') : normalized;
}
