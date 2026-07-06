param(
    [string]$ProjectPath = "benchmarks/mini-bukkit-plugin",
    [string]$Model = "qwen2.5-coder:14b",
    [string]$Profile = "minecraft-java-1.8",
    [string]$KeepAlive = "15m",
    [string]$RagIndex = "data/index",
    [int]$RagLimit = 4,
    [int]$MaxTokens = 120,
    [int]$MaxCases = 0,
    [switch]$NoMemory
)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$runsDir = Join-Path $root "benchmarks\runs"
New-Item -ItemType Directory -Force -Path $runsDir | Out-Null

$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$summaryPath = Join-Path $runsDir "rag-quality-$timestamp.md"
$jsonlPath = Join-Path $runsDir "rag-quality-$timestamp.jsonl"
$miniJsonlPath = Join-Path $runsDir "mini-bukkit-runs.jsonl"
$runner = Join-Path $PSScriptRoot "run-mini-benchmark.ps1"
$runTag = "rag-quality-$timestamp"

$cases = @(
    [ordered]@{
        id = "gunpowder-material"
        prompt = "En Bukkit 1.8.8 / Java 8, quel Material exact utiliser pour de la gunpowder ? Reponds court."
        expected = @("Material\.SULPHUR")
    },
    [ordered]@{
        id = "spawner-block"
        prompt = "En Bukkit 1.8.8, quel Material exact utiliser pour un bloc spawner ? Reponds court."
        expected = @("Material\.MOB_SPAWNER|MOB_SPAWNER")
    },
    [ordered]@{
        id = "nether-wart"
        prompt = "En Bukkit 1.8.8, quel nom legacy faut-il verifier pour nether wart ? Reponds court."
        expected = @("Material\.NETHER_STALK|NETHER_STALK|netherStalk")
    },
    [ordered]@{
        id = "shovel-materials"
        prompt = "Pour corriger des pelles modernes dans un plugin Bukkit 1.8.8, cite les noms legacy bois et diamant. Reponds court."
        expected = @("WOOD_SPADE", "DIAMOND_SPADE")
    },
    [ordered]@{
        id = "spawn-egg"
        prompt = "En Minecraft/Bukkit 1.8.8, quels noms legacy verifier pour les spawn eggs ? Reponds court."
        expected = @("MONSTER_EGG|monster_placer|monsterPlacer")
    }
)

if ($MaxCases -gt 0) {
    $cases = $cases | Select-Object -First $MaxCases
}

function Invoke-QualityRun {
    param(
        [object]$Case,
        [bool]$DisableRag
    )

    $params = @{
        Prompt = $Case.prompt
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

    & $runner @params | Out-Null
    $lastLine = Get-Content -Path $miniJsonlPath -Tail 1
    $record = $lastLine | ConvertFrom-Json
    $answer = Get-Content -Raw -Path $record.answer_path

    $matches = New-Object System.Collections.Generic.List[string]
    $missing = New-Object System.Collections.Generic.List[string]

    foreach ($pattern in $Case.expected) {
        if ([regex]::IsMatch($answer, $pattern, [System.Text.RegularExpressions.RegexOptions]::IgnoreCase)) {
            $matches.Add($pattern)
        } else {
            $missing.Add($pattern)
        }
    }

    $expectedCount = @($Case.expected).Count
    $score = if ($expectedCount -eq 0) { 1.0 } else { $matches.Count / $expectedCount }

    return [pscustomobject][ordered]@{
        timestamp = (Get-Date).ToString("o")
        run_tag = $runTag
        case_id = $Case.id
        prompt = $Case.prompt
        mode = if ($DisableRag) { "sans RAG" } else { "avec RAG" }
        no_rag = $DisableRag
        expected = @($Case.expected)
        matched = @($matches)
        missing = @($missing)
        quality_score = $score
        answer_path = $record.answer_path
        metrics_path = $record.metrics_path
        metrics = $record.metrics
        answer = $answer.Trim()
    }
}

$records = New-Object System.Collections.Generic.List[object]

foreach ($case in $cases) {
    $withRag = Invoke-QualityRun -Case $case -DisableRag $false
    $withoutRag = Invoke-QualityRun -Case $case -DisableRag $true
    $records.Add($withRag)
    $records.Add($withoutRag)
    ($withRag | ConvertTo-Json -Depth 8 -Compress) | Add-Content -Path $jsonlPath
    ($withoutRag | ConvertTo-Json -Depth 8 -Compress) | Add-Content -Path $jsonlPath
}

$summary = New-Object System.Collections.Generic.List[string]
$summary.Add("# OpticCode - Qualite RAG legacy")
$summary.Add("")
$summary.Add("Date : $(Get-Date -Format o)")
$summary.Add("")
$summary.Add("| Cas | Mode | Score | Prompt chars | Client s | Eval tok/s | Manquants |")
$summary.Add("| --- | --- | ---: | ---: | ---: | ---: | --- |")

foreach ($record in $records) {
    $promptChars = $record.metrics.prompt_chars
    $clientSeconds = "{0:N2}" -f $record.metrics.client_seconds
    $tokPerSecond = if ($null -ne $record.metrics.eval_tokens_per_second) { "{0:N2}" -f $record.metrics.eval_tokens_per_second } else { "n/a" }
    $score = "{0:P0}" -f $record.quality_score
    $missingText = if ($record.missing.Count -eq 0) { "-" } else { ($record.missing -join "<br>") }
    $summary.Add("| $($record.case_id) | $($record.mode) | $score | $promptChars | $clientSeconds | $tokPerSecond | $missingText |")
}

$withRagRecords = @($records | Where-Object { -not $_.no_rag })
$withoutRagRecords = @($records | Where-Object { $_.no_rag })
$withRagAverage = if ($withRagRecords.Count -gt 0) { ($withRagRecords | Measure-Object -Property quality_score -Average).Average } else { 0 }
$withoutRagAverage = if ($withoutRagRecords.Count -gt 0) { ($withoutRagRecords | Measure-Object -Property quality_score -Average).Average } else { 0 }

$summary.Add("")
$summary.Add("## Moyennes")
$summary.Add("")
$withRagAverageText = "{0:P0}" -f $withRagAverage
$withoutRagAverageText = "{0:P0}" -f $withoutRagAverage
$summary.Add("- avec RAG : $withRagAverageText")
$summary.Add("- sans RAG : $withoutRagAverageText")
$summary.Add("")
$summary.Add("## Reponses")

foreach ($record in $records) {
    $summary.Add("")
    $summary.Add("### $($record.case_id) - $($record.mode)")
    $summary.Add("")
    $expectedText = $record.expected -join ", "
    $summary.Add("Attendus : $expectedText")
    $summary.Add("")
    $summary.Add('```text')
    $summary.Add($record.answer)
    $summary.Add('```')
}

$summary.Add("")
$summary.Add("JSONL detail : $jsonlPath")

$summary | Set-Content -Path $summaryPath

Write-Output "Summary: $summaryPath"
Write-Output "JSONL: $jsonlPath"
