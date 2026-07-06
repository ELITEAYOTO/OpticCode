param(
    [string[]]$Prompts = @(
        "Verifier nether wart et spawner dans un plugin Bukkit 1.8.8",
        "Quels risques legacy verifier pour des pelles, spawners et spawn eggs en Bukkit 1.8.8 ?",
        "Verifier rapidement les risques Java 8 et Bukkit legacy du mini plugin",
        "Quels fichiers inspecter avant de corriger des materials modernes dans un plugin Bukkit 1.8.8 ?"
    ),
    [string]$ProjectPath = "benchmarks/mini-bukkit-plugin",
    [string]$Model = "qwen2.5-coder:14b",
    [string]$Profile = "minecraft-java-1.8",
    [string]$KeepAlive = "15m",
    [string]$RagIndex = "data/index",
    [int]$RagLimit = 4,
    [int]$MaxTokens = 80,
    [switch]$NoMemory,
    [switch]$RagDebug
)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$runsDir = Join-Path $root "benchmarks\runs"
New-Item -ItemType Directory -Force -Path $runsDir | Out-Null

$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$summaryPath = Join-Path $runsDir "rag-comparison-$timestamp.md"
$jsonlPath = Join-Path $runsDir "rag-comparison-$timestamp.jsonl"
$miniJsonlPath = Join-Path $runsDir "mini-bukkit-runs.jsonl"
$runner = Join-Path $PSScriptRoot "run-mini-benchmark.ps1"
$runTag = "rag-comparison-$timestamp"
$records = @()

function Invoke-OneBenchmark {
    param(
        [string]$Prompt,
        [bool]$DisableRag
    )

    $params = @{
        Prompt = $Prompt
        ProjectPath = $ProjectPath
        Model = $Model
        Profile = $Profile
        KeepAlive = $KeepAlive
        RunTag = $runTag
        RagIndex = $RagIndex
        RagLimit = $RagLimit
        MaxTokens = $MaxTokens
    }

    if ($NoMemory) {
        $params.NoMemory = $true
    }

    if ($DisableRag) {
        $params.NoRag = $true
    }

    if ($RagDebug) {
        $params.RagDebug = $true
    }

    & $runner @params | Out-Null
    $lastLine = Get-Content -Path $miniJsonlPath -Tail 1
    return $lastLine | ConvertFrom-Json
}

foreach ($prompt in $Prompts) {
    $withRag = Invoke-OneBenchmark -Prompt $prompt -DisableRag $false
    $withoutRag = Invoke-OneBenchmark -Prompt $prompt -DisableRag $true
    $records += $withRag
    $records += $withoutRag
    ($withRag | ConvertTo-Json -Depth 8 -Compress) | Add-Content -Path $jsonlPath
    ($withoutRag | ConvertTo-Json -Depth 8 -Compress) | Add-Content -Path $jsonlPath
}

$summary = New-Object System.Collections.Generic.List[string]
$summary.Add("# OpticCode - Comparaison RAG")
$summary.Add("")
$summary.Add("Date : $(Get-Date -Format o)")
$summary.Add("")
$summary.Add("| Prompt | Mode | Prompt chars | Client s | Load s | Eval tokens | Eval tok/s |")
$summary.Add("| --- | --- | ---: | ---: | ---: | ---: | ---: |")

foreach ($record in $records) {
    $mode = if ($record.no_rag) { "sans RAG" } else { "avec RAG" }
    $promptText = $record.prompt.Replace("|", "\|")
    $promptChars = $record.metrics.prompt_chars
    $clientSeconds = "{0:N2}" -f $record.metrics.client_seconds
    $loadSeconds = if ($null -ne $record.metrics.ollama_load_seconds) { "{0:N2}" -f $record.metrics.ollama_load_seconds } else { "n/a" }
    $evalCount = if ($null -ne $record.metrics.eval_count) { $record.metrics.eval_count } else { "n/a" }
    $tokPerSecond = if ($null -ne $record.metrics.eval_tokens_per_second) { "{0:N2}" -f $record.metrics.eval_tokens_per_second } else { "n/a" }
    $summary.Add("| $promptText | $mode | $promptChars | $clientSeconds | $loadSeconds | $evalCount | $tokPerSecond |")
}

$summary.Add("")
$summary.Add("JSONL detail : $jsonlPath")
$summary.Add("")
$summary.Add('Les reponses completes restent dans les fichiers `benchmarks/runs/mini-bukkit-*.answer.md`.')

$summary | Set-Content -Path $summaryPath

Write-Output "Summary: $summaryPath"
Write-Output "JSONL: $jsonlPath"
