import * as vscode from 'vscode';

import { utf16ColumnFromUtf8, type Finding, type TextPoint } from './model';
import type { SessionState } from './state';

function severity(finding: Finding): vscode.DiagnosticSeverity {
  switch (finding.kind) {
    case 'build_error':
      return vscode.DiagnosticSeverity.Error;
    case 'ambiguity':
      return vscode.DiagnosticSeverity.Warning;
    case 'safe_fix':
      return vscode.DiagnosticSeverity.Hint;
    case 'information':
    case 'refused':
      return vscode.DiagnosticSeverity.Information;
  }
}

function toVscodePosition(document: vscode.TextDocument, point: TextPoint): vscode.Position {
  const line = Math.max(0, Math.min(point.line, document.lineCount - 1));
  const text = document.lineAt(line).text;
  return new vscode.Position(line, utf16ColumnFromUtf8(text, point.character));
}

export function toVscodeRange(document: vscode.TextDocument, finding: Finding): vscode.Range {
  const start = document.validatePosition(toVscodePosition(document, finding.range.start));
  let end = document.validatePosition(toVscodePosition(document, finding.range.end));
  if (end.isBeforeOrEqual(start)) {
    end = document.positionAt(Math.min(document.getText().length, document.offsetAt(start) + 1));
  }
  return new vscode.Range(start, end);
}

export class OpticCodeDiagnostics implements vscode.Disposable {
  private readonly collection = vscode.languages.createDiagnosticCollection('opticcode');
  private readonly subscriptions: vscode.Disposable[];

  public constructor(private readonly state: SessionState) {
    this.subscriptions = [
      vscode.workspace.onDidChangeTextDocument((event) => {
        this.collection.delete(event.document.uri);
        this.state.removeFindingsForFile(event.document.uri.fsPath);
      }),
      vscode.languages.registerCodeActionsProvider(
        { language: 'java', scheme: 'file' },
        {
          provideCodeActions(document, range, context) {
            if (!context.diagnostics.some((diagnostic) => diagnostic.source === 'OpticCode')) {
              return [];
            }
            const action = new vscode.CodeAction(
              'Verify with OpticCode in Disposable Worktree',
              vscode.CodeActionKind.QuickFix,
            );
            action.command = {
              command: 'opticcode.verifyProposedFixes',
              title: 'Verify with OpticCode in Disposable Worktree',
              arguments: [document.uri, range],
            };
            return [action];
          },
        },
        { providedCodeActionKinds: [vscode.CodeActionKind.QuickFix] },
      ),
    ];
  }

  public async replace(findings: readonly Finding[]): Promise<void> {
    this.collection.clear();
    const grouped = new Map<string, Finding[]>();
    for (const finding of findings) {
      const existing = grouped.get(finding.file) ?? [];
      existing.push(finding);
      grouped.set(finding.file, existing);
    }
    for (const [file, entries] of grouped) {
      const uri = vscode.Uri.file(file);
      let document: vscode.TextDocument;
      try {
        document = await vscode.workspace.openTextDocument(uri);
      } catch {
        continue;
      }
      const diagnostics = entries.map((finding) => {
        const range = toVscodeRange(document, finding);
        const diagnostic = new vscode.Diagnostic(range, finding.message, severity(finding));
        diagnostic.source = 'OpticCode';
        diagnostic.code = finding.rule ?? finding.kind;
        return diagnostic;
      });
      this.collection.set(uri, diagnostics);
    }
  }

  public dispose(): void {
    this.collection.dispose();
    for (const subscription of this.subscriptions) {
      subscription.dispose();
    }
  }
}
