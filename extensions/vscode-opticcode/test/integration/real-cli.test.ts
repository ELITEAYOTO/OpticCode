import * as assert from 'node:assert/strict';
import * as path from 'node:path';
import { describe, it } from 'node:test';

import { OpticCodeProtocolClient } from '../../src/protocol/client';
import {
  isRecord,
  validateCapabilitiesReport,
  validateDoctorReport,
  validateVersionReport,
} from '../../src/protocol/validation';

const repository = process.env.OPTICCODE_REPO_ROOT ?? path.resolve(process.cwd(), '..', '..');
const executable =
  process.env.OPTICCODE_EXE ??
  path.join(
    repository,
    'target',
    'release',
    process.platform === 'win32' ? 'opticcode.exe' : 'opticcode',
  );
const client = new OpticCodeProtocolClient({
  executablePath: executable,
  workingDirectory: repository,
  timeoutMs: 30_000,
});

describe('real opticcode.exe integration', () => {
  it('validates version, build provenance, and capabilities', async () => {
    const version = await client.runJson(['version', '--json'], validateVersionReport);
    const capabilities = await client.runJson(
      ['capabilities', '--json'],
      validateCapabilitiesReport,
    );

    assert.equal(version.protocols.assistant.schema_version, 1);
    assert.equal(version.protocols.policy?.id, 'opticcode.policy');

    assert.ok(
      typeof version.platform.target === 'string' &&
        version.platform.target.trim().length > 0,
    );
    assert.equal(version.build.kind, 'release');
    assert.equal(version.build.profile, 'release');

    const commit = version.build.commit;
    assert.ok(typeof commit === 'string');
    assert.match(commit, /^[0-9a-f]{40,64}$/);
    assert.equal(version.build.commit_short, commit.slice(0, 8));
    assert.ok(typeof version.build.dirty === 'boolean');

    assert.ok(capabilities.commands.includes('java-context'));
  });

  it('renders a real doctor report', async () => {
    const report = await client.runJson(
      [
        'doctor',
        '--json',
        '--path',
        path.join(repository, 'benchmarks', 'java-index-mini'),
        '--profile',
        'minecraft-java-1.8',
        '--rag-index',
        path.join(repository, 'data', 'index'),
        '--timeout-ms',
        '5000',
      ],
      validateDoctorReport,
    );

    assert.equal(report.success, true);
    assert.ok(report.checks.some((check) => check.id === 'configured_model'));
  });

  it('builds real symbol-guided context for java-index-mini', async () => {
    const report = await client.runJson(
      [
        'java-context',
        'Locate dev.opticcode.util.Helpers#ping().',
        '--json',
        '--path',
        path.join(repository, 'benchmarks', 'java-index-mini'),
      ],
      (value) => {
        assert.ok(isRecord(value));
        return value;
      },
    );

    assert.equal(report.operation, 'java_task_context');
    assert.equal(report.analysis_complete, true);
  });
});