param(
    [switch]$WithExtensionHost,
    [switch]$Full
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$extensionRoot = Join-Path $repoRoot "extensions\vscode-opticcode"
Set-Location -LiteralPath $repoRoot

$statusBefore = git status --porcelain=v1
if ($LASTEXITCODE -ne 0) { throw "Unable to capture Git status before VSCODE-CHAT-001 gate." }

cargo fmt --all -- --check
if ($LASTEXITCODE -ne 0) { throw "Rust formatting failed." }
cargo clippy --workspace --all-targets --all-features -- -D warnings
if ($LASTEXITCODE -ne 0) { throw "Rust Clippy failed." }
cargo test -p opticcode-core chat
if ($LASTEXITCODE -ne 0) { throw "Chat runtime tests failed." }
cargo test -p opticcode-tools rag::reference
if ($LASTEXITCODE -ne 0) { throw "Safe reference tests failed." }
cargo test -p opticcode-cli --test chat_cli
if ($LASTEXITCODE -ne 0) { throw "Chat CLI tests failed." }
if ($Full) {
    cargo test --workspace
    if ($LASTEXITCODE -ne 0) { throw "Workspace tests failed." }
}
cargo build --workspace --release
if ($LASTEXITCODE -ne 0) { throw "Release build failed." }

$fixture = (Resolve-Path -LiteralPath "benchmarks\java-index-mini").Path
$request = @{
    schema_version = 1
    protocol = "opticcode.chat"
    request_id = "vscode-chat-gate-help"
    workspace_id = "vscode-chat-gate"
    workspace_root = $fixture
    command = "help"
    prompt = ""
    profile = "none"
    provider = "ollama"
    model = "qwen2.5-coder:14b"
    context_mode = "symbol"
    references = @()
    history = @()
    budgets = @{
        max_history_turns = 12
        max_history_chars = 32768
        max_history_tokens = 8192
        max_references = 24
        max_reference_bytes = 1048576
        max_prompt_tokens = 32768
        rag_hits = 0
    }
    generation = @{
        max_output_tokens = 64
        temperature = $null
        seed = $null
        brief = $true
        compare_generate = $false
    }
    security_mode = "read_only"
    client = @{
        name = "vscode-chat-gate"
        version = "0.1.0"
        vscode_version = "1.125.0"
        session_id = "vscode-chat-gate-session"
        locale = "en"
        recent_run_ids = @()
        previous_repository_state = $null
    }
    expected_protocols = @{ chat = 1; assistant = 1; discovery = 1; llm = 1 }
}
$requestLine = $request | ConvertTo-Json -Depth 12 -Compress
$rawEvents = $requestLine | & .\target\release\opticcode.exe chat `
    --protocol-jsonl `
    --rag-index missing-vscode-chat-gate-index `
    --http-timeout-ms 1000
if ($LASTEXITCODE -ne 0) { throw "Read-only chat smoke failed." }
$events = @($rawEvents | ForEach-Object { $_ | ConvertFrom-Json })
if ($events.Count -lt 2) { throw "Chat smoke emitted too few events." }
$terminals = @($events | Where-Object { $_.type -in @("completed", "failed", "cancelled") })
if ($terminals.Count -ne 1 -or $terminals[0].type -ne "completed") {
    throw "Chat smoke did not emit one completed terminal event."
}
for ($index = 0; $index -lt $events.Count; $index++) {
    if ($events[$index].protocol -ne "opticcode.chat" -or
        $events[$index].schema_version -ne 1 -or
        $events[$index].request_id -ne "vscode-chat-gate-help" -or
        $events[$index].sequence -ne $index) {
        throw "Chat smoke emitted an invalid envelope or sequence."
    }
}

Push-Location -LiteralPath $extensionRoot
try {
    npm ci
    if ($LASTEXITCODE -ne 0) { throw "Extension dependency installation failed." }
    npm run compile
    if ($LASTEXITCODE -ne 0) { throw "TypeScript compilation failed." }
    npm run lint
    if ($LASTEXITCODE -ne 0) { throw "Extension lint failed." }
    npm test
    if ($LASTEXITCODE -ne 0) { throw "Extension unit tests failed." }
    npm run test:integration
    if ($LASTEXITCODE -ne 0) { throw "Extension/CLI integration failed." }
    if ($WithExtensionHost -or $Full) {
        npm run test:vscode
        if ($LASTEXITCODE -ne 0) { throw "VS Code Extension Host test failed." }
    }
} finally {
    Pop-Location
}

$leases = & .\target\release\opticcode.exe worktrees --json | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or @($leases.leases).Count -ne 0) {
    throw "VSCODE-CHAT-001 left an OpticCode worktree lease."
}
$statusAfter = git status --porcelain=v1
if ($LASTEXITCODE -ne 0) { throw "Unable to capture Git status after VSCODE-CHAT-001 gate." }
if (($statusBefore -join "`n") -ne ($statusAfter -join "`n")) {
    throw "VSCODE-CHAT-001 quality gate changed repository state."
}

Write-Host "VSCODE-CHAT-001 quality gate passed."
Write-Host "Extension Host executed: $($WithExtensionHost -or $Full)"
Write-Host "Full Rust workspace executed: $Full"
