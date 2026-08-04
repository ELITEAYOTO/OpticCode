export interface MementoLike {
  get<T>(key: string, defaultValue: T): T;
  update(key: string, value: unknown): Thenable<void>;
}

export interface ChatSessionMetadata {
  schemaVersion: 1;
  namespace: string;
  workspaceId: string;
  sessionId: string;
  repositoryState?: string | undefined;
  recentRunIds: string[];
  lastReportPath?: string | undefined;
  lastProposalId?: string | undefined;
  lastTransactionId?: string | undefined;
  updatedAt: string;
}

const STORAGE_KEY = 'opticcode.chat.sessions.v1';
const MAX_SESSIONS = 32;
const MAX_RECENT_RUNS = 16;

export class ChatSessionStore {
  private readonly sessions = new Map<string, ChatSessionMetadata>();

  public constructor(private readonly storage: MementoLike) {
    const stored = storage.get<unknown[]>(STORAGE_KEY, []);
    for (const candidate of stored.slice(0, MAX_SESSIONS)) {
      const session = parseSession(candidate);
      if (session !== undefined) {
        this.sessions.set(sessionKey(session.workspaceId, session.sessionId), session);
      }
    }
  }

  public get(workspaceId: string, sessionId: string): ChatSessionMetadata | undefined {
    return this.sessions.get(sessionKey(workspaceId, sessionId));
  }

  public findByRunId(runId: string): ChatSessionMetadata | undefined {
    return [...this.sessions.values()].find((session) =>
      session.recentRunIds.includes(runId),
    );
  }

  public async record(session: ChatSessionMetadata): Promise<void> {
    const bounded: ChatSessionMetadata = {
      ...session,
      recentRunIds: session.recentRunIds.slice(0, MAX_RECENT_RUNS),
      updatedAt: new Date().toISOString(),
    };
    this.sessions.set(sessionKey(session.workspaceId, session.sessionId), bounded);
    const retained = [...this.sessions.values()]
      .sort((left, right) => right.updatedAt.localeCompare(left.updatedAt))
      .slice(0, MAX_SESSIONS);
    this.sessions.clear();
    for (const value of retained) {
      this.sessions.set(sessionKey(value.workspaceId, value.sessionId), value);
    }
    await this.storage.update(STORAGE_KEY, retained);
  }

  public async remove(workspaceId: string, sessionId: string): Promise<void> {
    this.sessions.delete(sessionKey(workspaceId, sessionId));
    await this.storage.update(STORAGE_KEY, [...this.sessions.values()]);
  }
}

function parseSession(value: unknown): ChatSessionMetadata | undefined {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    return undefined;
  }
  const record = value as Record<string, unknown>;
  if (
    record.schemaVersion !== 1 ||
    typeof record.namespace !== 'string' ||
    typeof record.workspaceId !== 'string' ||
    typeof record.sessionId !== 'string' ||
    typeof record.updatedAt !== 'string' ||
    !Array.isArray(record.recentRunIds)
  ) {
    return undefined;
  }
  const recentRunIds = record.recentRunIds.filter(
    (entry): entry is string => typeof entry === 'string' && entry.length <= 128,
  );
  return {
    schemaVersion: 1,
    namespace: record.namespace.slice(0, 128),
    workspaceId: record.workspaceId.slice(0, 128),
    sessionId: record.sessionId.slice(0, 128),
    recentRunIds: recentRunIds.slice(0, MAX_RECENT_RUNS),
    updatedAt: record.updatedAt,
    ...(optionalString(record.repositoryState) === undefined
      ? {}
      : { repositoryState: optionalString(record.repositoryState) }),
    ...(optionalString(record.lastReportPath) === undefined
      ? {}
      : { lastReportPath: optionalString(record.lastReportPath) }),
    ...(optionalString(record.lastProposalId) === undefined
      ? {}
      : { lastProposalId: optionalString(record.lastProposalId) }),
    ...(optionalString(record.lastTransactionId) === undefined
      ? {}
      : { lastTransactionId: optionalString(record.lastTransactionId) }),
  };
}

function optionalString(value: unknown): string | undefined {
  return typeof value === 'string' && value.length <= 512 ? value : undefined;
}

function sessionKey(workspaceId: string, sessionId: string): string {
  return `${workspaceId}\u0000${sessionId}`;
}
