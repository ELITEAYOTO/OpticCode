import { randomUUID } from 'node:crypto';
import * as path from 'node:path';
import * as vscode from 'vscode';

import type { Connection } from '../service';
import type {
  ChatCommand,
  ChatEditControl,
  ChatProtocolEvent,
  ChatProtocolRequest,
  ChatStreamResult,
} from '../protocol/types';
import { buildChatRequest } from './model';
import { ChatEditArtifactStore, type ChatEditArtifact } from './editArtifacts';

export const EDIT_BASE_SCHEME = 'opticcode-base';
export const EDIT_PROPOSED_SCHEME = 'opticcode-proposed';

export interface EditReviewRuntimeService {
  connectForWorkspace(workspace: vscode.Uri, force?: boolean): Promise<Connection>;
  runChat(
    request: ChatProtocolRequest,
    workspace: vscode.Uri,
    onEvent: (event: ChatProtocolEvent) => void,
    cancellation?: vscode.CancellationToken,
  ): Promise<ChatStreamResult>;
}

export class ChatEditReviewController implements vscode.Disposable {
  private readonly artifacts = new ChatEditArtifactStore();
  private readonly disposables: vscode.Disposable[];

  public constructor(
    private readonly clientVersion: string,
    private readonly service: EditReviewRuntimeService,
    private readonly output: Pick<vscode.OutputChannel, 'appendLine'>,
  ) {
    const provider = new EditReviewContentProvider(this.artifacts);
    this.disposables = [
      vscode.workspace.registerTextDocumentContentProvider(EDIT_BASE_SCHEME, provider),
      vscode.workspace.registerTextDocumentContentProvider(EDIT_PROPOSED_SCHEME, provider),
    ];
  }

  public observe(workspaceRoot: string, event: ChatProtocolEvent): void {
    this.artifacts.accept(workspaceRoot, event);
  }

  public async showDiff(proposalId: unknown): Promise<void> {
    const selected = requiredIdentifier(proposalId, 'proposal');
    const workspace = currentWorkspace();
    const artifact = await this.ensureArtifact(workspace, selected);
    const first = artifact.files[0];
    if (first === undefined) {
      throw new Error('The verified proposal contains no reviewable file.');
    }
    await openNativeDiff(artifact, first.path);
  }

  public async showAllChanges(proposalId: unknown): Promise<void> {
    const selected = requiredIdentifier(proposalId, 'proposal');
    const workspace = currentWorkspace();
    const artifact = await this.ensureArtifact(workspace, selected);
    for (const file of artifact.files) {
      await openNativeDiff(artifact, file.path);
    }
  }

  public async applyProposal(
    proposalId: unknown,
    approvalRequestId: unknown,
  ): Promise<void> {
    const selected = requiredIdentifier(proposalId, 'proposal');
    const workspace = currentWorkspace();
    let artifact = await this.ensureArtifact(workspace, selected);
    let approval = optionalIdentifier(approvalRequestId) ?? artifact.approvalRequestId;
    if (approval === undefined || artifact.approvalOperation !== 'apply') {
      await this.runEditCommand(workspace, 'apply', { proposal_id: selected });
      artifact = await this.ensureArtifact(workspace, selected);
      approval = artifact.approvalRequestId;
    }
    if (approval === undefined || artifact.approvalOperation !== 'apply') {
      throw new Error('The runtime did not publish a state-bound apply confirmation request.');
    }
    const created = artifact.files.filter((file) => file.status === 'created').length;
    const choice = await vscode.window.showWarningMessage(
      'Apply verified OpticCode changes to the original workspace?',
      {
        modal: true,
        detail: [
          `Workspace: ${artifact.workspaceRoot}`,
          `${artifact.files.length} file(s), ${created} created, +${artifact.additions} / -${artifact.deletions}`,
          `Build: ${artifact.build ?? 'passed in isolated worktree'}; tests: ${artifact.tests ?? 'not_run'}`,
          artifact.approvalSummary ?? 'The original workspace will be changed transactionally.',
          'An exact rollback transaction will be retained.',
        ].join('\n'),
      },
      'Apply',
    );
    if (choice !== 'Apply') {
      return;
    }
    const result = await this.runEditCommand(workspace, 'apply', {
      proposal_id: selected,
      native_confirmation: nativeConfirmation(approval),
    });
    const applied = [...result.events]
      .reverse()
      .find((event) => event.type === 'apply_completed');
    if (applied?.type !== 'apply_completed' || !applied.success) {
      throw new Error('The transactional apply did not complete. Inspect the OpticCode report.');
    }
    await vscode.window.showInformationMessage(
      'OpticCode applied the verified transaction. Exact rollback is available.',
    );
  }

  public async rollbackTransaction(
    proposalOrTransaction: unknown,
    approvalOrTransaction: unknown,
  ): Promise<void> {
    const workspace = currentWorkspace();
    const first = optionalIdentifier(proposalOrTransaction);
    const second = optionalIdentifier(approvalOrTransaction);
    let proposalId = first;
    let transactionId = second?.startsWith('rollback-confirmation-') ? undefined : second;
    let approval = second?.startsWith('rollback-confirmation-') ? second : undefined;
    if (proposalId === undefined && transactionId !== undefined) {
      proposalId = this.artifacts.findByTransaction(workspace.uri.fsPath, transactionId)?.proposalId;
    }
    if (proposalId === undefined && first !== undefined) {
      const byTransaction = this.artifacts.findByTransaction(workspace.uri.fsPath, first);
      if (byTransaction !== undefined) {
        proposalId = byTransaction.proposalId;
        transactionId = first;
      }
    }
    if (proposalId === undefined) {
      throw new Error('No proposal is associated with the selected rollback transaction.');
    }
    let artifact = await this.ensureArtifact(workspace, proposalId);
    transactionId ??= artifact.transactionId;
    if (approval === undefined || artifact.approvalOperation !== 'rollback') {
      await this.runEditCommand(workspace, 'rollback', {
        proposal_id: proposalId,
        ...(transactionId === undefined ? {} : { transaction_id: transactionId }),
      });
      artifact = await this.ensureArtifact(workspace, proposalId);
      approval = artifact.approvalRequestId;
    }
    if (approval === undefined || artifact.approvalOperation !== 'rollback') {
      throw new Error('The runtime did not publish a state-bound rollback confirmation request.');
    }
    const choice = await vscode.window.showWarningMessage(
      'Rollback this exact OpticCode transaction?',
      {
        modal: true,
        detail: [
          `Workspace: ${artifact.workspaceRoot}`,
          `Transaction: ${transactionId ?? artifact.transactionId ?? 'selected proposal transaction'}`,
          `${artifact.files.length} file(s) will be restored to their verified base snapshots.`,
          artifact.approvalSummary ?? 'Current files and Git state will be revalidated first.',
        ].join('\n'),
      },
      'Rollback',
    );
    if (choice !== 'Rollback') {
      return;
    }
    const result = await this.runEditCommand(workspace, 'rollback', {
      proposal_id: proposalId,
      ...(transactionId === undefined ? {} : { transaction_id: transactionId }),
      native_confirmation: nativeConfirmation(approval),
    });
    const rollback = [...result.events]
      .reverse()
      .find((event) => event.type === 'rollback_completed');
    if (rollback?.type !== 'rollback_completed' || !rollback.success) {
      throw new Error('The exact rollback did not complete. Inspect the OpticCode report.');
    }
    await vscode.window.showInformationMessage('OpticCode restored the verified base snapshots.');
  }

  public async discardProposal(proposalId: unknown): Promise<void> {
    const selected = requiredIdentifier(proposalId, 'proposal');
    const workspace = currentWorkspace();
    const choice = await vscode.window.showWarningMessage(
      'Discard this local OpticCode proposal?',
      { modal: true, detail: 'The original workspace is not modified.' },
      'Discard',
    );
    if (choice !== 'Discard') {
      return;
    }
    await this.runEditCommand(workspace, 'diff', {
      proposal_id: selected,
      discard: true,
    });
  }

  public dispose(): void {
    for (const disposable of this.disposables) {
      disposable.dispose();
    }
  }

  private async ensureArtifact(
    workspace: vscode.WorkspaceFolder,
    proposalId: string,
  ): Promise<ChatEditArtifact> {
    let artifact = this.artifacts.get(workspace.uri.fsPath, proposalId);
    if (artifact === undefined) {
      await this.runEditCommand(workspace, 'diff', { proposal_id: proposalId });
      artifact = this.artifacts.get(workspace.uri.fsPath, proposalId);
    }
    if (artifact === undefined) {
      throw new Error('Verified review snapshots are unavailable for this proposal.');
    }
    return artifact;
  }

  private async runEditCommand(
    workspace: vscode.WorkspaceFolder,
    command: Extract<ChatCommand, 'diff' | 'apply' | 'rollback'>,
    edit: ChatEditControl,
  ): Promise<ChatStreamResult> {
    const connection = await this.service.connectForWorkspace(workspace.uri);
    const request = internalRequest(
      command,
      edit,
      connection,
      this.clientVersion,
    );
    this.output.appendLine(`[chat-edit] /${command} ${edit.proposal_id ?? edit.transaction_id ?? ''}`);
    const result = await this.service.runChat(
      request,
      workspace.uri,
      (event) => this.observe(connection.workspace, event),
    );
    if (result.status === 'failed') {
      const failure = result.terminal.type === 'failed'
        ? result.terminal.error.message
        : `/${command} failed`;
      throw new Error(failure);
    }
    return result;
  }
}

export class EditReviewContentProvider implements vscode.TextDocumentContentProvider {
  public constructor(private readonly artifacts: ChatEditArtifactStore) {}

  public provideTextDocumentContent(uri: vscode.Uri): string {
    const parsed = parseReviewUri(uri);
    const content = this.artifacts.content(
      parsed.workspaceRoot,
      parsed.proposalId,
      parsed.relativePath,
      parsed.side,
    );
    if (content === undefined) {
      throw vscode.FileSystemError.FileNotFound(uri);
    }
    return content;
  }
}

async function openNativeDiff(artifact: ChatEditArtifact, relativePath: string): Promise<void> {
  const base = reviewUri(artifact, relativePath, 'base');
  const proposed = reviewUri(artifact, relativePath, 'proposed');
  await vscode.commands.executeCommand(
    'vscode.diff',
    base,
    proposed,
    `${relativePath} (OpticCode ${artifact.proposalId})`,
    { preview: false },
  );
}

export function reviewUri(
  artifact: Pick<ChatEditArtifact, 'workspaceRoot' | 'proposalId'>,
  relativePath: string,
  side: 'base' | 'proposed',
): vscode.Uri {
  const query = new URLSearchParams({
    workspace: artifact.workspaceRoot,
    proposal: artifact.proposalId,
    path: relativePath,
  }).toString();
  return vscode.Uri.from({
    scheme: side === 'base' ? EDIT_BASE_SCHEME : EDIT_PROPOSED_SCHEME,
    path: `/${path.basename(relativePath)}`,
    query,
  });
}

function parseReviewUri(uri: vscode.Uri): {
  workspaceRoot: string;
  proposalId: string;
  relativePath: string;
  side: 'base' | 'proposed';
} {
  if (uri.scheme !== EDIT_BASE_SCHEME && uri.scheme !== EDIT_PROPOSED_SCHEME) {
    throw vscode.FileSystemError.FileNotFound(uri);
  }
  const query = new URLSearchParams(uri.query);
  const workspaceRoot = query.get('workspace');
  const proposalId = query.get('proposal');
  const relativePath = query.get('path');
  if (
    workspaceRoot === null ||
    proposalId === null ||
    relativePath === null ||
    optionalIdentifier(proposalId) === undefined ||
    relativePath === '' ||
    path.isAbsolute(relativePath) ||
    relativePath.split(/[\\/]/u).includes('..')
  ) {
    throw vscode.FileSystemError.FileNotFound(uri);
  }
  return {
    workspaceRoot,
    proposalId,
    relativePath: relativePath.replaceAll('\\', '/'),
    side: uri.scheme === EDIT_BASE_SCHEME ? 'base' : 'proposed',
  };
}

function internalRequest(
  command: Extract<ChatCommand, 'diff' | 'apply' | 'rollback'>,
  edit: ChatEditControl,
  connection: Connection,
  clientVersion: string,
): ChatProtocolRequest {
  return buildChatRequest({
    command,
    prompt: '',
    workspaceRoot: connection.workspace,
    profile: connection.settings.profile,
    model: connection.settings.model,
    contextMode: connection.settings.contextMode,
    sessionId: `edit-review-${randomUUID()}`,
    clientVersion,
    vscodeVersion: vscode.version,
    locale: vscode.env.language,
    references: [],
    history: [],
    recentRunIds: [],
    expectedProtocols: {
      chat: connection.version.protocols.chat.schema_version,
      assistant: connection.version.protocols.assistant.schema_version,
      discovery: connection.version.protocols.discovery.schema_version,
      llm: connection.version.protocols.llm.schema_version,
    },
    securityMode: 'read_only',
    edit,
  });
}

function nativeConfirmation(approvalRequestId: string): {
  client: string;
  confirmation_id: string;
  approval_request_id: string;
} {
  return {
    client: 'opticcode-vscode',
    confirmation_id: `vscode-modal-${randomUUID()}`,
    approval_request_id: approvalRequestId,
  };
}

function currentWorkspace(): vscode.WorkspaceFolder {
  const active = vscode.window.activeTextEditor?.document.uri;
  const selected = active === undefined ? undefined : vscode.workspace.getWorkspaceFolder(active);
  const workspace = selected ?? vscode.workspace.workspaceFolders?.[0];
  if (workspace === undefined || workspace.uri.scheme !== 'file') {
    throw new Error('Open a local workspace before reviewing OpticCode changes.');
  }
  return workspace;
}

function requiredIdentifier(value: unknown, name: string): string {
  const identifier = optionalIdentifier(value);
  if (identifier === undefined) {
    throw new Error(`No valid OpticCode ${name} was selected.`);
  }
  return identifier;
}

function optionalIdentifier(value: unknown): string | undefined {
  return typeof value === 'string' && /^[a-zA-Z0-9._:-]{1,160}$/u.test(value)
    ? value
    : undefined;
}
