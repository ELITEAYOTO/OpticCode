# Release Policy

OpticCode currently produces two independently versioned artifacts:

1. the Rust workspace and `opticcode` CLI;
2. the experimental VS Code extension.

Their versions do not need to match. The CLI may remain at `0.1.x` while the
extension advances through `0.2.x`, because they have different packaging,
compatibility, and user-interface concerns.

## Version sources

The authoritative version sources are:

- CLI and Rust crates: each crate's `Cargo.toml`;
- VS Code extension: `extensions/vscode-opticcode/package.json`.

Generated artifact names, manifests, documentation, and release notes must agree
with the corresponding authoritative version.

## Versioning model

Before `1.0.0`, OpticCode follows a conservative interpretation of Semantic
Versioning:

- patch: compatible fixes, tests, documentation, packaging, or internal
  hardening;
- minor: new user-visible capabilities or additive protocol behavior;
- major: reserved for stable post-1.0 compatibility breaks.

Pre-1.0 compatibility changes must still be explicit. A breaking protocol,
configuration, persistent-state, or CLI change must be documented even when a
minor version bump is technically permitted.

## Git tags

Use artifact-specific tags so CLI and extension releases cannot collide:

```text
cli-v0.1.0
vscode-v0.2.1
```

Do not use an unqualified tag such as `v0.2.1` while artifacts are versioned
independently.

## Release prerequisites

A release candidate must satisfy all applicable requirements:

- release commit is on `master`;
- repository state is clean;
- required GitHub Actions checks pass;
- `git diff --check` passes;
- Rust formatting, Clippy, tests, and release build pass;
- extension compilation, lint, unit tests, and VSIX packaging pass;
- real CLI integration passes for release environments that support Ollama;
- provider-facing changes run the real LLM smoke;
- no abandoned OpticCode worktree leases remain;
- documentation and release notes describe user-visible and compatibility
  changes;
- packaged artifacts contain no personal paths, secrets, source-only test data,
  model files, generated indexes, or build directories.

## Build provenance

Release builds should embed the exact source commit and a clean dirty-state
marker.

A controlled release environment may set:

```powershell
$env:OPTICCODE_GIT_COMMIT = "<full commit hash>"
$env:OPTICCODE_GIT_DIRTY = "false"
```

No compilation timestamp is embedded. This avoids unnecessary
non-reproducibility.

Before publishing, verify:

```powershell
.\target\release\opticcode.exe version
```

The output must show the expected release profile, commit, target, and
`state=clean`.

## CLI release artifacts

A CLI release should include, as applicable:

- a platform-labelled archive containing the executable;
- `LICENSE`;
- concise installation and verification instructions;
- a SHA-256 checksum file;
- release notes describing supported platforms and required external tools.

Do not publish local model files, private configurations, RAG indexes, or user
workspace data.

## VS Code extension release artifacts

A VS Code extension release should include:

- the versioned `.vsix`;
- its SHA-256 checksum;
- installation instructions;
- required CLI compatibility information;
- release notes for commands, configuration, protocols, and safety behavior.

The quality gate already inspects VSIX contents for forbidden files and personal
path markers. A release must use the artifact produced by the validated commit.

## GitHub release procedure

Initial releases are manual:

1. update the authoritative version;
2. update documentation and release notes;
3. run the complete quality gate;
4. commit and merge through a pull request;
5. build from the clean `master` commit;
6. verify embedded provenance;
7. calculate SHA-256 checksums;
8. create the artifact-specific Git tag;
9. create a GitHub release from that tag;
10. upload only the verified artifacts and checksum files.

Automated publishing may be added later, but it must preserve the same
provenance, validation, and artifact-inspection requirements.

## Release notes

Release notes should separate:

- added capabilities;
- fixes;
- security and safety hardening;
- protocol or configuration changes;
- compatibility notes;
- known limitations;
- validation performed.

Do not claim support for a platform, provider, model, project type, or migration
that was not tested.
