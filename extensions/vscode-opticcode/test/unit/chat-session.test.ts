import * as assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import { ChatSessionStore, type MementoLike } from '../../src/chat/session';

class MemoryMemento implements MementoLike {
  private readonly values = new Map<string, unknown>();

  public get<T>(key: string, defaultValue: T): T {
    return (this.values.get(key) as T | undefined) ?? defaultValue;
  }

  public async update(key: string, value: unknown): Promise<void> {
    this.values.set(key, value);
  }
}

describe('OpticCode chat session metadata', () => {
  it('survives reload without persisting prompts or source content', async () => {
    const memory = new MemoryMemento();
    const store = new ChatSessionStore(memory);
    await store.record({
      schemaVersion: 1,
      namespace: 'namespace-a',
      workspaceId: 'workspace-a',
      sessionId: 'session-a',
      repositoryState: 'head-a',
      recentRunIds: ['run-a'],
      lastReportPath: 'C:\\storage\\report.md',
      updatedAt: new Date(0).toISOString(),
    });

    const reloaded = new ChatSessionStore(memory);
    const session = reloaded.get('workspace-a', 'session-a');
    assert.equal(session?.repositoryState, 'head-a');
    assert.deepEqual(session?.recentRunIds, ['run-a']);
    assert.equal(reloaded.findByRunId('run-a')?.sessionId, 'session-a');
    assert.equal(JSON.stringify(session).includes('prompt'), false);
    assert.equal(JSON.stringify(session).includes('source'), false);
  });

  it('does not leak metadata between workspaces', async () => {
    const memory = new MemoryMemento();
    const store = new ChatSessionStore(memory);
    await store.record({
      schemaVersion: 1,
      namespace: 'namespace-a',
      workspaceId: 'workspace-a',
      sessionId: 'same-session',
      recentRunIds: ['run-a'],
      updatedAt: new Date().toISOString(),
    });
    assert.equal(store.get('workspace-b', 'same-session'), undefined);
  });

  it('clears conversational context while preserving edit transaction pointers', async () => {
    const memory = new MemoryMemento();
    const store = new ChatSessionStore(memory);
    await store.record({
      schemaVersion: 1,
      namespace: 'namespace-a',
      workspaceId: 'workspace-a',
      sessionId: 'session-a',
      repositoryState: 'state-a',
      recentRunIds: ['run-a'],
      lastReportPath: 'C:\\storage\\chat.md',
      lastProposalId: 'proposal-a',
      lastTransactionId: 'transaction-a',
      updatedAt: new Date().toISOString(),
    });
    const epoch = await store.clearContext();
    const retained = store.get('workspace-a', 'session-a');
    assert.equal(epoch, 1);
    assert.equal(store.contextEpoch(), 1);
    assert.deepEqual(retained?.recentRunIds, []);
    assert.equal(retained?.repositoryState, undefined);
    assert.equal(retained?.lastReportPath, undefined);
    assert.equal(retained?.lastProposalId, 'proposal-a');
    assert.equal(retained?.lastTransactionId, 'transaction-a');
  });
});
