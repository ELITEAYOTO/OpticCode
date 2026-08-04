import * as vscode from 'vscode';

import type { Finding, RunRecord, StatusSnapshot } from './model';

export class SessionState implements vscode.Disposable {
  private readonly changedEmitter = new vscode.EventEmitter<void>();
  private readonly runs: RunRecord[] = [];
  private findings: Finding[] = [];
  private status: StatusSnapshot = { protocolCompatible: false };
  private lastReport: { title: string; content: string; path?: string | undefined } | undefined;

  public readonly onDidChange = this.changedEmitter.event;

  public getStatus(): StatusSnapshot {
    return this.status;
  }

  public setStatus(status: StatusSnapshot): void {
    this.status = status;
    this.changedEmitter.fire();
  }

  public getFindings(): readonly Finding[] {
    return this.findings;
  }

  public setFindings(findings: Finding[]): void {
    this.findings = findings;
    this.changedEmitter.fire();
  }

  public removeFindingsForFile(file: string): void {
    const retained = this.findings.filter((finding) => finding.file !== file);
    if (retained.length !== this.findings.length) {
      this.findings = retained;
      this.changedEmitter.fire();
    }
  }

  public getRuns(): readonly RunRecord[] {
    return this.runs;
  }

  public getRunsForWorkspace(workspaceId: string): readonly RunRecord[] {
    return this.runs.filter((run) => run.workspaceId === workspaceId);
  }

  public addRun(run: RunRecord): void {
    this.runs.unshift(run);
    this.runs.splice(50);
    this.changedEmitter.fire();
  }

  public setLastReport(title: string, content: string, reportPath?: string): void {
    this.lastReport = { title, content, path: reportPath };
    this.changedEmitter.fire();
  }

  public getLastReport(): { title: string; content: string; path?: string | undefined } | undefined {
    return this.lastReport;
  }

  public dispose(): void {
    this.changedEmitter.dispose();
  }
}
