# Client discovery protocol

`LLM/PROTOCOL-001` exposes three read-only commands for thin clients such as the
experimental VS Code extension. Their stdout is reserved for one JSON document
when `--json` is used. Diagnostics from failed checks are represented in the
document instead of being mixed into stdout.

## Commands

```powershell
target\release\opticcode.exe version --json
target\release\opticcode.exe capabilities --json
target\release\opticcode.exe doctor --json --path C:\path\to\project
```

All three documents use protocol `opticcode.discovery` and schema version `1`.
A client must reject a different protocol identifier or an unsupported schema
version before using any payload fields.

`version` reports the OpticCode version, assistant, Chat, LLM and Policy protocol versions,
the schemas consumed by clients, the target OS/architecture, the build kind,
and an optional build commit. Set `OPTICCODE_GIT_COMMIT` while compiling when a
distribution needs the commit embedded in the binary.

`capabilities` is static discovery. It lists commands, providers, context modes,
machine output formats, streaming/cancellation support, and major feature
families. It does not contact Ollama.

POLICY-001 adds the compatible `policy_runtime` block: schema/version, the
`read_only`, `worktree_edit`, and `approved_apply` modes, engine/audit/approval/
CLI availability, plus `chat_read_only: true` and `chat_write: false`.

`doctor` performs bounded, read-only checks for the executable, Git, Java,
Maven, Gradle, the Ollama CLI/provider, the configured model, RAG v2, the active
profile, workspace Git state, Git worktrees, OpticCode leases, and PolicyEngine
state. It never
installs software, downloads a model, starts Ollama, builds a project, creates a
worktree, or repairs a lease.

## Doctor semantics

Each check has a stable `id`, `status`, `required`, and `summary`. Optional
`version` and `path` fields add display information. Status values are:

- `ok`: the check completed successfully;
- `warning`: an optional feature needs attention;
- `unavailable`: an executable was not found;
- `error`: a check failed or a required service is unusable.

The report-level `success` is true only when every required check is `ok`.
Maven, Gradle, the RAG index, and abandoned leases are reported independently so
a client can render partial readiness without inventing a single generic error.

Useful overrides:

```powershell
target\release\opticcode.exe doctor --json `
  --path C:\path\to\plugin `
  --profile minecraft-java-1.8 `
  --model qwen2.5-coder:14b `
  --ollama-url http://localhost:11434 `
  --rag-index C:\path\to\OpticCode\data\index `
  --timeout-ms 5000
```

The default context mode remains `legacy`; discovery does not change assistant
behavior or select a mode on behalf of a client.

## Assistant stream completion and cancellation

The additive schema-v1 `completed.summary` field contains bounded IDE metadata:
the requested and used context modes, warnings, context file paths, estimated and
actual token counts, timings, and generation speed. Older schema-v1 producers
may omit this optional field; clients must degrade explicitly instead of
inventing metrics.

For `ask --protocol-jsonl` and `plan --protocol-jsonl`, a child-process client can
write exactly `cancel\n` to stdin. OpticCode forwards that request to the
provider cancellation token and emits a terminal `cancelled` event when the
provider confirms it. Unknown or oversized stdin commands do nothing. Killing
the process remains an unclean interruption and must not be reported as a
confirmed cancellation.
