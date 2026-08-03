param(
    [string]$Suite = "benchmarks/eval/context-retrieval-v1.json",
    [string]$ReportsDir = "benchmarks/runs/eval",
    [switch]$IncludeRag,
    [string]$RagIndex = "data/index"
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $repoRoot

$statusBefore = git status --porcelain=v1
if ($LASTEXITCODE -ne 0) {
    throw "Unable to capture Git status before evaluation."
}

$suiteDocument = Get-Content -LiteralPath $Suite -Raw | ConvertFrom-Json
$caseCount = @($suiteDocument.cases).Count
if ($caseCount -lt 40 -or $caseCount -gt 60) {
    throw "The versioned evaluation suite must contain between 40 and 60 cases; found $caseCount."
}
$categoryCount = @($suiteDocument.cases.category | Sort-Object -Unique).Count
if ($categoryCount -lt 5) {
    throw "The versioned evaluation suite must cover at least five categories."
}

cargo test -p opticcode-tools eval::
if ($LASTEXITCODE -ne 0) { throw "Evaluation unit tests failed." }

cargo test -p opticcode-cli --test eval_cli
if ($LASTEXITCODE -ne 0) { throw "Evaluation CLI tests failed." }

cargo build --workspace --release
if ($LASTEXITCODE -ne 0) { throw "Release build failed." }

$strategies = "legacy,symbol,exact"
$arguments = @(
    "eval",
    "--suite", $Suite,
    "--strategy", $strategies,
    "--reports-dir", $ReportsDir,
    "--no-rag"
)
if ($IncludeRag) {
    $arguments = @(
        "eval",
        "--suite", $Suite,
        "--strategy", "legacy,symbol,exact,rag",
        "--reports-dir", $ReportsDir,
        "--rag-index", $RagIndex
    )
}

& .\target\release\opticcode.exe @arguments
if ($LASTEXITCODE -ne 0) { throw "Full deterministic evaluation suite failed." }

$statusAfter = git status --porcelain=v1
if ($LASTEXITCODE -ne 0) {
    throw "Unable to capture Git status after evaluation."
}
if (($statusBefore -join "`n") -ne ($statusAfter -join "`n")) {
    throw "Evaluation changed the tracked or untracked Git state."
}

Write-Host "EVAL-001 quality gate passed: $caseCount cases, $categoryCount categories."
