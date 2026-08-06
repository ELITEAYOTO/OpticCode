import * as assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import { OpticCodeClientError } from '../../src/protocol/errors';
import { validateVersionReport } from '../../src/protocol/validation';

function legacyReport(): Record<string, unknown> {
  return {
    schema_version: 1,
    protocol: 'opticcode.discovery',
    opticcode_version: '0.1.0',
    protocols: {
      assistant: { id: 'opticcode.assistant', schema_version: 1 },
      chat: { id: 'opticcode.chat', schema_version: 1 },
      discovery: { id: 'opticcode.discovery', schema_version: 1 },
      llm: { id: 'opticcode.llm', schema_version: 1 },
    },
    schemas: {},
    platform: {
      os: 'windows',
      architecture: 'x86_64',
    },
    build: {
      kind: 'test',
    },
  };
}

function enrichedReport(): Record<string, unknown> {
  return {
    ...legacyReport(),
    protocols: {
      assistant: { id: 'opticcode.assistant', schema_version: 1 },
      chat: { id: 'opticcode.chat', schema_version: 1 },
      discovery: { id: 'opticcode.discovery', schema_version: 1 },
      llm: { id: 'opticcode.llm', schema_version: 1 },
      policy: { id: 'opticcode.policy', schema_version: 1 },
    },
    platform: {
      os: 'windows',
      architecture: 'x86_64',
      target: 'x86_64-pc-windows-msvc',
    },
    build: {
      kind: 'debug',
      profile: 'debug',
      commit: '5344d320c5f1dc6a8669040ce1c8b65c7192dd15',
      commit_short: '5344d320',
      dirty: true,
    },
  };
}

function rejectsProtocol(value: unknown, message: RegExp): void {
  assert.throws(
    () => validateVersionReport(value),
    (error: unknown) =>
      error instanceof OpticCodeClientError &&
      error.code === 'protocol_incompatible' &&
      message.test(error.message),
  );
}

describe('OpticCode version report validation', () => {
  it('keeps legacy schema-one reports compatible', () => {
    const report = validateVersionReport(legacyReport());

    assert.equal(report.platform.target, undefined);
    assert.equal(report.protocols.policy, undefined);
    assert.equal(report.build.profile, undefined);
    assert.equal(report.build.commit, undefined);
  });

  it('accepts complete build provenance metadata', () => {
    const report = validateVersionReport(enrichedReport());

    assert.equal(report.protocols.policy?.id, 'opticcode.policy');
    assert.equal(report.platform.target, 'x86_64-pc-windows-msvc');
    assert.equal(report.build.profile, 'debug');
    assert.equal(report.build.commit_short, '5344d320');
    assert.equal(report.build.dirty, true);
  });

  it('rejects malformed full commits', () => {
    const report = enrichedReport();
    const build = report.build as Record<string, unknown>;
    build.commit = 'not-a-commit';

    rejectsProtocol(report, /Invalid build commit/);
  });

  it('rejects a short commit that differs from the full commit', () => {
    const report = enrichedReport();
    const build = report.build as Record<string, unknown>;
    build.commit_short = 'deadbeef';

    rejectsProtocol(report, /does not match/);
  });

  it('rejects a short commit without a full commit', () => {
    const report = legacyReport();
    const build = report.build as Record<string, unknown>;
    build.commit_short = '5344d320';

    rejectsProtocol(report, /without a full commit/);
  });

  it('rejects invalid dirty-state types', () => {
    const report = enrichedReport();
    const build = report.build as Record<string, unknown>;
    build.dirty = 'true';

    rejectsProtocol(report, /Expected boolean at build\.dirty/);
  });

  it('rejects invalid policy protocol descriptors', () => {
    const report = enrichedReport();
    const protocols = report.protocols as Record<string, unknown>;
    protocols.policy = {
      id: 'opticcode.wrong-policy',
      schema_version: 1,
    };

    rejectsProtocol(report, /Unsupported policy protocol descriptor/);
  });
});
