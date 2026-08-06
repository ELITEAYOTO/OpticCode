## Summary

<!-- What changed and why? Keep this focused on one coherent change. -->

## Scope

<!-- Which crates, protocols, commands, extension areas, or documents changed? -->

## Safety impact

<!--
Describe any effect on:
- workspace or path confinement
- process execution
- Git state or worktrees
- transactions, approvals, apply, or rollback
- secrets, prompts, logs, RAG, or packaged artifacts
- machine-readable protocol compatibility
Write "None" only when there is genuinely no safety impact.
-->

## Validation

- [ ] `git diff --check`
- [ ] Rust formatting
- [ ] Rust Clippy with warnings denied
- [ ] Rust tests
- [ ] Rust release build
- [ ] TypeScript compilation
- [ ] ESLint
- [ ] VS Code unit tests
- [ ] Offline quality gate
- [ ] Real CLI integration, when applicable
- [ ] Real LLM smoke, when applicable
- [ ] Extension Host test, when applicable

List commands actually executed and summarize important results:

```text
<!-- Commands and results -->
```

## Protocol and compatibility

- [ ] No machine-readable contract changed
- [ ] Contract changes are additive and backward compatible
- [ ] Schema/version change is documented and tested
- [ ] Not applicable

<!-- Explain the selected case. -->

## Documentation

- [ ] Documentation was updated
- [ ] No documentation change is required

## Release impact

- [ ] No release required
- [ ] CLI release candidate
- [ ] VS Code extension release candidate
- [ ] Both artifacts may require a release

## Checklist

- [ ] The branch is based on the latest intended base branch
- [ ] The change contains no secrets, model files, generated indexes, or
      personal absolute paths
- [ ] New behavior has focused tests
- [ ] Claims above reflect tests that actually ran
