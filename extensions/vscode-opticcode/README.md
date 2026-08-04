# OpticCode for VS Code

Native VS Code client for the local OpticCode Rust CLI. The
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
code --install-extension ..\..\artifacts\opticcode-vscode-0.2.0.vsix --force
```

No global npm package is required. The package output is:

```text
artifacts/opticcode-vscode-0.2.0.vsix
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

The native Chat view also exposes `@opticcode`. With no slash command it runs
Ask. Available read-only commands are `/ask`, `/plan`, `/context`, `/analyze`,
`/index`, `/legacy`, `/status`, `/runs`, and `/help`. `/fix`, `/verify`,
`/diff`, `/apply`, and `/rollback` implement the verified edit workflow.
Every request starts in `read_only`; Rust alone may authorize the bounded
`worktree_edit` and `approved_apply` stages through POLICY-001.

Attach files or precise selections with the Chat context controls. OpticCode
keeps paths workspace-relative, validates them again in Rust, refuses sensitive
or linked files, and shows why each user reference, discovered context file and
RAG hit was used.

Findings open the exact file range. Diagnostics are removed when that document
changes. The only CodeAction is `Verify with OpticCode in Disposable Worktree`;
it never edits the original document.

## Streaming and cancellation

Ask and Plan validate both `opticcode.assistant` and nested `opticcode.llm`
events, monotonic sequences, request IDs, output bounds, and exactly one
terminal event. VS Code cancellation writes `cancel\n` to the child process.
Only a terminal `cancelled` event is shown as confirmed cancellation. A forced
process termination is reported as an interruption.

Chat uses the versioned `opticcode.chat` stdin/NDJSON protocol. History is
bounded to recent turns and no prompt/source content is persisted in session
metadata. Connections and run IDs are isolated per workspace.

The first `request_accepted` event includes the Policy version, decision,
stable rule ID, requested mode and effective mode. These values come from Rust;
the extension only validates and presents them.

## Worktree safety

`/fix` creates a detached disposable worktree, applies verified edits there,
runs Maven or Gradle offline, captures exact snapshots and a Git diff, then
cleans up. Review uses read-only `opticcode-base:` and
`opticcode-proposed:` documents. Only a native modal can trigger the one-shot
Policy approval and APPLY-001 transaction on the original project. There is no
automatic transfer, `--allow-dirty`, arbitrary shell, Git commit, or Git push.

## First test

1. Open `benchmarks/java-index-mini` in VS Code.
2. Open Chat and run `@opticcode /help`.
3. Run `@opticcode /status`.
4. Attach `Helpers.java` and ask `@opticcode /context Locate Helpers#ping().`.
5. Select a precise range and run `@opticcode /ask Explain this code.`.
6. Inspect references, context, token counts and the full report.
7. Open a legacy fixture and run the existing read-only proposal command.
8. On a temporary clean Git fixture, run `@opticcode /fix <small change>`.
9. Review `Show Diff`, verify that the original is unchanged, then test Apply
   and Rollback through their native confirmation modals.

## Limitations

- This is an experimental local extension, not an LSP or autonomous agent.
- It stores session reports in VS Code global extension storage.
- Non-streaming long operations cannot claim provider-confirmed cancellation.
- `compare` may produce a context comparison without model text unless double
  generation is explicitly authorized at the CLI level.
- Delete, rename, binary edits and autonomous multi-iteration remain disabled.
- Apply and rollback require a clean main worktree, exact proposal state and a
  native VS Code confirmation; typed Chat approval is deliberately ignored.
