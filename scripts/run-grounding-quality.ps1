[CmdletBinding()]
param(
    [switch]$Full
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $repoRoot

function Assert-Exit([string]$Message) {
    if ($LASTEXITCODE -ne 0) { throw "$Message (exit $LASTEXITCODE)." }
}

$statusBefore = git status --porcelain=v1
Assert-Exit "Unable to capture Git status before GROUNDING-METRICS-001"

git diff --check
Assert-Exit "Git diff check failed"
cargo fmt --all -- --check
Assert-Exit "Rust formatting failed"
cargo clippy --workspace --all-targets --all-features -- -D warnings
Assert-Exit "Workspace Clippy failed"
cargo test -p opticcode-core grounding
Assert-Exit "Grounding contract tests failed"
cargo test -p opticcode-core chat_runtime
Assert-Exit "Grounded chat runtime tests failed"
cargo test -p opticcode-cli --test chat_cli
Assert-Exit "Chat CLI tests failed"
if ($Full) {
    cargo test --workspace
    Assert-Exit "Workspace tests failed"
}
cargo build --workspace --release
Assert-Exit "Release build failed"

$fixture = (Resolve-Path -LiteralPath "benchmarks\grounding-plugin").Path
$request = @{
    schema_version = 1
    protocol = "opticcode.chat"
    request_id = "grounding-quality-inspect"
    workspace_id = "grounding-quality"
    workspace_root = $fixture
    command = "inspect"
    prompt = "Use only the attached file. Return its top-level keys and say whether api-version exists."
    profile = "minecraft-java-1.8"
    provider = "ollama"
    model = "qwen2.5-coder:14b"
    context_mode = "legacy"
    context_scope = "references_only"
    scope_reason = "explicit_setting"
    evidence_mode = "required"
    references = @(@{
        reference_id = "grounding-quality-plugin"
        kind = "file"
        path = "src/main/resources/plugin.yml"
        inclusion_reason = "quality gate fixture"
    })
    history = @()
    budgets = @{
        max_history_turns = 1
        max_history_chars = 1
        max_history_tokens = 1
        max_references = 4
        max_reference_bytes = 65536
        max_prompt_tokens = 4096
        rag_hits = 0
    }
    generation = @{
        max_output_tokens = 256
        temperature = 0.0
        seed = 42
        brief = $true
        compare_generate = $false
    }
    security_mode = "read_only"
    client = @{
        name = "grounding-quality"
        version = "0.2.1"
        vscode_version = "quality-gate"
        session_id = "grounding-quality-session"
        locale = "en"
        recent_run_ids = @()
        previous_repository_state = $null
    }
    expected_protocols = @{ chat = 1; assistant = 1; discovery = 1; llm = 1 }
}
$events = @(($request | ConvertTo-Json -Depth 12 -Compress) |
    & .\target\release\opticcode.exe chat --protocol-jsonl --rag-index missing-grounding-index |
    ForEach-Object { $_ | ConvertFrom-Json })
Assert-Exit "Deterministic grounding CLI smoke failed"
$terminal = @($events | Where-Object type -eq "completed")
if ($terminal.Count -ne 1) { throw "Grounding smoke did not complete exactly once." }
$grounding = $terminal[0].summary.grounding
if ($grounding.route -ne "document_facts" -or
    $grounding.effective_scope -ne "references_only" -or
    $grounding.injected_references -ne 1 -or
    $grounding.discovered_files -ne 0 -or
    $grounding.rag_hits -ne 0 -or
    -not $grounding.evidence.valid -or
    -not $grounding.compliance.compliant) {
    throw "Grounding smoke violated its strict manifest or evidence contract."
}
if (@($events | Where-Object type -eq "provider_started").Count -ne 0 -or
    @($events | Where-Object {
        $_.type -eq "document_inspection_completed" -and $_.model_calls -eq 0
    }).Count -ne 1) {
    throw "DocumentFacts unexpectedly used or failed to account for a model call."
}

$statusAfter = git status --porcelain=v1
Assert-Exit "Unable to capture Git status after GROUNDING-METRICS-001"
if (($statusBefore -join "`n") -ne ($statusAfter -join "`n")) {
    throw "GROUNDING-METRICS-001 quality gate changed repository state."
}

Write-Host "GROUNDING-METRICS-001 grounding gate passed."
Write-Host "Full Rust workspace executed: $Full"
