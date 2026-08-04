import * as vscode from 'vscode';

import { registerOpticCodeChat } from './chat/participant';
import { readSettings } from './configuration';
import { OpticCodeController } from './controller';
import { OpticCodeService } from './service';
import { SessionState } from './state';
import { FindingsProvider, RunsProvider, StatusProvider } from './views';

export interface OpticCodeExtensionApi {
  chatParticipantId: string;
  promptLabHandler?: vscode.ChatRequestHandler;
}

export function activate(context: vscode.ExtensionContext): OpticCodeExtensionApi {
  const output = vscode.window.createOutputChannel('OpticCode');
  const statusBar = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 50);
  const state = new SessionState();
  const service = new OpticCodeService(context.extensionPath, output);
  const controller = new OpticCodeController(
    service,
    state,
    output,
    statusBar,
    context.globalStorageUri,
  );
  const statusProvider = new StatusProvider(state);
  const findingsProvider = new FindingsProvider(state);
  const runsProvider = new RunsProvider(state);
  const chat = registerOpticCodeChat(context, service, state, output);

  context.subscriptions.push(
    output,
    statusBar,
    state,
    controller,
    statusProvider,
    findingsProvider,
    runsProvider,
    chat,
    vscode.window.registerTreeDataProvider('opticcode.status', statusProvider),
    vscode.window.registerTreeDataProvider('opticcode.findings', findingsProvider),
    vscode.window.registerTreeDataProvider('opticcode.runs', runsProvider),
  );

  const scope = vscode.workspace.workspaceFolders?.[0]?.uri;
  if (readSettings(scope).autoCheckOnStartup) {
    void controller.refreshStatus(false);
  }
  return {
    chatParticipantId: chat.participantId,
    ...(process.env['OPTICCODE_PROMPT_LAB'] === '1'
      ? { promptLabHandler: chat.requestHandler }
      : {}),
  };
}

export function deactivate(): void {}
