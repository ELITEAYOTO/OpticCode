Client discovery protocol

LLM/PROTOCOL-001 exposes three read-only commands for thin clients such as theexperimental VS Code extension. Their stdout is reserved for one JSON documentwhen --json is used. Diagnostics from failed checks are represented in thedocument instead of being mixed into stdout.

Commands

target\release\opticcode.exe version --json
target\release\opticcode.exe capabilities --json
target\release\opticcode.exe doctor --json --path C:\path\to\project

All three documents use protocol opticcode.discovery and schema version 1.A client must reject a different protocol identifier or an unsupported schemaversion before using any payload fields.

Version and build provenance

version reports the OpticCode version, assistant, Chat, LLM and Policyprotocol versions, schemas consumed by clients, platform information and buildprovenance.

The additive schema-v1 platform/build fields are:

platform.os: Rust target operating system;

platform.architecture: Rust target architecture;

platform.target: Cargo target triple;

build.kind: debug or release according to Rust debug assertions;

build.profile: Cargo build profile;

build.commit: complete Git object identifier when available;

build.commit_short: first eight hexadecimal characters of build.commit;

build.dirty: whether tracked or untracked workspace changes existed whenthe crate was compiled.

The CLI build script discovers Git metadata without invoking a shell. MissingGit, a source archive without repository metadata, or an invalid override neverblocks compilation: commit and dirty fields can be absent. Clients supportingschema version 1 must therefore accept historical reports that omit theadditive fields, while validating them when present.

A release system can override repository discovery with:

$env:OPTICCODE_GIT_COMMIT = "5344d320c5f1dc6a8669040ce1c8b65c7192dd15"
$env:OPTICCODE_GIT_DIRTY = "false"
cargo build --release -p opticcode-cli

OPTICCODE_GIT_COMMIT must contain 40 to 64 hexadecimal characters.OPTICCODE_GIT_DIRTY accepts true, false, 1, 0, yes, or no.No build timestamp is embedded, preserving reproducible build behavior.

Without --json, version renders the target, profile, short commit andclean, dirty, or unknown build state for diagnostics.

Capabilities

capabilities is static discovery. It lists commands, providers, context modes,machine output formats, streaming/cancellation support, and major featurefamilies. It does not contact Ollama.

POLICY-001 adds the compatible policy_runtime block: schema/version, theread_only, worktree_edit, and approved_apply modes, engine/audit/approval/CLI availability, plus chat_read_only: true and chat_write: true. Clientsstill request read_only; Rust alone performs scoped internal mode transitions.

Doctor

doctor performs bounded, read-only checks for the executable, Git, Java,Maven, Gradle, the Ollama CLI/provider, the configured model, RAG v2, the activeprofile, workspace Git state, Git worktrees, OpticCode leases, and PolicyEnginestate. It never installs software, downloads a model, starts Ollama, builds aproject, creates a worktree, or repairs a lease.

Doctor semantics

Each check has a stable id, status, required, and summary. Optionalversion and path fields add display information. Status values are:

ok: the check completed successfully;

warning: an optional feature needs attention;

unavailable: an executable was not found;

error: a check failed or a required service is unusable.

The report-level success is true only when every required check is ok.Maven, Gradle, the RAG index, and abandoned leases are reported independently soa client can render partial readiness without inventing a single generic error.

Useful overrides:

target\release\opticcode.exe doctor --json `
  --path C:\path\to\plugin `
  --profile minecraft-java-1.8 `
  --model qwen2.5-coder:14b `
  --ollama-url http://localhost:11434 `
  --rag-index C:\path\to\OpticCode\data\index `
  --timeout-ms 5000

The default context mode remains legacy; discovery does not change assistantbehavior or select a mode on behalf of a client.

Assistant stream completion and cancellation

The additive schema-v1 completed.summary field contains bounded IDE metadata:the requested and used context modes, warnings, context file paths, estimated andactual token counts, timings, and generation speed. Older schema-v1 producersmay omit this optional field; clients must degrade explicitly instead ofinventing metrics.

For ask --protocol-jsonl and plan --protocol-jsonl, a child-process client canwrite exactly cancel\n to stdin. OpticCode forwards that request to theprovider cancellation token and emits a terminal cancelled event when theprovider confirms it. Unknown or oversized stdin commands do nothing. Killingthe process remains an unclean interruption and must not be reported as aconfirmed cancellation.