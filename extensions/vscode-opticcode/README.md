# OpticCode for VS Code

Experimental native VS Code client for the local OpticCode Rust CLI. The
extension launches `opticcode.exe` directly and consumes the versioned JSON and
NDJSON protocols. It does not implement Java parsing, RAG, LLM inference, Git,
worktrees, transactions, or Minecraft legacy rules in TypeScript.

## Prerequisites

- VS Code 1.125 or newer;
- the OpticCode Rust repository and a release build;
- Git and Java 8 for project inspection;
- Maven or Gradle when worktree verification must build a project;
- Ollama with the configured model for Ask and Plan.

Build the CLI from the repository root:

```powershell
cargo build --workspace --release
```

The extension does not bundle `opticcode.exe`. Set
`opticcode.executablePath` to its absolute path, or use the development layout
`target/release/opticcode.exe` under the open OpticCode workspace. When this
extension is run from its source directory, it can also resolve the repository
development build without scanning the disk.

## Build and install

```powershell
cd extensions\vscode-opticcode
npm install
npm run compile
npm run lint
npm test
npm run package
code --install-extension ..\..\artifacts\opticcode-vscode-0.1.0.vsix
```

No global npm package is required. The package output is:

```text
artifacts/opticcode-vscode-0.1.0.vsix
```

## Configuration

- `opticcode.executablePath`: absolute CLI path; always takes priority;
- `opticcode.profile`: defaults to `minecraft-java-1.8`;
- `opticcode.model`: defaults to `qwen2.5-coder:14b`;
- `opticcode.contextMode`: `legacy`, `symbol`, or `compare`;
- `opticcode.defaultTimeoutSeconds`: bounded process timeout;
- `opticcode.showDebugOutput`: protocol lifecycle in the OutputChannel;
- `opticcode.autoCheckOnStartup`: read-only discovery after activation.

`legacy` remains the default. `symbol` is opt-in. `compare` does not silently
authorize two generations; it follows the CLI's explicit cost policy.

## Commands and views

The `OpticCode` activity container has native `Status`, `Findings`, and `Runs`
views. Commands cover installation/status, profile selection, Java syntax and
symbol context, read-only legacy proposals, worktree verification, streamed Ask
and Plan, report viewing, lease recovery, and the OutputChannel.

Findings open the exact file range. Diagnostics are removed when that document
changes. The only CodeAction is `Verify with OpticCode in Disposable Worktree`;
it never edits the original document.

## Streaming and cancellation

Ask and Plan validate both `opticcode.assistant` and nested `opticcode.llm`
events, monotonic sequences, request IDs, output bounds, and exactly one
terminal event. VS Code cancellation writes `cancel\n` to the child process.
Only a terminal `cancelled` event is shown as confirmed cancellation. A forced
process termination is reported as an interruption.

## Worktree safety

Verification always asks for confirmation. OpticCode creates a detached
disposable worktree, applies verified edits there, optionally runs Maven or
Gradle, captures the diff, and cleans up. The report keeps edit, build, diff,
and cleanup outcomes separate. There is no automatic transfer to the original
project, no `--allow-dirty`, no arbitrary shell, and no Git push.

## First test

1. Open `benchmarks/java-index-mini` in VS Code.
2. Run `OpticCode: Check Installation`.
3. Run `OpticCode: Refresh Status`.
4. Run `OpticCode: Build Smart Context`.
5. Run `OpticCode: Ask Qwen`.
6. Open a legacy fixture and run `OpticCode: Propose Minecraft Legacy Fixes`.
7. Run `OpticCode: Verify Proposed Fixes in Worktree` only from a clean Git project.
8. Inspect the report and confirm the original project is unchanged.

## Limitations

- This is an experimental local extension, not an LSP or autonomous agent.
- It stores session reports in VS Code global extension storage.
- Non-streaming long operations cannot claim provider-confirmed cancellation.
- `compare` may produce a context comparison without model text unless double
  generation is explicitly authorized at the CLI level.
- The extension has no automatic patch application to the original workspace.
