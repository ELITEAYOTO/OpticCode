import { runTests } from '@vscode/test-electron';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const directory = path.dirname(fileURLToPath(import.meta.url));
const extensionDevelopmentPath = path.resolve(directory, '..');
const extensionTestsPath = path.resolve(extensionDevelopmentPath, 'out', 'test', 'vscode', 'index.js');
const fixture = path.resolve(extensionDevelopmentPath, '..', '..', 'benchmarks', 'java-index-mini');

await runTests({
  extensionDevelopmentPath,
  extensionTestsPath,
  launchArgs: [fixture, '--disable-extensions'],
});
