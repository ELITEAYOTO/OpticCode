import { constants } from 'node:fs';
import { access, stat } from 'node:fs/promises';
import * as path from 'node:path';

import { OpticCodeClientError } from './protocol/errors';

export type ExecutableSource = 'configured' | 'workspace-development' | 'extension-development';

export interface ResolvedExecutable {
  path: string;
  workingDirectory: string;
  source: ExecutableSource;
}

function executableName(): string {
  return process.platform === 'win32' ? 'opticcode.exe' : 'opticcode';
}

async function isExecutableFile(candidate: string): Promise<boolean> {
  try {
    const metadata = await stat(candidate);
    if (!metadata.isFile()) {
      return false;
    }
    await access(candidate, process.platform === 'win32' ? constants.F_OK : constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

async function isRegularFile(candidate: string): Promise<boolean> {
  try {
    return (await stat(candidate)).isFile();
  } catch {
    return false;
  }
}

function developmentCandidate(root: string): string {
  return path.join(root, 'target', 'release', executableName());
}

async function configuredWorkingDirectory(
  executable: string,
  workspaceFallback: string | undefined,
): Promise<string> {
  const releaseDirectory = path.dirname(executable);
  const targetDirectory = path.dirname(releaseDirectory);
  const repository = path.dirname(targetDirectory);
  if (
    path.basename(releaseDirectory).toLowerCase() === 'release' &&
    path.basename(targetDirectory).toLowerCase() === 'target' &&
    (await isRegularFile(path.join(repository, 'Cargo.toml')))
  ) {
    return repository;
  }
  return workspaceFallback ?? path.dirname(executable);
}

export async function resolveOpticCodeExecutable(
  configuredPath: string,
  workspaceFolders: readonly string[],
  extensionPath: string,
): Promise<ResolvedExecutable> {
  const configured = configuredPath.trim();
  if (configured !== '') {
    if (!path.isAbsolute(configured)) {
      throw new OpticCodeClientError(
        'executable_not_found',
        'opticcode.executablePath must be absolute.',
      );
    }
    if (!(await isExecutableFile(configured))) {
      throw new OpticCodeClientError(
        'executable_not_found',
        `Configured OpticCode executable does not exist: ${configured}`,
      );
    }
    return {
      path: configured,
      workingDirectory: await configuredWorkingDirectory(configured, workspaceFolders[0]),
      source: 'configured',
    };
  }

  for (const workspace of workspaceFolders) {
    const candidate = developmentCandidate(workspace);
    if (await isExecutableFile(candidate)) {
      return {
        path: candidate,
        workingDirectory: workspace,
        source: 'workspace-development',
      };
    }
  }

  const repositoryRoot = path.resolve(extensionPath, '..', '..');
  const extensionCandidate = developmentCandidate(repositoryRoot);
  if (await isExecutableFile(extensionCandidate)) {
    return {
      path: extensionCandidate,
      workingDirectory: repositoryRoot,
      source: 'extension-development',
    };
  }

  throw new OpticCodeClientError(
    'executable_not_found',
    'OpticCode executable was not configured and no development build was found.',
  );
}
