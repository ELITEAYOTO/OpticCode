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
  private connection: Connection | undefined;

  public constructor(
    private readonly extensionPath: string,
    private readonly output: vscode.OutputChannel,
  ) {}

  public invalidate(): void {
    this.connection = undefined;
  }

  public async connect(force = false): Promise<Connection> {
    if (!force && this.connection !== undefined) {
      return this.connection;
    }
    const folders = vscode.workspace.workspaceFolders ?? [];
    const primary = folders[0];
    if (primary === undefined) {
      throw new OpticCodeClientError(
        'executable_not_found',
        'Open a Java project folder before using OpticCode.',
      );
    }
    const settings = readSettings(primary.uri);
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
    this.connection = {
      executable,
      settings,
      workspace: primary.uri.fsPath,
      version,
      capabilities,
      client,
    };
    return this.connection;
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
}
