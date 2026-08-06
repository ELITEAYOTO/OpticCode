# Contributing to OpticCode

Thank you for contributing to OpticCode.

OpticCode combines a Rust workspace, a local-model integration layer, bounded
code-analysis tools, transactional edit workflows, and an experimental VS Code
extension. Changes must preserve deterministic machine-readable contracts,
workspace safety, bounded resource use, and compatibility with supported
schema versions.

## Before starting

1. Search existing issues and pull requests for related work.
2. Base new work on the latest `master`.
3. Keep one branch focused on one coherent change.
4. Do not mix unrelated cleanup with functional work.

Recommended branch prefixes:

- `feat/` for new capabilities;
- `fix/` for bug fixes;
- `chore/` for maintenance and repository work;
- `docs/` for documentation-only changes;
- `test/` for test-only changes.

Example:

```powershell
git switch master
git pull --ff-only origin master
git switch -c feat/example-change
```

## Development requirements

The main development environment is Windows, while Rust portability is checked
on Linux in CI.

Required tools depend on the affected area:

- stable Rust with `rustfmt` and Clippy;
- Node.js 22 and npm for the VS Code extension;
- Git;
- Java, Maven, or Gradle only for tests that need them;
- Ollama and the configured local model only for real provider integration
  tests.

Never commit:

- model files;
- generated RAG indexes;
- build outputs;
- local benchmark runs;
- secrets, tokens, private keys, or `.env` files;
- personal absolute paths;
- abandoned OpticCode worktrees or transaction artifacts.

## Coding expectations

- Keep behavior bounded and deterministic where practical.
- Preserve exact JSON and NDJSON protocol contracts.
- Treat unknown schema fields, invalid states, ambiguous symbols, path escapes,
  and unsafe writes as fail-closed conditions.
- Do not add shell composition when direct process arguments are sufficient.
- Do not weaken Git-state, worktree, transaction, approval, or path-safety
  checks to make a test pass.
- Add focused tests for every bug fix and meaningful behavior change.
- Update documentation when commands, schemas, configuration, safety behavior,
  or release behavior changes.

## Commit guidance

Use concise imperative commit messages. Conventional prefixes are preferred:

```text
feat(scope): add capability
fix(scope): reject invalid state
test(scope): cover regression
docs(scope): explain behavior
chore(scope): maintain repository
```

A commit records a local project state. A push sends commits to GitHub. A pull
request proposes merging one branch into another.

Keep commits reviewable. Generated files and unrelated formatting changes should
not be included.

## Validation

Run the smallest relevant tests while developing, then run the complete gate
before requesting review.

### Rust

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo build --workspace --release
```

### VS Code extension

```powershell
npm --prefix .\extensions\vscode-opticcode ci
npm --prefix .\extensions\vscode-opticcode run compile
npm --prefix .\extensions\vscode-opticcode run lint
npm --prefix .\extensions\vscode-opticcode test
```

### Complete offline gate

This is the gate used by public GitHub-hosted runners. It does not require
Ollama:

```powershell
powershell -ExecutionPolicy Bypass `
  -File .\scripts\run-vscode-quality.ps1 `
  -SkipRealIntegration
```

### Complete local gate

When Ollama and the configured model are available:

```powershell
powershell -ExecutionPolicy Bypass `
  -File .\scripts\run-vscode-quality.ps1
```

Provider-facing changes should also run the real LLM smoke:

```powershell
powershell -ExecutionPolicy Bypass `
  -File .\scripts\run-vscode-quality.ps1 `
  -WithLlm
```

Before committing:

```powershell
git diff --check
git status -sb
```

## Pull requests

A pull request should:

- target `master`;
- contain one coherent change;
- explain behavior and safety impact;
- list the exact validation performed;
- include documentation and tests where relevant;
- avoid unrelated generated artifacts;
- remain open until required CI checks pass.

Do not claim that a test, benchmark, provider smoke, or platform validation ran
unless it actually ran.

## Security reports

Do not publish exploit details, credentials, secret material, or a working
proof of concept in a public issue. Follow [SECURITY.md](SECURITY.md).

## License

By contributing, you agree that your contribution is licensed under the
repository's MIT License.
