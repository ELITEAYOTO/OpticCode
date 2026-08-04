import * as assert from 'node:assert/strict';
import * as vscode from 'vscode';

import type { Finding } from '../../src/model';
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
  await extension.activate();
  assert.equal(extension.isActive, true);
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
  } finally {
    runs.dispose();
    status.dispose();
    state.dispose();
  }
}
