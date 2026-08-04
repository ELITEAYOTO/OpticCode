[CmdletBinding()]
param(
    [switch]$Mock,
    [switch]$WithExtensionHost,
    [switch]$WithQwen,
    [switch]$Holdout,
    [switch]$Full
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$extensionRoot = Join-Path $repoRoot "extensions\vscode-opticcode"
$reportsRoot = Join-Path $repoRoot "benchmarks\runs"
Set-Location -LiteralPath $repoRoot

function Assert-Exit([string]$Message) {
    if ($LASTEXITCODE -ne 0) { throw "$Message (exit $LASTEXITCODE)." }
}

function Invoke-PromptLab([ValidateSet("mock", "holdout", "qwen")][string]$Mode) {
    if ($Mode -eq "qwen") {
        $inventory = @(ollama list 2>&1)
        Assert-Exit "Unable to inspect the local Ollama inventory"
        if (-not ($inventory -match '^qwen2\.5-coder:14b\s')) {
            throw "qwen2.5-coder:14b is not installed; Prompt Lab never downloads models."
        }
    }
    $env:OPTICCODE_PROMPT_LAB_MODE = $Mode
    $env:OPTICCODE_PROMPT_LAB_RESULT = Join-Path $reportsRoot "prompt-lab-$Mode.json"
    try {
        node scripts/run-prompt-lab.mjs
        Assert-Exit "Prompt Lab $Mode failed"
    } finally {
        Remove-Item Env:\OPTICCODE_PROMPT_LAB_MODE -ErrorAction SilentlyContinue
        Remove-Item Env:\OPTICCODE_PROMPT_LAB_RESULT -ErrorAction SilentlyContinue
    }
    $report = Get-Content -LiteralPath (Join-Path $reportsRoot "prompt-lab-$Mode.json") -Raw |
        ConvertFrom-Json
    if (-not $report.integration.registered_participant_tested -or
        -not $report.integration.handler_tested_in_extension_host -or
        -not $report.integration.actual_cli_transport_tested -or
        $report.integration.visible_chat_input_automated) {
        throw "Prompt Lab $Mode integration claims are inconsistent."
    }
    $failures = @($report.runs | Where-Object status -notin @("passed", "refused"))
    if ($failures.Count -ne 0) { throw "Prompt Lab $Mode contains failed runs." }
    Write-Host "Prompt Lab $Mode passed: $(@($report.runs).Count) runs."
}

$statusBefore = git status --porcelain=v1
Assert-Exit "Unable to capture Git status before VS Code Prompt Lab"
$corpus = Get-Content -LiteralPath (Join-Path $repoRoot "benchmarks\eval\grounding-metrics-v1.json") -Raw |
    ConvertFrom-Json
$requiredCases = @(
    "plugin-yml-exact-keys",
    "plugin-yml-api-version-absent",
    "plugin-yml-api-version-present",
    "reference-only-no-java-leak",
    "reference-only-no-history-leak",
    "same-path-new-hash-invalidates-context",
    "no-internal-cargo-command-leak",
    "unsupported-evidence-refused",
    "bukkit-sulphur-source-only",
    "visible-duration-accurate",
    "late-metrics-ignored",
    "parallel-chat-isolation"
)
$actualCases = @($corpus.cases.id)
if (@($requiredCases | Where-Object { $_ -notin $actualCases }).Count -ne 0) {
    throw "The versioned grounding corpus is incomplete."
}
cargo build --workspace --release
Assert-Exit "Release build failed"
Push-Location -LiteralPath $extensionRoot
try {
    npm run compile
    Assert-Exit "TypeScript compilation failed"
    npm run lint
    Assert-Exit "Extension lint failed"
    if (-not ($Mock -or $WithExtensionHost -or $WithQwen -or $Holdout -or $Full)) {
        $Mock = $true
    }
    if ($Mock -or $WithExtensionHost -or $Full) { Invoke-PromptLab "mock" }
    if ($Holdout -or $Full) { Invoke-PromptLab "holdout" }
    if ($WithQwen -or $Full) { Invoke-PromptLab "qwen" }
} finally {
    Pop-Location
}

$statusAfter = git status --porcelain=v1
Assert-Exit "Unable to capture Git status after VS Code Prompt Lab"
if (($statusBefore -join "`n") -ne ($statusAfter -join "`n")) {
    throw "VS Code Prompt Lab changed repository state."
}
Write-Host "VS Code Prompt Lab gate passed."
Write-Host "The stable VS Code API does not automate the visible Chat input field."
