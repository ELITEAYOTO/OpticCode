[CmdletBinding()]
param(
    [switch]$Full,
    [string]$OutputPath = "target/java-context-quality.json"
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent $PSScriptRoot

function Invoke-CargoChecked {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$Arguments
    )

    Write-Host "cargo $($Arguments -join ' ')"
    & cargo @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "cargo command failed with exit code $LASTEXITCODE"
    }
}

function Assert-Equal {
    param(
        [Parameter(Mandatory = $true)][AllowNull()]$Actual,
        [Parameter(Mandatory = $false)][AllowNull()]$Expected,
        [Parameter(Mandatory = $true)][string]$Label
    )

    if ($Actual -ne $Expected) {
        throw "$Label mismatch: expected '$Expected', found '$Actual'"
    }
}

Push-Location $projectRoot
try {
    Invoke-CargoChecked -Arguments @("fmt", "--all", "--", "--check")
    Invoke-CargoChecked -Arguments @(
        "clippy", "-p", "opticcode-tools", "-p", "opticcode-cli",
        "--all-targets", "--all-features", "--", "-D", "warnings"
    )
    Invoke-CargoChecked -Arguments @("test", "-p", "opticcode-tools", "java_context")
    Invoke-CargoChecked -Arguments @(
        "test", "-p", "opticcode-cli", "--test", "java_context_cli"
    )
    Invoke-CargoChecked -Arguments @("build", "-p", "opticcode-cli", "--release")

    $binary = Join-Path $projectRoot "target/release/opticcode.exe"
    $corpus = Join-Path $projectRoot "benchmarks/java-index-mini"
    $tasks = Get-Content -LiteralPath "benchmarks/java-context/tasks.json" -Raw |
        ConvertFrom-Json
    $rows = @()

    foreach ($task in $tasks) {
        $raw = & $binary java-context $task.task --path $corpus --compare-baseline --json
        if ($LASTEXITCODE -ne 0) {
            throw "java-context failed for task '$($task.id)' with exit code $LASTEXITCODE"
        }
        $report = $raw | ConvertFrom-Json

        Assert-Equal -Actual $report.schema_version -Expected 1 -Label "$($task.id) schema"
        Assert-Equal -Actual $report.primary_ambiguous -Expected $task.expected_ambiguous `
            -Label "$($task.id) ambiguity"
        if ($null -ne $task.expected_primary) {
            Assert-Equal -Actual $report.primary_symbol -Expected $task.expected_primary `
                -Label "$($task.id) primary symbol"
        }
        elseif ($null -ne $report.primary_symbol) {
            throw "$($task.id) unexpectedly selected primary symbol '$($report.primary_symbol)'"
        }

        $selectedSymbols = @($report.candidates | ForEach-Object { $_.symbol_id })
        $selectedSymbols += @(
            $report.snippets |
                Where-Object { $null -ne $_.symbol_id } |
                ForEach-Object { $_.symbol_id }
        )
        $selectedSymbols = @($selectedSymbols | Sort-Object -Unique)
        $selectedRoles = @($report.snippets | ForEach-Object { $_.role } | Sort-Object -Unique)
        $selectedPaths = @($report.candidates | ForEach-Object { [string]$_.file })
        $selectedPaths += @($report.snippets | ForEach-Object { [string]$_.file })

        $missingSymbols = @(
            $task.required_symbols |
                Where-Object { $selectedSymbols -notcontains [string]$_ }
        )
        $missingRoles = @(
            $task.required_roles |
                Where-Object { $selectedRoles -notcontains [string]$_ }
        )
        $noiseHits = @()
        foreach ($fragment in $task.forbidden_symbol_fragments) {
            $noiseHits += @($selectedSymbols | Where-Object { $_ -like "*$fragment*" })
        }
        foreach ($fragment in $task.forbidden_path_fragments) {
            $noiseHits += @($selectedPaths | Where-Object { $_ -like "*$fragment*" })
        }
        $noiseHits = @($noiseHits | Sort-Object -Unique)

        if ($missingSymbols.Count -gt 0) {
            throw "$($task.id) missed required symbols: $($missingSymbols -join ', ')"
        }
        if ($missingRoles.Count -gt 0) {
            throw "$($task.id) missed required roles: $($missingRoles -join ', ')"
        }
        if ($noiseHits.Count -gt 0) {
            throw "$($task.id) included forbidden noise: $($noiseHits -join ', ')"
        }
        if ($report.budget.rendered_bytes -gt $report.limits.max_context_bytes -or
            $report.budget.rendered_chars -gt $report.limits.max_context_chars -or
            $report.budget.estimated_tokens -gt $report.limits.max_estimated_tokens) {
            throw "$($task.id) exceeded a context budget"
        }
        if ($null -eq $report.baseline_comparison) {
            throw "$($task.id) did not produce a legacy baseline comparison"
        }

        $truncations = @(
            $report.truncation.PSObject.Properties |
                Where-Object { $_.Value -eq $true } |
                ForEach-Object { $_.Name }
        )
        $rows += [pscustomobject]@{
            id = [string]$task.id
            selected_files = [int]$report.counts.selected_files
            snippets = [int]$report.counts.snippets
            selected_chars = [int]$report.budget.rendered_chars
            selected_tokens = [int]$report.budget.estimated_tokens
            baseline_files = [int]$report.baseline_comparison.baseline_files
            baseline_tokens = [int]$report.baseline_comparison.baseline_estimated_tokens
            token_delta = [int]$report.baseline_comparison.estimated_token_delta
            token_reduction_basis_points = [int]$report.baseline_comparison.estimated_token_reduction_basis_points
            context_ms = [math]::Round(
                ([double]$report.timings.total_us - [double]$report.timings.baseline_us) / 1000.0,
                3
            )
            required_symbol_hits = [int]$task.required_symbols.Count
            required_role_hits = [int]$task.required_roles.Count
            noise_hits = 0
            truncations = @($truncations)
            analysis_complete = [bool]$report.analysis_complete
            selection_complete = [bool]$report.selection_complete
        }
    }

    $selectedTokenTotal = [int](($rows | Measure-Object selected_tokens -Sum).Sum)
    $baselineTokenTotal = [int](($rows | Measure-Object baseline_tokens -Sum).Sum)
    $tokenDeltaTotal = $baselineTokenTotal - $selectedTokenTotal
    $reductionBasisPoints = if ($baselineTokenTotal -eq 0) {
        0
    }
    else {
        [int][math]::Round(($tokenDeltaTotal * 10000.0) / $baselineTokenTotal)
    }
    $result = [ordered]@{
        schema_version = 1
        benchmark = "context_001_vs_legacy_file_priority_v1"
        corpus = "benchmarks/java-index-mini"
        task_count = $rows.Count
        selected_tokens_total = $selectedTokenTotal
        baseline_tokens_total = $baselineTokenTotal
        token_delta_total = $tokenDeltaTotal
        token_reduction_basis_points = $reductionBasisPoints
        rows = $rows
    }

    $resolvedOutput = Join-Path $projectRoot $OutputPath
    $outputDirectory = Split-Path -Parent $resolvedOutput
    if (-not (Test-Path -LiteralPath $outputDirectory)) {
        New-Item -ItemType Directory -Path $outputDirectory -Force | Out-Null
    }
    $result | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $resolvedOutput -Encoding UTF8

    $rows | Format-Table id, selected_files, snippets, selected_tokens, baseline_tokens, `
        token_reduction_basis_points, context_ms -AutoSize
    Write-Host (
        "CONTEXT-001 benchmark: {0} -> {1} estimated tokens ({2:N2}% reduction)." -f
        $baselineTokenTotal,
        $selectedTokenTotal,
        ($reductionBasisPoints / 100.0)
    )

    if ($Full) {
        Invoke-CargoChecked -Arguments @("test", "--workspace")
        Invoke-CargoChecked -Arguments @("build", "--workspace", "--release")
    }

    Write-Host "CONTEXT-001 Java context quality checks passed. Results: $resolvedOutput"
}
finally {
    Pop-Location
}
