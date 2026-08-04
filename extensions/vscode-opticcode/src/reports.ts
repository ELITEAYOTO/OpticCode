import * as vscode from 'vscode';

export class ReportStore {
  public constructor(private readonly root: vscode.Uri) {}

  public async write(name: string, content: string): Promise<string> {
    const reports = vscode.Uri.joinPath(this.root, 'reports');
    await vscode.workspace.fs.createDirectory(reports);
    const safeName = name.replaceAll(/[^a-zA-Z0-9._-]/g, '_').slice(0, 96);
    const target = vscode.Uri.joinPath(reports, safeName);
    await vscode.workspace.fs.writeFile(target, Buffer.from(content, 'utf8'));
    return target.fsPath;
  }

  public async showMarkdown(content: string): Promise<void> {
    const document = await vscode.workspace.openTextDocument({
      language: 'markdown',
      content,
    });
    await vscode.window.showTextDocument(document, { preview: true });
  }

  public async showJson(value: unknown): Promise<void> {
    const document = await vscode.workspace.openTextDocument({
      language: 'json',
      content: `${JSON.stringify(value, null, 2)}\n`,
    });
    await vscode.window.showTextDocument(document, { preview: true });
  }
}
