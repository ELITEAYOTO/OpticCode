param(
    [int]$Iterations = 5,
    [string]$KspawnersCopyPath = "benchmarks/runs/real-plugin-kspawners-20260707-015407/Kspawners-copy",
    [string]$PandaSpigotPath = "C:\Users\timot\Desktop\KhopeSpigot\PandaSpigot-Fork\PandaSpigot"
)

$ErrorActionPreference = "Stop"

if ($Iterations -lt 1) {
    throw "Iterations must be at least 1."
}

$root = Split-Path -Parent $PSScriptRoot
$runsDir = Join-Path $root "benchmarks\runs"
$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$runDir = Join-Path $runsDir "git-snapshot-benchmark-$timestamp"
$smallPath = Join-Path $runDir "small-fixture"
$resultsPath = Join-Path $runDir "results.json"
$summaryPath = Join-Path $runDir "summary.md"

New-Item -ItemType Directory -Force -Path $smallPath | Out-Null

$resolvedRunDir = [System.IO.Path]::GetFullPath($runDir)
$resolvedSmallPath = [System.IO.Path]::GetFullPath($smallPath)
if (-not $resolvedSmallPath.StartsWith($resolvedRunDir, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to create the small fixture outside the benchmark run directory."
}

"clean`n" | Set-Content -Path (Join-Path $smallPath "README.md") -Encoding Ascii
Push-Location $smallPath
try {
    & git -c core.autocrlf=false init --quiet
    if ($LASTEXITCODE -ne 0) { throw "git init failed" }
    & git -c core.autocrlf=false add --all
    if ($LASTEXITCODE -ne 0) { throw "git add failed" }
    & git `
        -c core.autocrlf=false `
        -c commit.gpgsign=false `
        -c user.name="OpticCode Test" `
        -c user.email="opticcode-test@example.invalid" `
        commit --quiet --no-verify -m "snapshot benchmark fixture"
    if ($LASTEXITCODE -ne 0) { throw "git commit failed" }
} finally {
    Pop-Location
}
"dirty`n" | Set-Content -Path (Join-Path $smallPath "README.md") -Encoding Ascii
"untracked`n" | Set-Content -Path (Join-Path $smallPath "notes with spaces.txt") -Encoding Ascii

Push-Location $root
try {
    & cargo build -q -p opticcode-cli
    if ($LASTEXITCODE -ne 0) { throw "OpticCode debug build failed" }
} finally {
    Pop-Location
}

$opticcode = Join-Path $root "target\debug\opticcode.exe"
$sources = @(
    [pscustomobject]@{ Name = "small"; Path = $smallPath },
    [pscustomobject]@{ Name = "kspawners"; Path = $KspawnersCopyPath },
    [pscustomobject]@{ Name = "pandaspigot"; Path = $PandaSpigotPath }
)
$records = New-Object System.Collections.Generic.List[object]

foreach ($source in $sources) {
    $sourcePath = $source.Path
    if (-not [System.IO.Path]::IsPathRooted($sourcePath)) {
        $sourcePath = Join-Path $root $sourcePath
    }
    if (-not (Test-Path -LiteralPath $sourcePath)) {
        Write-Warning "Skipping missing source: $sourcePath"
        continue
    }

    for ($iteration = 1; $iteration -le $Iterations; $iteration++) {
        $jsonPath = Join-Path $runDir "$($source.Name)-$iteration.json"
        & $opticcode git-state --path $sourcePath --json 1> $jsonPath
        if ($LASTEXITCODE -ne 0) {
            throw "git-state failed for $($source.Name), iteration $iteration"
        }
        $snapshot = Get-Content -Raw -Path $jsonPath | ConvertFrom-Json
        $records.Add([pscustomobject][ordered]@{
            source = $source.Name
            path = $sourcePath
            iteration = $iteration
            duration_us = [long]$snapshot.metrics.duration_us
            status_entries = [int]$snapshot.metrics.status_entries
            fingerprinted_files = [int]$snapshot.metrics.fingerprinted_files
            fingerprinted_bytes = [long]$snapshot.metrics.fingerprinted_bytes
        })
    }
}

$records | ConvertTo-Json -Depth 5 | Set-Content -Path $resultsPath -Encoding UTF8

$summary = New-Object System.Collections.Generic.List[string]
$summary.Add("# OpticCode - Git snapshot benchmark")
$summary.Add("")
$summary.Add("Date : $(Get-Date -Format o)")
$summary.Add("")
$summary.Add("Iterations par source : $Iterations")
$summary.Add("")
$summary.Add("| Source | Avg ms | Min ms | Max ms | Status entries | Fingerprinted files | Fingerprinted bytes |")
$summary.Add("| --- | ---: | ---: | ---: | ---: | ---: | ---: |")

foreach ($source in @($records.source | Sort-Object -Unique)) {
    $sourceRecords = @($records | Where-Object { $_.source -eq $source })
    $durations = @($sourceRecords | ForEach-Object { $_.duration_us / 1000.0 })
    $average = ($durations | Measure-Object -Average).Average
    $minimum = ($durations | Measure-Object -Minimum).Minimum
    $maximum = ($durations | Measure-Object -Maximum).Maximum
    $latest = $sourceRecords[-1]
    $summary.Add((
        "| {0} | {1:N3} | {2:N3} | {3:N3} | {4} | {5} | {6} |" -f `
            $source,
            $average,
            $minimum,
            $maximum,
            $latest.status_entries,
            $latest.fingerprinted_files,
            $latest.fingerprinted_bytes
    ))
}

$summary.Add("")
$summary.Add("Results JSON: $resultsPath")
$summary | Set-Content -Path $summaryPath -Encoding UTF8

Write-Output "Summary: $summaryPath"
Write-Output "Results: $resultsPath"
exit 0
