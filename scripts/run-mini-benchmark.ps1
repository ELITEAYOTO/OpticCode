param(
    [string]$Prompt = "Verifier ce plugin Bukkit 1.8.8 et proposer les risques avant compilation",
    [string]$ProjectPath = "benchmarks/mini-bukkit-plugin",
    [string]$Model = "qwen2.5-coder:14b",
    [string]$Profile = "minecraft-java-1.8",
    [string]$KeepAlive = "15m",
    [int]$MaxTokens = 160,
    [switch]$Brief = $true,
    [switch]$NoMemory
)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$runsDir = Join-Path $root "benchmarks\runs"
New-Item -ItemType Directory -Force -Path $runsDir | Out-Null

$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$prefix = Join-Path $runsDir "mini-bukkit-$timestamp"
$answerPath = "$prefix.answer.md"
$metricsPath = "$prefix.metrics.txt"
$jsonlPath = Join-Path $runsDir "mini-bukkit-runs.jsonl"

$argsList = @(
    "run", "-q", "--",
    "plan", $Prompt,
    "--path", $ProjectPath,
    "--model", $Model,
    "--profile", $Profile,
    "--keep-alive", $KeepAlive,
    "--max-tokens", "$MaxTokens",
    "--metrics-json"
)

if ($Brief) {
    $argsList += "--brief"
}

if ($NoMemory) {
    $argsList += "--no-memory"
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

$metricsRaw = Get-Content -Raw $metricsPath
$marker = "=== metrics_json ==="
$markerIndex = $metricsRaw.IndexOf($marker)
if ($markerIndex -lt 0) {
    throw "Metrics JSON marker not found in $metricsPath"
}

$metricsJson = $metricsRaw.Substring($markerIndex + $marker.Length).Trim()
$metrics = $metricsJson | ConvertFrom-Json

$record = [ordered]@{
    timestamp = (Get-Date).ToString("o")
    prompt = $Prompt
    project_path = $ProjectPath
    model = $Model
    profile = $Profile
    keep_alive = $KeepAlive
    brief = [bool]$Brief
    no_memory = [bool]$NoMemory
    max_tokens = $MaxTokens
    answer_path = $answerPath
    metrics_path = $metricsPath
    metrics = $metrics
}

($record | ConvertTo-Json -Depth 8 -Compress) | Add-Content -Path $jsonlPath

Write-Output "Answer: $answerPath"
Write-Output "Metrics: $metricsPath"
Write-Output "JSONL: $jsonlPath"
