param(
    [string]$Prompt = "Verifier ce plugin Bukkit 1.8.8 et proposer les risques avant compilation",
    [string]$ProjectPath = "benchmarks/mini-bukkit-plugin",
    [string]$Model = "qwen2.5-coder:14b",
    [switch]$Brief = $true
)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$runsDir = Join-Path $root "benchmarks\runs"
New-Item -ItemType Directory -Force -Path $runsDir | Out-Null

$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$prefix = Join-Path $runsDir "mini-bukkit-$timestamp"
$answerPath = "$prefix.answer.md"
$metricsPath = "$prefix.metrics.txt"

$argsList = @(
    "run", "-q", "--",
    "plan", $Prompt,
    "--path", $ProjectPath,
    "--model", $Model,
    "--metrics-json"
)

if ($Brief) {
    $argsList += "--brief"
}

Push-Location $root
try {
    $previousErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & cargo @argsList 1> $answerPath 2> $metricsPath
    $ErrorActionPreference = $previousErrorActionPreference
    if ($LASTEXITCODE -ne 0) {
        throw "Benchmark command failed with exit code $LASTEXITCODE"
    }
} finally {
    if ($null -ne $previousErrorActionPreference) {
        $ErrorActionPreference = $previousErrorActionPreference
    }
    Pop-Location
}

Write-Output "Answer: $answerPath"
Write-Output "Metrics: $metricsPath"
