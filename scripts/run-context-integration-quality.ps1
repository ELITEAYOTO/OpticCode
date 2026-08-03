param(
    [switch]$WithLlm,
    [string]$Model = "qwen2.5-coder:14b",
    [int]$MaxGeneratedTokens = 128,
    [int]$HttpTimeoutMs = 300000
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $repoRoot

$statusBefore = git status --porcelain=v1
if ($LASTEXITCODE -ne 0) { throw "Unable to capture Git status before CONTEXT-002 gate." }

cargo test -p opticcode-core assistant_runtime::
if ($LASTEXITCODE -ne 0) { throw "Assistant runtime tests failed." }
cargo test -p opticcode-core context_runtime::
if ($LASTEXITCODE -ne 0) { throw "Context runtime tests failed." }
cargo test -p opticcode-llm
if ($LASTEXITCODE -ne 0) { throw "Ollama client tests failed." }
cargo test -p opticcode-cli --test assistant_cli
if ($LASTEXITCODE -ne 0) { throw "Assistant CLI tests failed." }
cargo build --workspace --release
if ($LASTEXITCODE -ne 0) { throw "Release build failed." }

$comparisonRaw = & .\target\release\opticcode.exe ask `
    "Locate dev.opticcode.util.Helpers#ping()." `
    --path benchmarks/java-index-mini `
    --profile none `
    --no-memory `
    --no-rag `
    --context-mode compare `
    --json
if ($LASTEXITCODE -ne 0) { throw "Context-only compare command failed." }
$comparison = ($comparisonRaw -join "`n") | ConvertFrom-Json
if (-not $comparison.success -or @($comparison.runs).Count -ne 2) {
    throw "Context-only compare did not return two successful variants."
}
if (@($comparison.runs | Where-Object { $_.generated }).Count -ne 0) {
    throw "Context-only compare contacted the model without explicit authorization."
}

$planComparisonRaw = & .\target\release\opticcode.exe plan `
    "Locate dev.opticcode.util.Helpers#ping()." `
    --path benchmarks/java-index-mini `
    --profile none `
    --no-memory `
    --no-rag `
    --context-mode compare `
    --json
if ($LASTEXITCODE -ne 0) { throw "Plan context-only compare command failed." }
$planComparison = ($planComparisonRaw -join "`n") | ConvertFrom-Json
if (-not $planComparison.success -or @($planComparison.runs).Count -ne 2) {
    throw "Plan context-only compare did not return two successful variants."
}
if (@($planComparison.runs | Where-Object { $_.generated }).Count -ne 0) {
    throw "Plan context-only compare contacted the model without explicit authorization."
}

$invalidRaw = & .\target\release\opticcode.exe plan `
    "Find plugin.yml." `
    --path benchmarks/java-index-mini `
    --no-rag `
    --ollama-url https://example.com `
    --json
$invalidExit = $LASTEXITCODE
$invalid = ($invalidRaw -join "`n") | ConvertFrom-Json
if ($invalidExit -ne 2 -or $invalid.success -ne $false) {
    throw "Non-local Ollama URL was not rejected with the structured exit code."
}

if ($WithLlm) {
    & .\target\release\opticcode.exe eval `
        --strategy legacy,symbol `
        --case impact-ping-static-import,config-java-index-permission,legacy-material-gunpowder `
        --with-llm `
        --no-rag `
        --model $Model `
        --temperature 0 `
        --seed 42 `
        --max-generated-tokens $MaxGeneratedTokens `
        --http-timeout-ms $HttpTimeoutMs `
        --keep-alive 15m `
        --warmup-runs 1
    if ($LASTEXITCODE -ne 0) { throw "Real Qwen A/B evaluation failed." }
}

$statusAfter = git status --porcelain=v1
if ($LASTEXITCODE -ne 0) { throw "Unable to capture Git status after CONTEXT-002 gate." }
if (($statusBefore -join "`n") -ne ($statusAfter -join "`n")) {
    throw "CONTEXT-002 quality gate changed the repository state."
}

Write-Host "CONTEXT-002 quality gate passed."
