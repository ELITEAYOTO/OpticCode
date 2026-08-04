import * as vscode from 'vscode';

import type { ChatContextScope, ChatEvidenceMode, ContextMode } from './protocol/types';

export interface ExtensionSettings {
  executablePath: string;
  profile: string;
  model: string;
  contextMode: ContextMode;
  chatContextScope: ChatContextScope;
  evidenceMode: ChatEvidenceMode;
  timeoutSeconds: number;
  showDebugOutput: boolean;
  autoCheckOnStartup: boolean;
}

export function readSettings(scope?: vscode.Uri): ExtensionSettings {
  const configuration = vscode.workspace.getConfiguration('opticcode', scope);
  const configuredMode = configuration.get<string>('contextMode', 'legacy');
  const contextMode: ContextMode = ['legacy', 'symbol', 'compare'].includes(configuredMode)
    ? (configuredMode as ContextMode)
    : 'legacy';
  const timeout = configuration.get<number>('defaultTimeoutSeconds', 300);
  const configuredScope = configuration.get<string>(
    'chatContextScope',
    'referencesPreferred',
  );
  const chatContextScope: ChatContextScope =
    configuredScope === 'automatic'
      ? 'automatic'
      : configuredScope === 'referencesOnly'
        ? 'references_only'
        : 'references_preferred';
  const evidenceMode: ChatEvidenceMode =
    configuration.get<string>('evidenceMode', 'required') === 'optional'
      ? 'optional'
      : 'required';
  return {
    executablePath: configuration.get<string>('executablePath', '').trim(),
    profile: configuration.get<string>('profile', 'minecraft-java-1.8').trim(),
    model: configuration.get<string>('model', 'qwen2.5-coder:14b').trim(),
    contextMode,
    chatContextScope,
    evidenceMode,
    timeoutSeconds: Math.max(1, Math.min(3600, Math.floor(timeout))),
    showDebugOutput: configuration.get<boolean>('showDebugOutput', false),
    autoCheckOnStartup: configuration.get<boolean>('autoCheckOnStartup', true),
  };
}
