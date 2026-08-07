import * as assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import { OpticCodeClientError } from '../../src/protocol/errors';
import {
  validateCapabilitiesReport,
  validateChatEvent,
} from '../../src/protocol/validation';

function capabilities(): Record<string, unknown> {
  return {
    schema_version: 1,
    protocol: 'opticcode.discovery',
    commands: ['version', 'capabilities', 'doctor', 'ask', 'plan', 'chat'],
    providers: [
      {
        id: 'ollama',
        active: true,
        capabilities: {
          local_only: true,
          health: true,
          model_listing: true,
          generation: true,
          streaming: true,
          cancellation: true,
          token_usage: true,
          provider_timings: true,
          deterministic_seed: true,
        },
      },
    ],
    context_modes: ['legacy', 'symbol', 'compare'],
    machine_output: {
      json: true,
      ndjson: true,
      streaming: true,
      cancellation: true,
    },
    features: {
      chat: true,
      policy: true,
      rag: true,
      java: true,
      worktrees: true,
      verified_edits: true,
      evaluation: true,
    },
    policy_runtime: {
      schema_version: 1,
      policy_version: 'opticcode.default.v1',
      engine: true,
      modes: ['read_only', 'worktree_edit', 'approved_apply'],
      audit: true,
      approvals: true,
      cli: true,
      chat_read_only: true,
      chat_write: true,
    },
    edit_runtime: {
      intent_schema_version: 1,
      plan_schema_version: 1,
      proposal_store_schema_version: 1,
      hash_algorithm: 'blake3',
      task_persistence: 'hash_only',
      offset_encoding: 'utf8_bytes',
      selection_modes: ['explicit_references'],
      operations: ['modify_existing'],
      validations: ['reparse_java', 'build_offline', 'test_offline'],
      max_intent_targets: 16,
      max_files: 5,
      max_created_files: 0,
      max_file_bytes: 524288,
      max_hunks: 64,
      max_changed_lines: 2000,
      global_timeout_seconds: 900,
      clean_worktree_required: true,
      offline_verification: true,
      worktree_verification: true,
      native_confirmation: true,
      transactional_apply: true,
      rollback: true,
    },
  };
}

function chatEvent(type: string, fields: Record<string, unknown>): Record<string, unknown> {
  return {
    schema_version: 1,
    protocol: 'opticcode.chat',
    request_id: 'request-edit-protocol-002',
    sequence: 7,
    elapsed_ms: 12,
    type,
    ...fields,
  };
}

function rejectsProtocol(action: () => unknown, message: RegExp): void {
  assert.throws(
    action,
    (error: unknown) =>
      error instanceof OpticCodeClientError &&
      error.code === 'protocol_incompatible' &&
      message.test(error.message),
  );
}

describe('OpticCode edit intent protocol', () => {
  it('accepts detailed edit runtime capabilities while keeping legacy reports compatible', () => {
    const detailed = validateCapabilitiesReport(capabilities());

    assert.equal(detailed.edit_runtime?.intent_schema_version, 1);
    assert.deepEqual(detailed.edit_runtime?.operations, ['modify_existing']);
    assert.equal(detailed.edit_runtime?.max_created_files, 0);
    assert.equal(detailed.edit_runtime?.task_persistence, 'hash_only');

    const legacy = capabilities();
    delete legacy.edit_runtime;
    assert.equal(validateCapabilitiesReport(legacy).edit_runtime, undefined);
  });

  it('rejects capability escalation and inconsistent operation limits', () => {
    const duplicateOperations = capabilities();
    const duplicateRuntime = duplicateOperations.edit_runtime as Record<string, unknown>;
    duplicateRuntime.operations = ['modify_existing', 'modify_existing'];
    rejectsProtocol(
      () => validateCapabilitiesReport(duplicateOperations),
      /edit_runtime\.operations must not contain duplicates/,
    );

    const creationMismatch = capabilities();
    const creationRuntime = creationMismatch.edit_runtime as Record<string, unknown>;
    creationRuntime.max_created_files = 1;
    rejectsProtocol(
      () => validateCapabilitiesReport(creationMismatch),
      /max_created_files must be zero/,
    );

    const oversized = capabilities();
    const oversizedRuntime = oversized.edit_runtime as Record<string, unknown>;
    oversizedRuntime.max_files = 6;
    rejectsProtocol(
      () => validateCapabilitiesReport(oversized),
      /edit_runtime\.max_files/,
    );
  });

  it('validates versioned intent lifecycle events and their proposal binding', () => {
    const started = validateChatEvent(
      chatEvent('edit_intent_started', {
        intent_id: 'intent-0123456789abcdef',
        intent_schema_version: 1,
      }),
      'request-edit-protocol-002',
    );
    assert.equal(started.type, 'edit_intent_started');

    const ready = validateChatEvent(
      chatEvent('edit_intent_ready', {
        intent_id: 'intent-0123456789abcdef',
        intent_schema_version: 1,
        intent_hash: 'a'.repeat(64),
        selection_mode: 'explicit_references',
        target_count: 2,
        expires_at_unix_ms: 1_800_000_900_000,
      }),
      'request-edit-protocol-002',
    );
    assert.equal(ready.type, 'edit_intent_ready');

    const stored = validateChatEvent(
      chatEvent('proposal_stored', {
        proposal_id: 'plan-0123456789abcdef',
        state: 'validated',
        expires_at_unix_ms: 1_800_003_600_000,
        intent_id: 'intent-0123456789abcdef',
        intent_schema_version: 1,
        intent_hash: 'a'.repeat(64),
      }),
      'request-edit-protocol-002',
    );
    assert.equal(stored.type, 'proposal_stored');
  });

  it('rejects malformed intent hashes, versions, target counts, and partial bindings', () => {
    rejectsProtocol(
      () =>
        validateChatEvent(
          chatEvent('edit_intent_ready', {
            intent_id: 'intent-0123456789abcdef',
            intent_schema_version: 1,
            intent_hash: 'not-a-hash',
            selection_mode: 'explicit_references',
            target_count: 1,
            expires_at_unix_ms: 1_800_000_900_000,
          }),
          'request-edit-protocol-002',
        ),
      /BLAKE3 hash/,
    );

    rejectsProtocol(
      () =>
        validateChatEvent(
          chatEvent('edit_intent_started', {
            intent_id: 'intent-0123456789abcdef',
            intent_schema_version: 2,
          }),
          'request-edit-protocol-002',
        ),
      /Unsupported edit intent schema/,
    );

    rejectsProtocol(
      () =>
        validateChatEvent(
          chatEvent('edit_intent_ready', {
            intent_id: 'intent-0123456789abcdef',
            intent_schema_version: 1,
            intent_hash: 'a'.repeat(64),
            selection_mode: 'explicit_references',
            target_count: 17,
            expires_at_unix_ms: 1_800_000_900_000,
          }),
          'request-edit-protocol-002',
        ),
      /target_count/,
    );

    rejectsProtocol(
      () =>
        validateChatEvent(
          chatEvent('proposal_stored', {
            proposal_id: 'plan-0123456789abcdef',
            state: 'validated',
            expires_at_unix_ms: 1_800_003_600_000,
            intent_id: 'intent-0123456789abcdef',
          }),
          'request-edit-protocol-002',
        ),
      /complete intent binding/,
    );
  });
});
