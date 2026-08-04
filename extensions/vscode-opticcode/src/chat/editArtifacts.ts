import * as path from 'node:path';

import type { ChatEditReviewFile, ChatProtocolEvent } from '../protocol/types';

export interface ChatEditArtifact {
  proposalId: string;
  workspaceRoot: string;
  files: ChatEditReviewFile[];
  additions: number;
  deletions: number;
  displayPatch: string;
  displayTruncated: boolean;
  approvalRequestId?: string | undefined;
  approvalOperation?: 'apply' | 'rollback' | undefined;
  approvalSummary?: string | undefined;
  transactionId?: string | undefined;
  build?: string | undefined;
  tests?: string | undefined;
}

export class ChatEditArtifactStore {
  private readonly artifacts = new Map<string, ChatEditArtifact>();

  public accept(workspaceRoot: string, event: ChatProtocolEvent): void {
    if (event.type === 'diff_ready' && event.changes !== undefined) {
      const previous = this.get(workspaceRoot, event.proposal_id);
      this.artifacts.set(key(workspaceRoot, event.proposal_id), {
        proposalId: event.proposal_id,
        workspaceRoot,
        files: event.changes.map((file) => ({ ...file })),
        additions: event.additions,
        deletions: event.deletions,
        displayPatch: event.display_patch ?? '',
        displayTruncated: event.display_truncated ?? false,
        ...(previous?.approvalRequestId === undefined
          ? {}
          : { approvalRequestId: previous.approvalRequestId }),
        ...(previous?.approvalOperation === undefined
          ? {}
          : { approvalOperation: previous.approvalOperation }),
        ...(previous?.approvalSummary === undefined
          ? {}
          : { approvalSummary: previous.approvalSummary }),
        ...(previous?.transactionId === undefined
          ? {}
          : { transactionId: previous.transactionId }),
        ...(previous?.build === undefined ? {} : { build: previous.build }),
        ...(previous?.tests === undefined ? {} : { tests: previous.tests }),
      });
      return;
    }
    const proposalId = eventProposalId(event);
    if (proposalId === undefined) {
      return;
    }
    const current = this.get(workspaceRoot, proposalId);
    if (event.type === 'proposal_discarded') {
      this.artifacts.delete(key(workspaceRoot, proposalId));
      return;
    }
    if (current === undefined) {
      return;
    }
    if (event.type === 'approval_required') {
      current.approvalRequestId = event.approval_request_id;
      current.approvalOperation = event.operation ?? 'apply';
      current.approvalSummary = event.summary;
    } else if (event.type === 'rollback_available') {
      current.transactionId = event.transaction_id;
    } else if (event.type === 'apply_completed' && event.success) {
      current.transactionId = event.transaction_id;
    } else if (event.type === 'verification_completed') {
      current.build = event.build;
      current.tests = event.tests;
    } else if (event.type === 'build_completed') {
      current.build = event.build;
      current.tests = event.tests;
    }
  }

  public get(workspaceRoot: string, proposalId: string): ChatEditArtifact | undefined {
    return this.artifacts.get(key(workspaceRoot, proposalId));
  }

  public findByTransaction(
    workspaceRoot: string,
    transactionId: string,
  ): ChatEditArtifact | undefined {
    return [...this.artifacts.values()].find(
      (artifact) =>
        sameWorkspace(artifact.workspaceRoot, workspaceRoot) &&
        artifact.transactionId === transactionId,
    );
  }

  public content(
    workspaceRoot: string,
    proposalId: string,
    relativePath: string,
    side: 'base' | 'proposed',
  ): string | undefined {
    const artifact = this.get(workspaceRoot, proposalId);
    const file = artifact?.files.find((candidate) => candidate.path === relativePath);
    if (file === undefined) {
      return undefined;
    }
    return side === 'base' ? (file.base_content ?? '') : file.proposed_content;
  }
}

function eventProposalId(event: ChatProtocolEvent): string | undefined {
  switch (event.type) {
    case 'edit_plan_started':
    case 'request_accepted':
    case 'references_resolving':
    case 'references_resolved':
    case 'context_started':
    case 'context_ready':
    case 'retrieval_progress':
    case 'provider_started':
    case 'token_delta':
    case 'finding':
    case 'warning':
    case 'metrics':
    case 'edit_plan_ready':
    case 'completed':
    case 'cancelled':
    case 'failed':
      return undefined;
    case 'policy_decision':
      return event.proposal_id;
    case 'proposal_stored':
    case 'verification_started':
    case 'worktree_created':
    case 'edit_applied_in_worktree':
    case 'build_started':
    case 'build_completed':
    case 'verification_completed':
    case 'diff_ready':
    case 'approval_required':
    case 'apply_started':
    case 'apply_completed':
    case 'rollback_started':
    case 'rollback_completed':
    case 'proposal_discarded':
      return event.proposal_id;
    case 'rollback_available':
      return event.proposal_id;
  }
}

function key(workspaceRoot: string, proposalId: string): string {
  return `${workspaceKey(workspaceRoot)}\u0000${proposalId}`;
}

function sameWorkspace(left: string, right: string): boolean {
  return workspaceKey(left) === workspaceKey(right);
}

function workspaceKey(value: string): string {
  const resolved = path.resolve(value);
  return process.platform === 'win32' ? resolved.toLocaleLowerCase('en-US') : resolved;
}
