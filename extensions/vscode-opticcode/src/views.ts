import * as path from 'node:path';
import * as vscode from 'vscode';

import type { Finding, RunRecord } from './model';
import type { SessionState } from './state';

abstract class RefreshingProvider<T> implements vscode.TreeDataProvider<T>, vscode.Disposable {
  protected readonly changeEmitter = new vscode.EventEmitter<T | undefined | void>();
  public readonly onDidChangeTreeData = this.changeEmitter.event;
  private readonly subscription: vscode.Disposable;

  protected constructor(state: SessionState) {
    this.subscription = state.onDidChange(() => this.changeEmitter.fire());
  }

  public abstract getTreeItem(element: T): vscode.TreeItem;
  public abstract getChildren(element?: T): T[];

  public dispose(): void {
    this.subscription.dispose();
    this.changeEmitter.dispose();
  }
}

interface StatusNode {
  label: string;
  value: string;
  icon: string;
}

export class StatusProvider extends RefreshingProvider<StatusNode> {
  public constructor(private readonly state: SessionState) {
    super(state);
  }

  public getTreeItem(element: StatusNode): vscode.TreeItem {
    const item = new vscode.TreeItem(element.label);
    item.description = element.value;
    item.tooltip = `${element.label}: ${element.value}`;
    item.iconPath = new vscode.ThemeIcon(element.icon);
    return item;
  }

  public getChildren(): StatusNode[] {
    const status = this.state.getStatus();
    const nodes: StatusNode[] = [
      {
        label: 'Executable',
        value: status.executablePath ?? 'not found',
        icon: status.executablePath === undefined ? 'error' : 'pass',
      },
      {
        label: 'Protocol',
        value: status.protocolCompatible ? 'compatible' : 'not checked',
        icon: status.protocolCompatible ? 'verified' : 'warning',
      },
    ];
    if (status.opticcodeVersion !== undefined) {
      nodes.push({ label: 'Version', value: status.opticcodeVersion, icon: 'versions' });
    }
    if (status.executableSource !== undefined) {
      nodes.push({ label: 'Resolution', value: status.executableSource, icon: 'location' });
    }
    for (const check of status.doctor?.checks ?? []) {
      nodes.push({
        label: check.id.replaceAll('_', ' '),
        value: check.summary,
        icon:
          check.status === 'ok'
            ? 'pass'
            : check.status === 'warning'
              ? 'warning'
              : check.status === 'unavailable'
                ? 'circle-slash'
                : 'error',
      });
    }
    if (status.error !== undefined) {
      nodes.push({ label: 'Error', value: status.error, icon: 'error' });
    }
    return nodes;
  }
}

export class FindingTreeItem extends vscode.TreeItem {
  public override readonly contextValue = 'opticcodeFinding';

  public constructor(public readonly finding: Finding) {
    super(
      `${path.basename(finding.file)}:${finding.range.start.line + 1}`,
      vscode.TreeItemCollapsibleState.None,
    );
    this.description = finding.rule ?? finding.kind;
    this.tooltip = new vscode.MarkdownString(
      `**${finding.message}**\n\n${finding.reason}\n\n${finding.file}`,
    );
    this.iconPath = new vscode.ThemeIcon(
      finding.kind === 'safe_fix'
        ? 'lightbulb'
        : finding.kind === 'build_error'
          ? 'error'
          : finding.kind === 'ambiguity'
            ? 'warning'
            : finding.kind === 'refused'
              ? 'circle-slash'
              : 'info',
    );
    this.command = {
      command: 'opticcode.openFinding',
      title: 'Open Finding',
      arguments: [finding],
    };
  }
}

export class FindingsProvider extends RefreshingProvider<FindingTreeItem> {
  public constructor(private readonly state: SessionState) {
    super(state);
  }

  public getTreeItem(element: FindingTreeItem): vscode.TreeItem {
    return element;
  }

  public getChildren(): FindingTreeItem[] {
    return this.state.getFindings().map((finding) => new FindingTreeItem(finding));
  }
}

class RunTreeItem extends vscode.TreeItem {
  public constructor(run: RunRecord) {
    super(run.command, vscode.TreeItemCollapsibleState.None);
    this.description = `${run.status} | ${(run.durationMs / 1000).toFixed(2)} s`;
    this.tooltip = [
      `Request: ${run.requestId}`,
      `Status: ${run.status}`,
      `Context: ${run.context ?? 'n/a'}`,
      `Tokens: ${run.promptTokens ?? 'n/a'} -> ${run.generatedTokens ?? 'n/a'}`,
      `Model: ${run.model ?? 'n/a'}`,
      `Build: ${run.build ?? 'n/a'}`,
      `Worktree: ${run.worktree ?? 'n/a'}`,
      `Report: ${run.reportPath ?? 'session only'}`,
    ].join('\n');
    this.iconPath = new vscode.ThemeIcon(
      run.status === 'completed'
        ? 'pass'
        : run.status === 'running'
          ? 'sync~spin'
          : run.status === 'cancelled'
            ? 'debug-stop'
            : 'error',
    );
  }
}

export class RunsProvider extends RefreshingProvider<RunTreeItem> {
  public constructor(private readonly state: SessionState) {
    super(state);
  }

  public getTreeItem(element: RunTreeItem): vscode.TreeItem {
    return element;
  }

  public getChildren(): RunTreeItem[] {
    return this.state.getRuns().map((run) => new RunTreeItem(run));
  }
}
