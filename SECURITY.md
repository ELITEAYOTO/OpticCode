# Security Policy

## Project status

OpticCode is pre-1.0 software under active development. It includes local
process execution, Git inspection, worktree management, transactional file
updates, local-model communication, and code-analysis features. Security
reports involving workspace confinement, command execution, approval binding,
secret exposure, unsafe path handling, or rollback integrity are treated as
high priority.

No stable release line is currently supported. Reports should target the latest
published release when one exists, or the latest `master` revision when no
release is available.

## Reporting a vulnerability

Do not disclose sensitive vulnerability details in a public issue.

Preferred reporting path:

1. Use GitHub's **Report a vulnerability** option in the repository Security
   section when it is available.
2. Include the affected commit or version, operating system, reproduction
   conditions, impact, and the smallest safe proof of concept.
3. Remove credentials, private source code, personal paths, model data, and
   unrelated workspace content from the report.

When private vulnerability reporting is not available, open a public issue that
contains only a request for a private reporting channel. Do not include exploit
steps, payloads, secrets, or sensitive logs in that issue.

## Useful report details

A useful report includes:

- affected OpticCode commit, CLI version, or extension version;
- operating system and relevant tool versions;
- whether the repository was clean or dirty;
- the command or protocol operation involved;
- expected and observed safety behavior;
- whether files, Git state, worktrees, transactions, approvals, or secrets were
  affected;
- minimal sanitized logs;
- whether the behavior reproduces after a clean build.

## Security-sensitive areas

Examples include:

- path traversal, junction, symlink, or reparse-point escapes;
- writes outside an authorized worktree or transaction;
- approval reuse, cross-workspace reuse, or approval bypass;
- command injection or unexpected shell execution;
- secret inclusion in prompts, logs, reports, indexes, diffs, or VSIX packages;
- unsafe rollback, recovery, or concurrent transaction behavior;
- Git-state validation bypasses;
- unbounded process output, parsing, memory use, or model output;
- protocol confusion across request IDs, schemas, sequences, or terminal events;
- remote-provider access where only local endpoints are allowed.

## Disclosure

Please allow time to reproduce, fix, test, and publish a coordinated update
before public disclosure. The project does not currently promise a fixed
response-time service level, but confirmed high-impact issues will be
prioritized.

## Scope limitations

A report is generally out of scope when it only demonstrates:

- behavior that requires manually disabling documented safety checks;
- compromise of the operating system, Git executable, compiler, Node.js
  runtime, Ollama installation, or model files before OpticCode starts;
- denial of service caused solely by intentionally exhausting the host outside
  OpticCode's configured bounds;
- model-quality disagreements without a security boundary failure.

Reports that reveal a real boundary failure remain in scope even when a local
model produced the triggering content.
