import { randomUUID } from 'node:crypto';
import * as path from 'node:path';
import * as vscode from 'vscode';

import type { RunRecord } from '../model';
import { OpticCodeClientError } from '../protocol/errors';
import type {
  ChatCommand,
  ChatHistoryTurn,
  ChatProtocolEvent,
  ChatProtocolRequest,
  ChatReference,
  ChatStreamResult,
  ChatTextRange,
} from '../protocol/types';
import { ReportStore } from '../reports';
import type { Connection, OpticCodeService } from '../service';
import type { SessionState } from '../state';
import {
  boundChatHistory,
  buildChatRequest,
  CHAT_PARTICIPANT_ID,
  collectWorkspaceReferences,
  parseChatCommand,
  sessionNamespace,
  workspaceIdentity,
  type NeutralHistoryTurn,
  type ReferenceCandidate,
} from './model';
import { ChatEventPresenter, type ChatRenderOperation } from './presentation';
import { ChatEditReviewController } from './editReview';
import { ChatSessionStore, type ChatSessionMetadata } from './session';

const INTERNAL_COMMANDS = {
  showContext: 'opticcode.internal.chat.showContext',
  showReport: 'opticcode.internal.chat.showReport',
  showDiff: 'opticcode.internal.chat.showDiff',
  showAllChanges: 'opticcode.internal.chat.showAllChanges',
  applyProposal: 'opticcode.internal.chat.applyProposal',
  discardProposal: 'opticcode.internal.chat.discardProposal',
  rollbackTransaction: 'opticcode.internal.chat.rollbackTransaction',
} as const;

const EDIT_COMMANDS = new Set<ChatCommand>(['fix', 'verify', 'diff', 'apply', 'rollback']);

export interface ChatRegistration {
  readonly participantId: string;
  dispose(): void;
}

export interface ChatRuntimeService {
  connectForWorkspace(workspace: vscode.Uri, force?: boolean): Promise<Connection>;
  runChat(
    request: ChatProtocolRequest,
    workspace: vscode.Uri,
    onEvent: (event: ChatProtocolEvent) => void,
    cancellation?: vscode.CancellationToken,
  ): Promise<ChatStreamResult>;
}

export interface ChatStateSink {
  setLastReport(title: string, content: string, reportPath?: string): void;
  addRun(run: RunRecord): void;
}

export interface ChatReportSink {
  write(name: string, content: string): Promise<string>;
}

export function registerOpticCodeChat(
  extensionContext: vscode.ExtensionContext,
  service: OpticCodeService,
  state: SessionState,
  output: vscode.OutputChannel,
): ChatRegistration {
  const reports = new ReportStore(extensionContext.globalStorageUri);
  const sessions = new ChatSessionStore(extensionContext.globalState);
  const subscriptions: vscode.Disposable[] = [];
  const clientVersion = extensionVersion(extensionContext);
  const editReview = new ChatEditReviewController(clientVersion, service, output);
  const handler = new OpticCodeChatHandler(
    clientVersion,
    service,
    state,
    output,
    reports,
    sessions,
    editReview,
  );
  const participant = vscode.chat.createChatParticipant(
    CHAT_PARTICIPANT_ID,
    (request, context, response, token) => handler.handle(request, context, response, token),
  );
  participant.iconPath = vscode.Uri.joinPath(extensionContext.extensionUri, 'media', 'opticcode.svg');
  subscriptions.push(participant);
  subscriptions.push(editReview);

  const showStoredReport = async (runId: unknown): Promise<void> => {
    if (typeof runId !== 'string') {
      await vscode.window.showInformationMessage('No OpticCode report was selected.');
      return;
    }
    const session = sessions.findByRunId(runId);
    if (session?.lastReportPath === undefined) {
      await vscode.window.showInformationMessage('This OpticCode report is no longer available.');
      return;
    }
    await reports.showPath(session.lastReportPath);
  };
  subscriptions.push(
    vscode.commands.registerCommand(INTERNAL_COMMANDS.showContext, showStoredReport),
    vscode.commands.registerCommand(INTERNAL_COMMANDS.showReport, showStoredReport),
    vscode.commands.registerCommand(INTERNAL_COMMANDS.showDiff, (proposalId) =>
      editReview.showDiff(proposalId),
    ),
    vscode.commands.registerCommand(INTERNAL_COMMANDS.showAllChanges, (proposalId) =>
      editReview.showAllChanges(proposalId),
    ),
    vscode.commands.registerCommand(
      INTERNAL_COMMANDS.applyProposal,
      (proposalId, approvalRequestId) =>
        editReview.applyProposal(proposalId, approvalRequestId),
    ),
    vscode.commands.registerCommand(INTERNAL_COMMANDS.discardProposal, (proposalId) =>
      editReview.discardProposal(proposalId),
    ),
    vscode.commands.registerCommand(
      INTERNAL_COMMANDS.rollbackTransaction,
      (proposalOrTransaction, approvalOrTransaction) =>
        editReview.rollbackTransaction(proposalOrTransaction, approvalOrTransaction),
    ),
  );

  return {
    participantId: CHAT_PARTICIPANT_ID,
    dispose: () => {
      for (const subscription of subscriptions) {
        subscription.dispose();
      }
    },
  };
}

export class OpticCodeChatHandler {
  public constructor(
    private readonly clientVersion: string,
    private readonly service: ChatRuntimeService,
    private readonly state: ChatStateSink,
    private readonly output: Pick<vscode.OutputChannel, 'appendLine'>,
    private readonly reports: ChatReportSink,
    private readonly sessions: ChatSessionStore,
    private readonly editReview?: Pick<ChatEditReviewController, 'observe'>,
  ) {}

  public async handle(
    request: vscode.ChatRequest,
    context: vscode.ChatContext,
    response: vscode.ChatResponseStream,
    token: vscode.CancellationToken,
  ): Promise<vscode.ChatResult> {
    const command = parseChatCommand(request.command);
    if (command === undefined) {
      response.markdown(
        `Unknown OpticCode command \`/${escapeMarkdown(request.command ?? '')}\`. Use \`@opticcode /help\` to list supported commands.`,
      );
      return {
        errorDetails: { message: `Unknown OpticCode command /${request.command ?? ''}.` },
        metadata: { opticcode: { schema_version: 1, status: 'rejected_unknown_command' } },
      };
    }
    const workspace = selectWorkspace(request);
    if (workspace === undefined) {
      response.markdown('Open a workspace folder before using `@opticcode`.');
      return { errorDetails: { message: 'No workspace folder is open.' } };
    }

    try {
      response.progress('Connecting to the local OpticCode runtime...');
      const connection = await this.service.connectForWorkspace(workspace.uri);
      const workspaceId = workspaceIdentity(connection.workspace);
      const participantSession = sessionIdFromHistory(context.history) ?? randomUUID();
      const stored = this.sessions.get(workspaceId, participantSession);
      const namespace = sessionNamespace(
        connection.workspace,
        stored?.repositoryState,
        participantSession,
      );
      const history = historyFromVscode(context.history);
      const references = referencesFromVscode(request, workspace, command);
      for (const rejection of references.rejected) {
        response.markdown(
          `\n> **Reference ignored:** ${escapeMarkdown(rejection.reason)}\n`,
        );
      }
      const protocolRequest = this.protocolRequest(
        request,
        command,
        connection,
        namespace,
        history,
        references.accepted,
        stored,
      );
      const presenter = new ChatEventPresenter();
      const result = await this.service.runChat(
        protocolRequest,
        workspace.uri,
        (event) => {
          this.editReview?.observe(connection.workspace, event);
          this.render(response, workspace, presenter.accept(event));
        },
        token,
      );
      let reportPath: string | undefined;
      try {
        reportPath = await this.recordResult(
          command,
          workspaceId,
          namespace,
          result,
          references.rejected,
        );
      } catch (error) {
        this.output.appendLine(`[chat] report storage failed: ${errorMessage(error)}`);
        response.markdown(
          '\n\n> **Warning:** The response completed, but its full report could not be stored.\n',
        );
      }
      const runId = result.summary?.run_id ?? result.requestId;
      const recentRunIds = [
        runId,
        ...(stored?.recentRunIds ?? []).filter((candidate) => candidate !== runId),
      ].slice(0, 16);
      const lastProposalId = latestProposalId(result.events) ?? stored?.lastProposalId;
      const lastTransactionId = latestTransactionId(result.events) ?? stored?.lastTransactionId;
      try {
        await this.sessions.record({
          schemaVersion: 1,
          namespace,
          workspaceId,
          sessionId: participantSession,
          ...(result.summary?.repository_state === undefined
            ? {}
            : { repositoryState: result.summary.repository_state }),
          recentRunIds,
          ...(reportPath === undefined ? {} : { lastReportPath: reportPath }),
          ...(lastProposalId === undefined ? {} : { lastProposalId }),
          ...(lastTransactionId === undefined ? {} : { lastTransactionId }),
          updatedAt: new Date().toISOString(),
        });
      } catch (error) {
        this.output.appendLine(`[chat] session metadata storage failed: ${errorMessage(error)}`);
        response.markdown(
          '\n\n> **Warning:** The response completed, but session metadata could not be stored.\n',
        );
      }
      return {
        ...(result.status === 'failed'
          ? { errorDetails: { message: terminalFailure(result) } }
          : {}),
        metadata: {
          opticcode: {
            schema_version: 1,
            request_id: result.requestId,
            run_id: runId,
            status: result.status,
            workspace_id: workspaceId,
            session_id: participantSession,
            repository_state: result.summary?.repository_state ?? null,
            command,
          },
        },
      };
    } catch (error) {
      const message = errorMessage(error);
      this.output.appendLine(`[chat] /${command}: ${message}`);
      if (token.isCancellationRequested) {
        response.markdown('\n\n_OpticCode cancellation was requested._');
      } else {
        response.markdown(`\n\n**OpticCode error:** ${escapeMarkdown(message)}`);
      }
      return {
        errorDetails: { message },
        metadata: {
          opticcode: {
            schema_version: 1,
            status: token.isCancellationRequested ? 'cancellation_error' : 'failed',
            command,
          },
        },
      };
    }
  }

  private protocolRequest(
    request: vscode.ChatRequest,
    command: ChatCommand,
    connection: Connection,
    namespace: string,
    history: readonly ChatHistoryTurn[],
    references: readonly ChatReference[],
    stored: ChatSessionMetadata | undefined,
  ): ChatProtocolRequest {
    return buildChatRequest({
      command,
      prompt: request.prompt,
      workspaceRoot: connection.workspace,
      profile: connection.settings.profile,
      model: connection.settings.model,
      contextMode: connection.settings.contextMode,
      sessionId: namespace,
      clientVersion: this.clientVersion,
      vscodeVersion: vscode.version,
      locale: vscode.env.language,
      references,
      history,
      recentRunIds: stored?.recentRunIds ?? [],
      previousRepositoryState: stored?.repositoryState,
      expectedProtocols: {
        chat: connection.version.protocols.chat.schema_version,
        assistant: connection.version.protocols.assistant.schema_version,
        discovery: connection.version.protocols.discovery.schema_version,
        llm: connection.version.protocols.llm.schema_version,
      },
      securityMode: 'read_only',
    });
  }

  private render(
    response: vscode.ChatResponseStream,
    workspace: vscode.WorkspaceFolder,
    operations: readonly ChatRenderOperation[],
  ): void {
    for (const operation of operations) {
      switch (operation.kind) {
        case 'progress':
          response.progress(operation.text);
          break;
        case 'markdown':
          response.markdown(operation.text);
          break;
        case 'reference': {
          const target = safeLocation(workspace.uri, operation.path, operation.range);
          if (target !== undefined) {
            response.reference(target);
          }
          break;
        }
        case 'anchor': {
          const target = safeLocation(workspace.uri, operation.path, operation.range);
          if (target !== undefined) {
            response.anchor(target, operation.title);
          }
          break;
        }
        case 'filetree': {
          const paths = operation.paths.filter(
            (candidate) => safeWorkspaceUri(workspace.uri, candidate) !== undefined,
          );
          if (paths.length !== 0) {
            response.filetree(
              paths.map((candidate) => ({ name: candidate })),
              workspace.uri,
            );
          }
          break;
        }
        case 'button':
          response.button({
            command: operation.command,
            title: operation.title,
            ...(operation.arguments === undefined ? {} : { arguments: operation.arguments }),
          });
          break;
      }
    }
  }

  private async recordResult(
    command: ChatCommand,
    workspaceId: string,
    namespace: string,
    result: ChatStreamResult,
    rejectedReferences: readonly { referenceId: string; reason: string }[],
  ): Promise<string> {
    const report = chatReport(command, workspaceId, namespace, result, rejectedReferences);
    const reportPath = await this.reports.write(
      `chat-${command}-${timestamp()}-${result.requestId.slice(-12)}.md`,
      report,
    );
    this.state.setLastReport(`OpticCode Chat /${command}`, report, reportPath);
    const run: RunRecord = {
      command: `Chat /${command}`,
      requestId: result.requestId,
      status: result.status,
      durationMs: result.durationMs,
      context: result.summary?.used_context_mode ?? result.summary?.requested_context_mode,
      promptTokens: result.summary?.metrics.prompt_tokens ?? undefined,
      generatedTokens: result.summary?.metrics.generated_tokens ?? undefined,
      model: result.summary?.model,
      reportPath,
      workspaceId,
    };
    this.state.addRun(run);
    return reportPath;
  }
}

function selectWorkspace(request: vscode.ChatRequest): vscode.WorkspaceFolder | undefined {
  for (const reference of request.references) {
    const uri = referenceUri(reference.value);
    if (uri !== undefined) {
      const workspace = vscode.workspace.getWorkspaceFolder(uri);
      if (workspace !== undefined) {
        return workspace;
      }
    }
  }
  const active = vscode.window.activeTextEditor?.document.uri;
  if (active !== undefined) {
    const workspace = vscode.workspace.getWorkspaceFolder(active);
    if (workspace !== undefined) {
      return workspace;
    }
  }
  return vscode.workspace.workspaceFolders?.[0];
}

function referencesFromVscode(
  request: vscode.ChatRequest,
  workspace: vscode.WorkspaceFolder,
  command: ChatCommand,
): ReturnType<typeof collectWorkspaceReferences> {
  const candidates: ReferenceCandidate[] = [];
  for (const [index, reference] of request.references.entries()) {
    const candidate = candidateFromPromptReference(reference, index);
    if (candidate !== undefined) {
      candidates.push(candidate);
    }
  }
  if (usesEditorContext(command)) {
    const editor = vscode.window.activeTextEditor;
    if (
      editor !== undefined &&
      editor.document.uri.scheme === 'file' &&
      vscode.workspace.getWorkspaceFolder(editor.document.uri)?.uri.toString() ===
        workspace.uri.toString()
    ) {
      const editorPath = editor.document.uri.fsPath;
      const alreadyPrecise = candidates.some(
        (candidate) =>
          candidate.path !== undefined &&
          equalPath(candidate.path, editorPath) &&
          candidate.range !== undefined,
      );
      const alreadyWhole = candidates.some(
        (candidate) =>
          candidate.path !== undefined &&
          equalPath(candidate.path, editorPath) &&
          candidate.range === undefined,
      );
      if (!editor.selection.isEmpty && !alreadyPrecise) {
        candidates.push({
          referenceId: 'active-selection',
          inclusionReason: 'active editor selection',
          kind: 'selection',
          path: editorPath,
          range: toChatRange(editor.selection),
        });
      } else if (editor.selection.isEmpty && !alreadyPrecise && !alreadyWhole) {
        candidates.push({
          referenceId: 'active-file',
          inclusionReason: 'active editor file',
          kind: 'active_file',
          path: editorPath,
        });
      }
    }
  }
  return collectWorkspaceReferences(workspace.uri.fsPath, candidates);
}

function candidateFromPromptReference(
  reference: vscode.ChatPromptReference,
  index: number,
): ReferenceCandidate | undefined {
  const inclusionReason = boundedDescription(reference.modelDescription) ?? 'attached by user';
  if (reference.value instanceof vscode.Uri) {
    if (reference.value.scheme !== 'file') {
      return undefined;
    }
    return {
      referenceId: `attached-file-${index}`,
      inclusionReason,
      kind: 'file',
      path: reference.value.fsPath,
    };
  }
  if (reference.value instanceof vscode.Location) {
    if (reference.value.uri.scheme !== 'file') {
      return undefined;
    }
    return {
      referenceId: `attached-range-${index}`,
      inclusionReason,
      kind: 'range',
      path: reference.value.uri.fsPath,
      range: toChatRange(reference.value.range),
    };
  }
  const structured = structuredReference(reference.value, index, inclusionReason);
  return structured;
}

function structuredReference(
  value: unknown,
  index: number,
  inclusionReason: string,
): ReferenceCandidate | undefined {
  if (!isRecord(value) || typeof value.kind !== 'string') {
    return undefined;
  }
  const base = {
    referenceId: `opticcode-reference-${index}`,
    inclusionReason,
  };
  if (value.kind === 'run' && typeof value.run_id === 'string') {
    return { ...base, kind: 'run', runId: value.run_id };
  }
  if (value.kind === 'diff' && typeof value.proposal_id === 'string') {
    return { ...base, kind: 'diff', proposalId: value.proposal_id };
  }
  if (value.kind === 'finding' && typeof value.finding_id === 'string') {
    return {
      ...base,
      kind: 'finding',
      findingId: value.finding_id,
      ...(typeof value.path === 'string' ? { path: value.path } : {}),
      ...(isChatRange(value.range) ? { range: value.range } : {}),
    };
  }
  return undefined;
}

function historyFromVscode(
  history: readonly (vscode.ChatRequestTurn | vscode.ChatResponseTurn)[],
): ChatHistoryTurn[] {
  const candidates: NeutralHistoryTurn[] = [];
  for (const turn of history) {
    if (turn instanceof vscode.ChatRequestTurn) {
      candidates.push({
        role: 'user',
        content: turn.prompt,
        command: turn.command,
      });
      continue;
    }
    if (turn instanceof vscode.ChatResponseTurn) {
      const markdown = turn.response
        .filter((part): part is vscode.ChatResponseMarkdownPart =>
          part instanceof vscode.ChatResponseMarkdownPart,
        )
        .map((part) => part.value.value)
        .join('\n');
      candidates.push({
        role: 'assistant',
        content: markdown,
        command: turn.command,
        resultId: resultMetadata(turn.result)?.run_id,
      });
    }
  }
  return boundChatHistory(candidates);
}

function sessionIdFromHistory(
  history: readonly (vscode.ChatRequestTurn | vscode.ChatResponseTurn)[],
): string | undefined {
  for (let index = history.length - 1; index >= 0; index -= 1) {
    const turn = history[index];
    if (turn instanceof vscode.ChatResponseTurn) {
      const sessionId = resultMetadata(turn.result)?.session_id;
      if (typeof sessionId === 'string' && /^[a-zA-Z0-9._:-]{1,128}$/.test(sessionId)) {
        return sessionId;
      }
    }
  }
  return undefined;
}

function resultMetadata(result: vscode.ChatResult): Record<string, unknown> | undefined {
  const metadata: unknown = result.metadata;
  if (!isRecord(metadata) || !isRecord(metadata.opticcode)) {
    return undefined;
  }
  return metadata.opticcode;
}

function safeLocation(
  workspace: vscode.Uri,
  candidate: string,
  range: ChatTextRange | undefined,
): vscode.Uri | vscode.Location | undefined {
  const uri = safeWorkspaceUri(workspace, candidate);
  if (uri === undefined || range === undefined) {
    return uri;
  }
  return new vscode.Location(uri, new vscode.Range(toPosition(range.start), toPosition(range.end)));
}

function safeWorkspaceUri(workspace: vscode.Uri, candidate: string): vscode.Uri | undefined {
  if (workspace.scheme !== 'file') {
    return undefined;
  }
  const root = path.resolve(workspace.fsPath);
  const absolute = path.isAbsolute(candidate)
    ? path.resolve(candidate)
    : path.resolve(root, candidate);
  const relative = path.relative(root, absolute);
  if (relative === '..' || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) {
    return undefined;
  }
  return vscode.Uri.file(absolute);
}

function referenceUri(value: unknown): vscode.Uri | undefined {
  if (value instanceof vscode.Uri) {
    return value;
  }
  return value instanceof vscode.Location ? value.uri : undefined;
}

function toChatRange(range: vscode.Range): ChatTextRange {
  return {
    start: { line: range.start.line, character: range.start.character },
    end: { line: range.end.line, character: range.end.character },
  };
}

function toPosition(position: { line: number; character: number }): vscode.Position {
  return new vscode.Position(position.line, position.character);
}

function usesEditorContext(command: ChatCommand): boolean {
  return !['help', 'status', 'runs', 'index', 'rollback'].includes(command);
}

function extensionVersion(context: vscode.ExtensionContext): string {
  const packageJson: unknown = context.extension.packageJSON;
  return isRecord(packageJson) && typeof packageJson.version === 'string'
    ? packageJson.version.slice(0, 64)
    : 'unknown';
}

function boundedDescription(value: string | undefined): string | undefined {
  const trimmed = value?.trim();
  return trimmed === undefined || trimmed === '' ? undefined : trimmed.slice(0, 512);
}

function isChatRange(value: unknown): value is ChatTextRange {
  if (!isRecord(value) || !isRecord(value.start) || !isRecord(value.end)) {
    return false;
  }
  return (
    typeof value.start.line === 'number' &&
    typeof value.start.character === 'number' &&
    typeof value.end.line === 'number' &&
    typeof value.end.character === 'number'
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function equalPath(left: string, right: string): boolean {
  const normalizedLeft = path.resolve(left);
  const normalizedRight = path.resolve(right);
  return process.platform === 'win32'
    ? normalizedLeft.toLocaleLowerCase('en-US') === normalizedRight.toLocaleLowerCase('en-US')
    : normalizedLeft === normalizedRight;
}

function chatReport(
  command: ChatCommand,
  workspaceId: string,
  namespace: string,
  result: ChatStreamResult,
  rejectedReferences: readonly { referenceId: string; reason: string }[],
): string {
  const eventSummary = result.events.map((event) =>
    event.type === 'token_delta'
      ? { ...event, text: `[${event.text.length} streamed characters]` }
      : event,
  );
  return [
    `# OpticCode Chat /${command}`,
    '',
    result.response === '' ? '*No generated response.*' : result.response,
    '',
    '## Execution',
    '',
    '```json',
    JSON.stringify(
      {
        schema_version: 1,
        request_id: result.requestId,
        workspace_id: workspaceId,
        session_namespace: namespace,
        status: result.status,
        duration_ms: result.durationMs,
        exit_code: result.exitCode,
        cancellation_confirmed: result.cancellationConfirmed,
        summary: result.summary ?? null,
        locally_rejected_references: rejectedReferences,
        events: eventSummary,
      },
      null,
      2,
    ),
    '```',
    '',
  ].join('\n');
}

function terminalFailure(result: ChatStreamResult): string {
  return result.terminal.type === 'failed'
    ? result.terminal.error.message
    : 'OpticCode chat failed.';
}

function errorMessage(error: unknown): string {
  if (error instanceof OpticCodeClientError) {
    return `${error.message} (${error.code})`;
  }
  return error instanceof Error ? error.message : String(error);
}

function escapeMarkdown(value: string): string {
  return value.replaceAll(/[\\`*_{}[\]()#+.!|>-]/g, '\\$&');
}

function timestamp(): string {
  return new Date().toISOString().replaceAll(/[:.]/g, '-');
}

export const chatInternalCommands = INTERNAL_COMMANDS;
export const chatEditCommands = EDIT_COMMANDS;

function latestProposalId(events: readonly ChatProtocolEvent[]): string | undefined {
  for (let index = events.length - 1; index >= 0; index -= 1) {
    const event = events[index];
    if (event !== undefined && 'proposal_id' in event && typeof event.proposal_id === 'string') {
      return event.proposal_id;
    }
  }
  return undefined;
}

function latestTransactionId(events: readonly ChatProtocolEvent[]): string | undefined {
  for (let index = events.length - 1; index >= 0; index -= 1) {
    const event = events[index];
    if (
      event !== undefined &&
      'transaction_id' in event &&
      typeof event.transaction_id === 'string'
    ) {
      return event.transaction_id;
    }
  }
  return undefined;
}
