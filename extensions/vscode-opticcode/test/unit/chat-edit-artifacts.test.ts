import * as assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import { ChatEditArtifactStore } from '../../src/chat/editArtifacts';
import { ChatEventPresenter } from '../../src/chat/presentation';
import type { ChatProtocolEvent } from '../../src/protocol/types';

const base = {
  schema_version: 1,
  protocol: 'opticcode.chat',
  request_id: 'request-edit',
  elapsed_ms: 1,
};

function diffEvent(sequence = 0): ChatProtocolEvent {
  return {
    ...base,
    sequence,
    type: 'diff_ready',
    proposal_id: 'plan-review-1',
    files: 2,
    additions: 4,
    deletions: 1,
    display_patch: 'diff --git a/src/Plugin.java b/src/Plugin.java\n',
    display_truncated: false,
    changes: [
      {
        path: 'src/Plugin.java',
        status: 'modified',
        line_ending: 'crlf',
        base_content: 'class Plugin { String name = "Eté"; }\r\n',
        base_hash: 'a'.repeat(64),
        proposed_content: 'class Plugin { String name = "Été"; }\r\n',
        proposed_hash: 'b'.repeat(64),
        proposed_bytes: 43,
        additions: 1,
        deletions: 1,
        hunks: 1,
      },
      {
        path: 'src/NewListener.java',
        status: 'created',
        line_ending: 'lf',
        proposed_content: 'class NewListener {}\n',
        proposed_hash: 'c'.repeat(64),
        proposed_bytes: 21,
        additions: 3,
        deletions: 0,
        hunks: 1,
      },
    ],
  };
}

describe('OpticCode edit review artifacts', () => {
  it('retains exact Unicode/CRLF snapshots and models a created file with an empty base', () => {
    const store = new ChatEditArtifactStore();
    store.accept('C:\\fixture', diffEvent());

    assert.equal(
      store.content('C:\\fixture', 'plan-review-1', 'src/Plugin.java', 'base'),
      'class Plugin { String name = "Eté"; }\r\n',
    );
    assert.equal(
      store.content('C:\\fixture', 'plan-review-1', 'src/Plugin.java', 'proposed'),
      'class Plugin { String name = "Été"; }\r\n',
    );
    assert.equal(
      store.content('C:\\fixture', 'plan-review-1', 'src/NewListener.java', 'base'),
      '',
    );
  });

  it('binds approvals and transactions without leaking artifacts across workspaces', () => {
    const store = new ChatEditArtifactStore();
    store.accept('C:\\fixture', diffEvent());
    store.accept('C:\\fixture', {
      ...base,
      sequence: 1,
      type: 'approval_required',
      proposal_id: 'plan-review-1',
      approval_request_id: 'apply-confirmation-1',
      operation: 'apply',
      summary: 'Apply one verified proposal.',
    });
    store.accept('C:\\fixture', {
      ...base,
      sequence: 2,
      type: 'rollback_available',
      proposal_id: 'plan-review-1',
      transaction_id: 'apply-transaction-1',
    });

    assert.equal(store.get('C:\\fixture', 'plan-review-1')?.approvalRequestId, 'apply-confirmation-1');
    assert.equal(
      store.findByTransaction('C:\\fixture', 'apply-transaction-1')?.proposalId,
      'plan-review-1',
    );
    assert.equal(store.get('C:\\other', 'plan-review-1'), undefined);
  });

  it('removes only the selected proposal after a discard event', () => {
    const store = new ChatEditArtifactStore();
    store.accept('C:\\fixture', diffEvent());
    store.accept('C:\\fixture', {
      ...base,
      sequence: 1,
      type: 'proposal_discarded',
      proposal_id: 'plan-review-1',
    });
    assert.equal(store.get('C:\\fixture', 'plan-review-1'), undefined);
  });
});

describe('OpticCode edit review presentation', () => {
  it('offers review controls for a diff and Apply only for an explicit approval event', () => {
    const presenter = new ChatEventPresenter();
    const diff = presenter.accept(diffEvent());
    assert.ok(diff.some((operation) => operation.kind === 'button' && operation.title === 'Show Diff'));
    assert.ok(diff.some((operation) => operation.kind === 'button' && operation.title === 'Show All Changes'));
    assert.ok(diff.some((operation) => operation.kind === 'button' && operation.title === 'Discard Proposal'));
    assert.equal(diff.some((operation) => operation.kind === 'button' && operation.title.includes('Apply')), false);

    const approval = presenter.accept({
      ...base,
      sequence: 1,
      type: 'approval_required',
      proposal_id: 'plan-review-1',
      approval_request_id: 'apply-confirmation-1',
      operation: 'apply',
      summary: 'Apply verified changes.',
    });
    assert.ok(
      approval.some(
        (operation) =>
          operation.kind === 'button' && operation.title === 'Apply Verified Changes',
      ),
    );
  });
});
