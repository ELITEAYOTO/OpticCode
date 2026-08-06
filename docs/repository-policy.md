# Repository Policy

This document describes the intended repository settings once the CI workflow
has been merged into `master` and has completed successfully.

## Default branch

The default branch is `master`.

Feature and maintenance work should happen on focused branches. Direct work on
`master` should be avoided.

## Recommended branch protection

After the CI checks exist on `master`, protect `master` with:

- require a pull request before merging;
- require status checks to pass;
- require branches to be up to date before merging;
- require conversation resolution;
- block force pushes;
- block branch deletion.

Required checks:

```text
Rust / Linux
Windows offline quality gate
```

While the repository has only one active maintainer, do not require an
independent approving review if that would make merging impossible. Enable at
least one required approval when another trusted reviewer is available.

Do not require signed commits yet: existing repository history contains unsigned
commits. Commit-signing policy can be introduced separately after tooling and
history expectations are documented.

## Merge strategy

Keep pull requests focused and preserve a clear project history.

A normal merge commit is acceptable. Squash merging is also acceptable for a
branch whose intermediate commits do not provide lasting value. Do not rebase or
force-push a shared branch after review has started unless reviewers are
notified.

## CI boundaries

Public CI is intentionally offline with respect to local model providers. It
must validate deterministic Rust and extension behavior without depending on a
developer's Ollama installation or model inventory.

Real provider integration and LLM smoke tests remain required locally for
provider-facing changes and releases.

## Releases

Repository merges do not automatically create releases. Releases follow
[Release Policy](releases.md) and use artifact-specific tags.

## Administrative changes

Changes to branch protection, repository visibility, merge methods, Actions
permissions, secrets, environments, or release automation should be recorded in
a focused pull request or repository-maintenance note.
