param(
    [switch]$WithLlm,
    [string]$Model = "qwen2.5-coder:14b",
    [int]$MaxGeneratedTokens = 32,
    [int]$HttpTimeoutMs = 300000
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $repoRoot

$statusBefore = git status --porcelain=v1
if ($LASTEXITCODE -ne 0) { throw "Unable to capture Git status before LLM/PROTOCOL-001 gate." }

cargo test -p opticcode-llm
if ($LASTEXITCODE -ne 0) { throw "Provider protocol tests failed." }
cargo test -p opticcode-core
if ($LASTEXITCODE -ne 0) { throw "Assistant protocol tests failed." }
cargo test -p opticcode-cli --test assistant_cli
if ($LASTEXITCODE -ne 0) { throw "Assistant CLI protocol tests failed." }
cargo build --workspace --release
if ($LASTEXITCODE -ne 0) { throw "Release build failed." }

& .\target\release\opticcode.exe --help | Out-Null
if ($LASTEXITCODE -ne 0) { throw "Global help failed." }
& .\target\release\opticcode.exe ask --help | Out-Null
if ($LASTEXITCODE -ne 0) { throw "Ask help failed." }
& .\target\release\opticcode.exe plan --help | Out-Null
if ($LASTEXITCODE -ne 0) { throw "Plan help failed." }

$compareRaw = & .\target\release\opticcode.exe ask `
    "Locate dev.opticcode.util.Helpers#ping()." `
    --path benchmarks/java-index-mini `
    --profile none `
    --no-memory `
    --no-rag `
    --context-mode compare `
    --ollama-url http://127.0.0.1:9 `
    --protocol-jsonl `
    --request-id gate-compare-1
if ($LASTEXITCODE -ne 0) { throw "Protocol compare command failed." }
$compareEvents = @($compareRaw | ForEach-Object { $_ | ConvertFrom-Json })
if ($compareEvents.Count -ne 3) { throw "Protocol compare did not emit exactly three events." }
for ($index = 0; $index -lt $compareEvents.Count; $index++) {
    if ($compareEvents[$index].protocol -ne "opticcode.assistant" -or
        $compareEvents[$index].schema_version -ne 1 -or
        $compareEvents[$index].sequence -ne $index) {
        throw "Protocol compare emitted an invalid envelope or sequence."
    }
}
if ($compareEvents[0].type -ne "started" -or
    $compareEvents[1].type -ne "context_prepared" -or
    $compareEvents[2].type -ne "completed") {
    throw "Protocol compare emitted an unexpected lifecycle."
}
if (@($compareEvents | Where-Object { $_.type -eq "provider_event" }).Count -ne 0) {
    throw "Protocol compare contacted a provider without explicit generation."
}

$invalidRaw = & .\target\release\opticcode.exe plan `
    "Locate plugin.yml." `
    --path benchmarks/java-index-mini `
    --no-rag `
    --ollama-url https://example.com `
    --protocol-jsonl `
    --request-id gate-invalid-1
$invalidExit = $LASTEXITCODE
$invalidEvents = @($invalidRaw | ForEach-Object { $_ | ConvertFrom-Json })
if ($invalidExit -ne 2 -or
    $invalidEvents.Count -ne 1 -or
    $invalidEvents[0].type -ne "failed" -or
    $invalidEvents[0].sequence -ne 0) {
    throw "Protocol setup failure did not use one terminal JSONL event and exit code 2."
}

if ($WithLlm) {
    $realRaw = & .\target\release\opticcode.exe ask `
        "Reply with only the Java version required by this project." `
        --path benchmarks/java-index-mini `
        --profile none `
        --no-memory `
        --no-rag `
        --model $Model `
        --temperature 0 `
        --seed 42 `
        --max-tokens $MaxGeneratedTokens `
        --http-timeout-ms $HttpTimeoutMs `
        --keep-alive 15m `
        --protocol-jsonl `
        --request-id gate-real-1
    if ($LASTEXITCODE -ne 0) { throw "Real local provider protocol smoke failed." }
    $realEvents = @($realRaw | ForEach-Object { $_ | ConvertFrom-Json })
    $terminalEvents = @($realEvents | Where-Object {
        $_.type -in @("completed", "failed", "cancelled")
    })
    $providerEvents = @($realEvents | Where-Object { $_.type -eq "provider_event" })
    $deltaEvents = @($providerEvents | Where-Object { $_.event.type -eq "delta" })
    if ($terminalEvents.Count -ne 1 -or
        $terminalEvents[0].type -ne "completed" -or
        $providerEvents.Count -lt 2 -or
        $deltaEvents.Count -lt 1) {
        throw "Real local provider stream did not emit a complete protocol lifecycle."
    }
}

$statusAfter = git status --porcelain=v1
if ($LASTEXITCODE -ne 0) { throw "Unable to capture Git status after LLM/PROTOCOL-001 gate." }
if (($statusBefore -join "`n") -ne ($statusAfter -join "`n")) {
    throw "LLM/PROTOCOL-001 quality gate changed the repository state."
}

Write-Host "LLM/PROTOCOL-001 quality gate passed."
