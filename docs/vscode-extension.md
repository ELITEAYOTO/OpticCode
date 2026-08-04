# VSCODE-001 / VSCODE-CHAT-001 - Native VS Code extension

## Scope

`extensions/vscode-opticcode` is an installable thin client for
`opticcode.exe`. It uses native VS Code APIs only: TreeViews, diagnostics,
CodeActions, OutputChannel, status bar, input/quick-pick controls, progress, and
untitled Markdown/JSON documents. V1 intentionally has no webview.

The ownership boundary is strict. VSCODE-CHAT-001 adds a stable native Chat
participant without changing it:

```text
VS Code UI
  -> @opticcode / TreeViews / commands
  -> TypeScript protocol client (spawn, validation, bounds, cancellation)
    -> opticcode.exe JSON / NDJSON
      -> Rust Java/RAG/LLM/Git/worktree/apply implementations
```

No Java, Tree-sitter, RAG, LLM, Git, transaction, legacy rule, or evaluation
algorithm is duplicated in the extension.

## Discovery

Every connection starts with:

```powershell
opticcode.exe version --json
opticcode.exe capabilities --json
```

The client requires discovery, assistant, and LLM schema version 1 and rejects
incompatible identifiers before invoking a feature. `doctor --json` supplies
the Status view. See [`client-discovery.md`](client-discovery.md).

The configured executable path is authoritative. Development detection is
limited to `target/release/opticcode.exe` under an open OpticCode workspace or
the extension's own repository root. There is no PATH fallback and no disk-wide
search.

## Process and protocol rules

- `child_process.spawn` receives an executable and argument array;
- `shell: false`, hidden Windows window, piped stdin/stdout/stderr;
- stdout contains only JSON/NDJSON and stderr goes to `OpticCode` output;
- JSON, line, total stream, stderr, and event counts are bounded;
- UTF-8, schema, protocol, request ID, outer and nested sequences are validated;
- one terminal event is mandatory; zero, two, or an event after terminal fails;
- timeout, process interruption, confirmed cancellation, and forced termination
  remain different outcomes.

Ask/Plan cancellation writes the bounded stdin control command `cancel\n`.
OpticCode forwards it to the provider token. A forced kill is never described as
a clean provider cancellation.

Native Chat uses `opticcode.exe chat --protocol-jsonl`. It writes one structured
`opticcode.chat` request, then a structured `opticcode.chat.control` cancellation
message when needed. The JSONL parser and lifecycle checks are shared with the
Assistant path.

## Native interface

Status renders executable/version/protocol, provider/model/profile, Git,
Ollama, Java, Maven/Gradle, RAG, and worktree/lease checks.

Findings renders file/range, symbol, rule, confidence, decision, reason, and
verification result. Clicking opens the validated range. Java syntax errors,
safe legacy proposals, controlled refusals, and selected context snippets map
to distinct diagnostic severities. Document edits remove stale diagnostics.

Runs retains up to 50 session entries with command, request ID, state, duration,
context, tokens, model, build/worktree summary, and report path.

The Chat view exposes `@opticcode` with `/ask`, `/plan`, `/context`, `/analyze`,
`/index`, `/legacy`, `/status`, `/runs`, `/help`, `/fix`, `/verify`, `/diff`,
`/apply`, and `/rollback`.
Attached files, locations and active Unicode selections become structured
references. Full details are documented in [`vscode-chat.md`](vscode-chat.md).

## Security boundary

The extension exposes original-workspace apply only through a native modal and
a Rust one-shot approval. It does not expose `--allow-dirty`, shell commands,
package installation, Git push, or an autonomous loop. Worktree verification
does not need confirmation and reports edit, build, diff, and cleanup
independently. Recovery is targeted by an explicit OpticCode lease/transaction.

POLICY-001 is enforced in the Rust Chat runtime. Every request begins in
`read_only`, and `request_accepted` reports its decision/rule. Only the trusted
Rust edit orchestrator can request bounded `worktree_edit` or
`approved_apply` stages; a client-supplied edit mode is rejected before
references or tools are reached. TypeScript validates and presents Policy
results but is not a second policy implementation.

Reports are written outside the source workspace to VS Code global extension
storage. The VSIX excludes dependencies, tests, local models, RAG indexes,
benchmarks, personal documents, and build outputs.

## Development and validation

```powershell
cargo build --workspace --release
cd extensions\vscode-opticcode
npm install
npm run compile
npm run lint
npm test
npm run test:integration
npm run test:assistant
npm run package
```

The deterministic suite uses a fake executable for spaces/Unicode
arguments, JSON/NDJSON fragmentation, malformed output, request IDs, outer and
nested sequences, terminal lifecycle, timeout, interruption, cancellation,
compatibility, and size limits. Three real integration tests use version,
capabilities, doctor, and Java context. The assistant smoke sends real streamed
Ask and Plan calls to local Qwen.

Run the Extension Development Host separately. It checks activation, the Chat
participant ID, public commands, Status/doctor rendering, Runs rendering, exact
finding ranges and deterministic handlers for Help, Status, Context and Ask:

```powershell
npm run test:vscode
```

The packaged extension is `artifacts/opticcode-vscode-0.2.1.vsix`.
GROUNDING-METRICS-001 adds context-scope/evidence settings,
injected-context/evidence actions, `/inspect`, split timing labels, and the
real-process Extension Host Prompt Lab.

## Known limits

- V1 is session-oriented and does not persist findings/runs across VS Code restarts.
- Long non-streaming build commands have timeout protection but no stdin
  cancellation contract yet, so their progress is not advertised as cancellable.
- `compare` preserves the CLI rule that two model generations require explicit
  authorization.
- No fix is automatically applied to the original project; native confirmation
  is mandatory.
- Delete, rename, binary edits and autonomous iteration remain unavailable.

See [`chat-edits.md`](chat-edits.md) for the complete transaction contract.
