import * as vscode from 'vscode';

import { OpticCodeDiagnostics, toVscodeRange } from './diagnostics';
import type { Finding, RunRecord } from './model';
import {
  assistantMarkdown,
  findingsFromJavaContext,
  findingsFromJavaEdits,
  findingsFromJavaSyntax,
  worktreeMarkdown,
} from './model';
import { createRequestId } from './protocol/client';
import { OpticCodeClientError } from './protocol/errors';
import type { AssistantProtocolEvent, JsonObject } from './protocol/types';
import { isRecord } from './protocol/validation';
import { ReportStore } from './reports';
import type { OpticCodeService } from './service';
import type { SessionState } from './state';

function timestamp(): string {
  return new Date().toISOString().replaceAll(/[:.]/g, '-');
}

function errorMessage(error: unknown): string {
  if (error instanceof OpticCodeClientError) {
    return `${error.message} (${error.code})`;
  }
  return error instanceof Error ? error.message : String(error);
}

function optionalString(value: unknown): string | undefined {
  return typeof value === 'string' ? value : undefined;
}

function optionalNumber(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
}

function findingFromArgument(value: unknown): Finding | undefined {
  if (isRecord(value) && isRecord(value.finding)) {
    return value.finding as unknown as Finding;
  }
  if (
    isRecord(value) &&
    typeof value.file === 'string' &&
    typeof value.message === 'string' &&
    isRecord(value.range)
  ) {
    return value as unknown as Finding;
  }
  return undefined;
}

export class OpticCodeController implements vscode.Disposable {
  private readonly diagnostics: OpticCodeDiagnostics;
  private readonly reports: ReportStore;
  private readonly subscriptions: vscode.Disposable[] = [];

  public constructor(
    private readonly service: OpticCodeService,
    private readonly state: SessionState,
    private readonly output: vscode.OutputChannel,
    private readonly statusBar: vscode.StatusBarItem,
    storageUri: vscode.Uri,
  ) {
    this.diagnostics = new OpticCodeDiagnostics(state);
    this.reports = new ReportStore(storageUri);
    this.registerCommands();
    this.subscriptions.push(
      vscode.workspace.onDidChangeConfiguration((event) => {
        if (event.affectsConfiguration('opticcode')) {
          this.service.invalidate();
          void this.refreshStatus(false);
        }
      }),
      vscode.workspace.onDidChangeWorkspaceFolders(() => {
        this.service.invalidate();
        void this.refreshStatus(false);
      }),
    );
    this.updateStatusBar();
  }

  public async refreshStatus(notify = true): Promise<void> {
    try {
      const connection = await this.service.connect(true);
      const doctor = await this.service.doctor();
      this.state.setStatus({
        executablePath: connection.executable.path,
        executableSource: connection.executable.source,
        opticcodeVersion: connection.version.opticcode_version,
        protocolCompatible: true,
        doctor,
      });
      this.updateStatusBar();
      if (notify) {
        const message = doctor.success
          ? 'OpticCode is ready.'
          : 'OpticCode is installed, but required doctor checks need attention.';
        await vscode.window.showInformationMessage(message);
      }
    } catch (error) {
      this.state.setStatus({
        protocolCompatible: false,
        error: errorMessage(error),
      });
      this.updateStatusBar();
      if (notify) {
        throw error;
      }
    }
  }

  public dispose(): void {
    this.diagnostics.dispose();
    for (const subscription of this.subscriptions) {
      subscription.dispose();
    }
  }

  private registerCommands(): void {
    this.register('opticcode.checkInstallation', async () => {
      const connection = await this.service.connect(true);
      this.state.setStatus({
        executablePath: connection.executable.path,
        executableSource: connection.executable.source,
        opticcodeVersion: connection.version.opticcode_version,
        protocolCompatible: true,
      });
      this.updateStatusBar();
      await vscode.window.showInformationMessage(
        `OpticCode ${connection.version.opticcode_version} is protocol-compatible.`,
      );
    });
    this.register('opticcode.refreshStatus', async () => this.refreshStatus(true));
    this.register('opticcode.selectProfile', async () => this.selectProfile());
    this.register('opticcode.selectChatContextScope', async () =>
      this.selectChatContextScope(),
    );
    this.register('opticcode.analyzeJavaProject', async () => this.analyzeJavaProject());
    this.register('opticcode.buildJavaSymbolIndex', async () => this.buildJavaSymbolIndex());
    this.register('opticcode.buildSmartContext', async () => this.buildSmartContext());
    this.register('opticcode.proposeLegacyFixes', async () => this.proposeLegacyFixes());
    this.register('opticcode.verifyProposedFixes', async () => this.verifyProposedFixes());
    this.register('opticcode.askQwen', async () => this.runAssistant('ask'));
    this.register('opticcode.planQwen', async () => this.runAssistant('plan'));
    this.register('opticcode.showLastReport', async () => this.showLastReport());
    this.register('opticcode.recoverWorktrees', async () => this.recoverWorktrees());
    this.register('opticcode.openOutput', () => this.output.show(true));
    this.register('opticcode.openFinding', async (argument: unknown) => this.openFinding(argument));
  }

  private register(command: string, callback: (...args: unknown[]) => unknown): void {
    this.subscriptions.push(
      vscode.commands.registerCommand(command, async (...args: unknown[]) => {
        try {
          await callback(...args);
        } catch (error) {
          const message = errorMessage(error);
          this.output.appendLine(`[error] ${command}: ${message}`);
          await vscode.window.showErrorMessage(message, 'Open Output').then((choice) => {
            if (choice === 'Open Output') {
              this.output.show(true);
            }
          });
        }
      }),
    );
  }

  private async selectProfile(): Promise<void> {
    const selected = await vscode.window.showQuickPick(
      [
        { label: 'minecraft-java-1.8', description: 'Java 8 / Bukkit / PandaSpigot' },
        { label: 'none', description: 'Disable profile context' },
        { label: '$(edit) Enter profile ID', description: 'Use another local profile' },
      ],
      { title: 'Select OpticCode Profile', placeHolder: 'Profile passed to opticcode.exe' },
    );
    if (selected === undefined) {
      return;
    }
    const profile = selected.label.startsWith('$(')
      ? await vscode.window.showInputBox({
          title: 'OpticCode Profile ID',
          prompt: 'Enter an existing profile directory name.',
          validateInput: (value) =>
            /^[a-zA-Z0-9._-]{1,96}$/.test(value) ? undefined : 'Use letters, digits, ., _ or -.',
        })
      : selected.label;
    if (profile === undefined) {
      return;
    }
    await vscode.workspace
      .getConfiguration('opticcode')
      .update('profile', profile, vscode.ConfigurationTarget.Workspace);
    this.service.invalidate();
    await this.refreshStatus(false);
  }

  private async selectChatContextScope(): Promise<void> {
    const selected = await vscode.window.showQuickPick(
      [
        {
          label: 'References preferred',
          description: 'Prioritize current references and expand only when needed',
          value: 'referencesPreferred',
        },
        {
          label: 'References only',
          description: 'Use only references attached to the current request',
          value: 'referencesOnly',
        },
        {
          label: 'Automatic',
          description: 'Allow project discovery, RAG, and compatible history',
          value: 'automatic',
        },
      ],
      {
        title: 'Select OpticCode Chat Context Scope',
        placeHolder: 'The Rust runtime remains authoritative and may narrow this scope',
      },
    );
    if (selected === undefined) {
      return;
    }
    await vscode.workspace
      .getConfiguration('opticcode')
      .update('chatContextScope', selected.value, vscode.ConfigurationTarget.Workspace);
  }

  private async analyzeJavaProject(): Promise<void> {
    const connection = await this.service.connect();
    const report = await this.progress('Analyze Java project', false, async () =>
      this.service.runJson(['java-syntax', '--json', '--path', connection.workspace]),
    );
    const findings = findingsFromJavaSyntax(report, connection.workspace);
    await this.replaceFindings(findings);
    await this.recordJsonReport('java-syntax', report, {
      command: 'Analyze Current Java Project',
      requestId: `java-syntax-${Date.now()}`,
      status: 'completed',
      durationMs: (optionalNumber(report.duration_us) ?? 0) / 1000,
    });
    await vscode.window.showInformationMessage(
      `OpticCode analyzed Java syntax: ${findings.length} diagnostic(s).`,
    );
  }

  private async buildJavaSymbolIndex(): Promise<void> {
    const connection = await this.service.connect();
    const report = await this.progress('Build Java symbol index', false, async () =>
      this.service.runJson(['java-index', '--json', '--path', connection.workspace]),
    );
    await this.recordJsonReport('java-index', report, {
      command: 'Build Java Symbol Index',
      requestId: `java-index-${Date.now()}`,
      status: 'completed',
      durationMs: (optionalNumber(report.duration_us) ?? 0) / 1000,
    });
    await vscode.window.showInformationMessage('OpticCode Java symbol index is ready.');
  }

  private async buildSmartContext(): Promise<void> {
    const task = await vscode.window.showInputBox({
      title: 'Build Smart Context',
      prompt: 'Describe the Java task or symbol to contextualize.',
      ignoreFocusOut: true,
    });
    if (task === undefined || task.trim() === '') {
      return;
    }
    const connection = await this.service.connect();
    const report = await this.progress('Build smart Java context', false, async () =>
      this.service.runJson([
        'java-context',
        task,
        '--json',
        '--path',
        connection.workspace,
      ]),
    );
    const findings = findingsFromJavaContext(report, connection.workspace);
    await this.replaceFindings(findings);
    await this.recordJsonReport('java-context', report, {
      command: 'Build Smart Context',
      requestId: `java-context-${Date.now()}`,
      status: 'completed',
      durationMs: (optionalNumber((isRecord(report.timings) ? report.timings.total_us : undefined)) ?? 0) / 1000,
      context: 'symbol',
    });
    await vscode.window.showInformationMessage(
      `OpticCode selected ${findings.length} context snippet(s).`,
    );
  }

  private async proposeLegacyFixes(): Promise<void> {
    const connection = await this.service.connect();
    const report = await this.progress('Propose Minecraft legacy fixes', false, async () =>
      this.service.runJson(['java-edits', '--json', '--path', connection.workspace]),
    );
    const findings = findingsFromJavaEdits(report, connection.workspace);
    await this.replaceFindings(findings);
    await this.recordJsonReport('java-edits', report, {
      command: 'Propose Minecraft Legacy Fixes',
      requestId: `java-edits-${Date.now()}`,
      status: 'completed',
      durationMs: (optionalNumber((isRecord(report.timings) ? report.timings.total_us : undefined)) ?? 0) / 1000,
    });
    await vscode.window.showInformationMessage(
      `OpticCode found ${findings.filter((finding) => finding.kind === 'safe_fix').length} safe proposal(s).`,
    );
  }

  private async verifyProposedFixes(): Promise<void> {
    const confirmation = await vscode.window.showWarningMessage(
      'OpticCode will create a disposable Git worktree, apply only verified proposals there, and may run Maven or Gradle. The original project will remain unchanged and no diff will be transferred automatically.',
      { modal: true },
      'Verify in Worktree',
    );
    if (confirmation !== 'Verify in Worktree') {
      return;
    }
    const connection = await this.service.connect();
    const report = await this.progress('Verify fixes in disposable worktree', false, async () =>
      this.service.runJson(
        [
          'java-edits-verify',
          '--json',
          '--path',
          connection.workspace,
          '--timeout-seconds',
          String(connection.settings.timeoutSeconds),
        ],
        undefined,
        true,
      ),
    );
    const markdown = worktreeMarkdown(report);
    const reportPath = await this.reports.write(`worktree-${timestamp()}.md`, markdown);
    this.state.setLastReport('Worktree Verification', markdown, reportPath);
    const worktree = isRecord(report.worktree) ? report.worktree : {};
    const build = isRecord(worktree.build) ? worktree.build : {};
    this.state.addRun({
      command: 'Verify Proposed Fixes in Worktree',
      requestId: optionalString(worktree.run_id) ?? `worktree-${Date.now()}`,
      status: report.operation_success === true ? 'completed' : 'failed',
      durationMs: optionalNumber(report.duration_ms) ?? 0,
      build: build.success === true ? 'passed' : build.success === false ? 'failed' : 'not run',
      worktree: optionalString(worktree.worktree_root),
      reportPath,
    });
    const verification = `status=${String(report.status)}, cleanup=${String(report.cleanup_success)}`;
    await this.replaceFindings(
      this.state.getFindings().map((finding) => ({ ...finding, verification })),
    );
    await this.reports.showMarkdown(markdown);
  }

  private async runAssistant(command: 'ask' | 'plan'): Promise<void> {
    const request = await vscode.window.showInputBox({
      title: command === 'ask' ? 'Ask Qwen' : 'Plan with Qwen',
      prompt:
        command === 'ask'
          ? 'Ask about the current Java project.'
          : 'Describe the implementation goal to plan.',
      ignoreFocusOut: true,
    });
    if (request === undefined || request.trim() === '') {
      return;
    }
    const connection = await this.service.connect();
    const requestId = createRequestId(command);
    const result = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: command === 'ask' ? 'OpticCode: Ask Qwen' : 'OpticCode: Plan with Qwen',
        cancellable: true,
      },
      async (progress, token) => {
        let deltas = 0;
        const onEvent = (event: AssistantProtocolEvent): void => {
          if (event.type === 'context_prepared') {
            progress.report({ message: 'Context prepared' });
          } else if (event.event?.type === 'delta') {
            deltas += 1;
            progress.report({ message: `Streaming response (${deltas} chunks)` });
          } else if (event.type === 'cancelled') {
            progress.report({ message: 'Cancellation confirmed' });
          }
        };
        return await this.service.runAssistant(command, request, requestId, onEvent, token);
      },
    );
    const title = command === 'ask' ? 'OpticCode Answer' : 'OpticCode Plan';
    const markdown = assistantMarkdown(title, result);
    const reportPath = await this.reports.write(`${command}-${timestamp()}.md`, markdown);
    this.state.setLastReport(title, markdown, reportPath);
    const generatedRun = result.summary?.runs.find((run) => run.generated);
    this.state.addRun({
      command: command === 'ask' ? 'Ask Qwen' : 'Plan with Qwen',
      requestId,
      status: result.status,
      durationMs: result.durationMs,
      context: result.summary?.used_context_mode ?? result.summary?.requested_context_mode,
      promptTokens:
        result.generation?.usage.prompt_tokens ?? generatedRun?.prompt_tokens ?? undefined,
      generatedTokens:
        result.generation?.usage.generated_tokens ?? generatedRun?.generated_tokens ?? undefined,
      model: result.summary?.model ?? result.generation?.model ?? connection.settings.model,
      reportPath,
    });
    await this.reports.showMarkdown(markdown);
    if (result.status === 'cancelled' && result.cancellationConfirmed) {
      await vscode.window.showInformationMessage('OpticCode cancellation was confirmed.');
    } else if (result.status !== 'completed') {
      await vscode.window.showWarningMessage(`OpticCode ${command} ended as ${result.status}.`);
    }
  }

  private async recoverWorktrees(): Promise<void> {
    const report = await this.service.runJson(['worktrees', '--json']);
    const inspections = Array.isArray(report.leases) ? report.leases : [];
    const choices = inspections.flatMap((inspection): Array<{ label: string; description: string }> => {
      if (!isRecord(inspection) || !isRecord(inspection.lease)) {
        return [];
      }
      const runId = optionalString(inspection.lease.run_id);
      if (runId === undefined) {
        return [];
      }
      return [
        {
          label: runId,
          description: inspection.valid === true ? 'valid lease' : 'requires inspection',
        },
      ];
    });
    if (choices.length === 0) {
      await vscode.window.showInformationMessage('No abandoned OpticCode worktree lease was found.');
      return;
    }
    const selected = await vscode.window.showQuickPick(choices, {
      title: 'Recover Abandoned Worktree',
      placeHolder: 'Select one OpticCode lease',
    });
    if (selected === undefined) {
      return;
    }
    const confirmation = await vscode.window.showWarningMessage(
      `Remove disposable worktree lease ${selected.label}?`,
      { modal: true },
      'Recover',
    );
    if (confirmation !== 'Recover') {
      return;
    }
    const cleanup = await this.service.runJson(
      ['worktrees', '--cleanup', selected.label, '--yes', '--json'],
      undefined,
      true,
    );
    await this.recordJsonReport('worktree-recovery', cleanup, {
      command: 'Recover Abandoned Worktree',
      requestId: selected.label,
      status: cleanup.success === true ? 'completed' : 'failed',
      durationMs: 0,
    });
    await this.reports.showJson(cleanup);
    await this.refreshStatus(false);
  }

  private async showLastReport(): Promise<void> {
    const report = this.state.getLastReport();
    if (report === undefined) {
      await vscode.window.showInformationMessage('No OpticCode report is available in this session.');
      return;
    }
    await this.reports.showMarkdown(report.content);
  }

  private async openFinding(argument: unknown): Promise<void> {
    const finding = findingFromArgument(argument);
    if (finding === undefined) {
      throw new Error('No OpticCode finding was selected.');
    }
    const document = await vscode.workspace.openTextDocument(vscode.Uri.file(finding.file));
    const editor = await vscode.window.showTextDocument(document, { preview: true });
    const range = toVscodeRange(document, finding);
    editor.selection = new vscode.Selection(range.start, range.end);
    editor.revealRange(range, vscode.TextEditorRevealType.InCenterIfOutsideViewport);
  }

  private async replaceFindings(findings: Finding[]): Promise<void> {
    this.state.setFindings(findings);
    await this.diagnostics.replace(findings);
  }

  private async recordJsonReport(
    kind: string,
    report: JsonObject,
    run: RunRecord,
  ): Promise<void> {
    const content = `${JSON.stringify(report, null, 2)}\n`;
    const reportPath = await this.reports.write(`${kind}-${timestamp()}.json`, content);
    this.state.setLastReport(kind, `# ${kind}\n\n\`\`\`json\n${content}\`\`\`\n`, reportPath);
    this.state.addRun({ ...run, reportPath });
  }

  private async progress<T>(
    title: string,
    cancellable: boolean,
    task: () => Promise<T>,
  ): Promise<T> {
    return await vscode.window.withProgress(
      { location: vscode.ProgressLocation.Notification, title: `OpticCode: ${title}`, cancellable },
      task,
    );
  }

  private updateStatusBar(): void {
    const status = this.state.getStatus();
    if (status.protocolCompatible && status.doctor?.success !== false) {
      this.statusBar.text = '$(pass-filled) OpticCode';
      this.statusBar.tooltip = 'OpticCode is ready';
      this.statusBar.backgroundColor = undefined;
    } else if (status.protocolCompatible) {
      this.statusBar.text = '$(warning) OpticCode';
      this.statusBar.tooltip = 'OpticCode needs attention';
      this.statusBar.backgroundColor = new vscode.ThemeColor('statusBarItem.warningBackground');
    } else {
      this.statusBar.text = '$(circle-slash) OpticCode';
      this.statusBar.tooltip = status.error ?? 'OpticCode has not been checked';
      this.statusBar.backgroundColor = undefined;
    }
    this.statusBar.command = 'opticcode.refreshStatus';
    this.statusBar.show();
  }
}
