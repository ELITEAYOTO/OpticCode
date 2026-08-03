[CmdletBinding()]
param(
    [ValidateRange(1, 50)]
    [int]$SearchIterations = 5
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent $PSScriptRoot
$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$runDir = Join-Path $projectRoot "benchmarks/runs/rag-safe-$timestamp"
$sourceDir = Join-Path $runDir "source"
$indexDir = Join-Path $runDir "index"
$repeatIndexDir = Join-Path $runDir "index-repeat"
$legacyIndexDir = Join-Path $runDir "legacy-index"
$secretSentinel = "RAG_SAFE_INVALID_SECRET_SENTINEL_12345"

function Invoke-CargoChecked {
    param([Parameter(Mandatory = $true)][string[]]$Arguments)

    & cargo @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "cargo $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
    }
}

function Invoke-OpticCodeJson {
    param([Parameter(Mandatory = $true)][string[]]$Arguments)

    $raw = & $script:binary @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "opticcode $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
    }
    $text = $raw -join "`n"
    if ($text.Contains($script:secretSentinel)) {
        throw "a rejected secret value was exposed by opticcode output"
    }
    return $text | ConvertFrom-Json
}

function Get-ActiveGenerationDirectory {
    param([Parameter(Mandatory = $true)][string]$Index)

    $generation = (Get-Content -LiteralPath (Join-Path $Index "CURRENT") -Raw).Trim()
    return Join-Path (Join-Path $Index "generations") $generation
}

Push-Location $projectRoot
try {
    New-Item -ItemType Directory -Path $sourceDir -Force | Out-Null
    Copy-Item -LiteralPath "benchmarks/rag-safe/src" -Destination $sourceDir -Recurse
    Copy-Item -LiteralPath "benchmarks/rag-safe/docs" -Destination $sourceDir -Recurse
    Copy-Item -LiteralPath "benchmarks/rag-safe/config.yml" -Destination $sourceDir
    [IO.File]::WriteAllText((Join-Path $sourceDir ".env"), "PASSWORD=$secretSentinel`n")
    [IO.File]::WriteAllText(
        (Join-Path $sourceDir "application.properties"),
        "password=$secretSentinel`n"
    )
    [IO.File]::WriteAllText((Join-Path $sourceDir "README"), "extensionless test`n")

    Invoke-CargoChecked -Arguments @("test", "-p", "opticcode-tools", "rag::")
    Invoke-CargoChecked -Arguments @("test", "-p", "opticcode-cli", "--test", "rag_safe_cli")
    Invoke-CargoChecked -Arguments @("build", "-p", "opticcode-cli", "--release")
    $script:binary = Join-Path $projectRoot "target/release/opticcode.exe"

    $scanWatch = [Diagnostics.Stopwatch]::StartNew()
    $scan = Invoke-OpticCodeJson -Arguments @(
        "rag-scan", "--path", $sourceDir, "--limit", "50", "--json"
    )
    $scanWatch.Stop()
    if ($scan.sources[0].indexable_files -ne 3) {
        throw "expected 3 safe files, found $($scan.sources[0].indexable_files)"
    }
    if ($scan.sources[0].excluded_entries -ne 3) {
        throw "expected 3 excluded entries, found $($scan.sources[0].excluded_entries)"
    }

    $indexWatch = [Diagnostics.Stopwatch]::StartNew()
    $index = Invoke-OpticCodeJson -Arguments @(
        "rag-index", "--path", $sourceDir, "--output", $indexDir,
        "--chunk-chars", "512", "--json"
    )
    $indexWatch.Stop()
    $repeat = Invoke-OpticCodeJson -Arguments @(
        "rag-index", "--path", $sourceDir, "--output", $repeatIndexDir,
        "--chunk-chars", "512", "--json"
    )
    if ($index.schema_version -ne 2 -or $index.documents -ne 3 -or $index.chunks -ne 3) {
        throw "unexpected RAG-SAFE index report"
    }

    $active = Get-ActiveGenerationDirectory -Index $indexDir
    $repeatActive = Get-ActiveGenerationDirectory -Index $repeatIndexDir
    foreach ($name in @("documents.jsonl", "chunks.jsonl")) {
        $firstHash = (Get-FileHash -LiteralPath (Join-Path $active $name) -Algorithm SHA256).Hash
        $secondHash = (Get-FileHash -LiteralPath (Join-Path $repeatActive $name) -Algorithm SHA256).Hash
        if ($firstHash -ne $secondHash) {
            throw "$name is not deterministic across identical builds"
        }
    }
    $allIndexText = Get-ChildItem -LiteralPath $indexDir -File -Recurse |
        ForEach-Object { Get-Content -LiteralPath $_.FullName -Raw } |
        Out-String
    if ($allIndexText.Contains($secretSentinel)) {
        throw "a rejected secret value was written into the index"
    }

    $searchDurations = @()
    $lastSearch = $null
    for ($iteration = 0; $iteration -lt $SearchIterations; $iteration++) {
        $watch = [Diagnostics.Stopwatch]::StartNew()
        $lastSearch = Invoke-OpticCodeJson -Arguments @(
            "rag-search", "MOB_SPAWNER", "--index", $indexDir, "--limit", "5", "--json"
        )
        $watch.Stop()
        $searchDurations += $watch.Elapsed.TotalMilliseconds
    }
    if (@($lastSearch.hits).Count -lt 1) {
        throw "RAG-SAFE search returned no fixture hit"
    }
    $debugWatch = [Diagnostics.Stopwatch]::StartNew()
    $debug = Invoke-OpticCodeJson -Arguments @(
        "rag-debug", "legacy spawner", "--index", $indexDir, "--limit", "3", "--json"
    )
    $debugWatch.Stop()
    if (@($debug.context.hits).Count -lt 1) {
        throw "RAG-SAFE debug returned no fixture hit"
    }

    New-Item -ItemType Directory -Path $legacyIndexDir -Force | Out-Null
    [IO.File]::WriteAllText((Join-Path $legacyIndexDir "documents.jsonl"), "{}`n")
    [IO.File]::WriteAllText((Join-Path $legacyIndexDir "chunks.jsonl"), "{}`n")
    $legacyStdout = Join-Path $runDir "legacy-stdout.txt"
    $legacyStderr = Join-Path $runDir "legacy-stderr.txt"
    $previousErrorAction = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & $binary rag-search test --index $legacyIndexDir --json 1> $legacyStdout 2> $legacyStderr
    $legacyExit = $LASTEXITCODE
    $ErrorActionPreference = $previousErrorAction
    if ($legacyExit -eq 0 -or -not (Select-String -LiteralPath $legacyStderr -Pattern "legacy RAG index" -Quiet)) {
        throw "legacy index was not rejected with an explicit diagnostic"
    }

    $searchStats = $searchDurations | Measure-Object -Minimum -Maximum -Average
    $sourceReport = $scan.sources[0]
    $secretOverheadPercent = if ($sourceReport.scan_us -gt 0) {
        100.0 * [double]$sourceReport.secret_scan_us / [double]$sourceReport.scan_us
    } else {
        0.0
    }
    $summary = [ordered]@{
        schema_version = 1
        fixture_files = 6
        safe_documents = [int]$index.documents
        chunks = [int]$index.chunks
        excluded_entries = [int]$index.excluded_entries
        scan_ms = [Math]::Round($scanWatch.Elapsed.TotalMilliseconds, 3)
        index_ms = [Math]::Round($indexWatch.Elapsed.TotalMilliseconds, 3)
        internal_scan_us = [int64]$sourceReport.scan_us
        secret_scan_us = [int64]$sourceReport.secret_scan_us
        secret_scan_percent = [Math]::Round($secretOverheadPercent, 3)
        search_iterations = $SearchIterations
        search_min_ms = [Math]::Round($searchStats.Minimum, 3)
        search_max_ms = [Math]::Round($searchStats.Maximum, 3)
        search_average_ms = [Math]::Round($searchStats.Average, 3)
        debug_ms = [Math]::Round($debugWatch.Elapsed.TotalMilliseconds, 3)
        debug_hits = @($debug.context.hits).Count
        deterministic_data_files = $true
        secret_value_absent = $true
        legacy_index_rejected = $true
        generation_id = [string]$index.generation_id
    }
    $summary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $runDir "summary.json") -Encoding UTF8
    @(
        "# RAG-SAFE-001 quality"
        ""
        "- safe documents: $($summary.safe_documents)"
        "- excluded entries: $($summary.excluded_entries)"
        "- scan: $($summary.scan_ms) ms"
        "- index: $($summary.index_ms) ms"
        "- secret scan: $($summary.secret_scan_us) us ($($summary.secret_scan_percent)%)"
        "- search average: $($summary.search_average_ms) ms"
        "- batched debug: $($summary.debug_ms) ms ($($summary.debug_hits) hits)"
        "- deterministic data files: yes"
        "- rejected secret absent from outputs: yes"
        "- legacy index rejected: yes"
    ) | Set-Content -LiteralPath (Join-Path $runDir "summary.md") -Encoding UTF8

    $summary | ConvertTo-Json -Depth 8
    Write-Host "Report: $runDir"
}
finally {
    Pop-Location
}
