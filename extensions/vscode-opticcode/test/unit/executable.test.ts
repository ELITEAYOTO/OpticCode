import * as assert from 'node:assert/strict';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import * as os from 'node:os';
import * as path from 'node:path';
import { describe, it } from 'node:test';

import { resolveOpticCodeExecutable } from '../../src/executable';
import { OpticCodeClientError } from '../../src/protocol/errors';

async function fixture(): Promise<{ root: string; executable: string }> {
  const root = await mkdtemp(path.join(os.tmpdir(), 'opticcode vscode \u00e9t\u00e9 '));
  const executable = path.join(
    root,
    'target',
    'release',
    process.platform === 'win32' ? 'opticcode.exe' : 'opticcode',
  );
  await mkdir(path.dirname(executable), { recursive: true });
  await writeFile(path.join(root, 'Cargo.toml'), '[workspace]\n');
  await writeFile(executable, 'fixture');
  if (process.platform !== 'win32') {
    const { chmod } = await import('node:fs/promises');
    await chmod(executable, 0o755);
  }
  return { root, executable };
}

describe('executable resolution', () => {
  it('prioritizes an absolute configured path with spaces and Unicode', async () => {
    const configured = await fixture();
    const workspace = await fixture();
    try {
      const result = await resolveOpticCodeExecutable(
        configured.executable,
        [workspace.root],
        workspace.root,
      );
      assert.equal(result.path, configured.executable);
      assert.equal(result.source, 'configured');
      assert.equal(result.workingDirectory, configured.root);
    } finally {
      await rm(configured.root, { recursive: true, force: true });
      await rm(workspace.root, { recursive: true, force: true });
    }
  });

  it('detects only the bounded development candidate', async () => {
    const workspace = await fixture();
    try {
      const result = await resolveOpticCodeExecutable('', [workspace.root], workspace.root);
      assert.equal(result.path, workspace.executable);
      assert.equal(result.source, 'workspace-development');
    } finally {
      await rm(workspace.root, { recursive: true, force: true });
    }
  });

  it('reports an absent executable without searching the disk', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'opticcode-empty-'));
    try {
      await assert.rejects(
        resolveOpticCodeExecutable('', [root], root),
        (error: unknown) =>
          error instanceof OpticCodeClientError && error.code === 'executable_not_found',
      );
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });
});
