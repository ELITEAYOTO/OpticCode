[CmdletBinding()]
param(
    [string]$Model = "qwen2.5-coder:14b",
    [ValidateRange(256, 8192)]
    [int]$MaxOutputTokens = 2048,
    [ValidateRange(1000, 900000)]
    [int]$HttpTimeoutMs = 600000
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$binary = Join-Path $repoRoot "target\release\opticcode.exe"
$tempBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$runRoot = Join-Path $tempBase ("opticcode-qwen-chat-edit-" + [Guid]::NewGuid().ToString("N"))
$workspace = Join-Path $runRoot "Projet Java 8 Unicode"
$state = Join-Path $runRoot "state"
$sourceRelative = "src/main/java/test/Demo.java"
$source = Join-Path $workspace $sourceRelative
$previousLocalAppData = $env:LOCALAPPDATA
$previousPolicyState = $env:OPTICCODE_POLICY_STATE_DIR

function Assert-LastExitCode([int]$Expected, [string]$Message) {
    if ($LASTEXITCODE -ne $Expected) {
        throw "$Message (expected exit $Expected, got $LASTEXITCODE)."
    }
}

function Remove-ControlledTemp([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return }
    $resolved = (Resolve-Path -LiteralPath $Path).Path
    if (-not $resolved.StartsWith($tempBase, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to remove a path outside the system temp directory: $resolved"
    }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}

if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
    throw "Release OpticCode executable is missing: $binary"
}
$models = @(ollama list 2>$null | Select-Object -Skip 1)
Assert-LastExitCode 0 "Unable to list local Ollama models"
if (-not ($models | Where-Object { $_ -match "^$([regex]::Escape($Model))\s" })) {
    throw "The requested local Ollama model is not installed: $Model"
}

try {
    New-Item -ItemType Directory -Path (Split-Path -Parent $source) -Force | Out-Null
    New-Item -ItemType Directory -Path (Join-Path $state "policy") -Force | Out-Null
    $utf8 = New-Object Text.UTF8Encoding($false)
    [IO.File]::WriteAllText(
        $source,
        "package test;`n`npublic final class Demo {`n    public int value() {`n        return 1;`n    }`n}`n",
        $utf8
    )
    [IO.File]::WriteAllText(
        (Join-Path $workspace "pom.xml"),
        "<project xmlns=`"http://maven.apache.org/POM/4.0.0`">`n<modelVersion>4.0.0</modelVersion>`n<groupId>test</groupId><artifactId>qwen-smoke</artifactId><version>1</version>`n<properties><maven.compiler.source>1.8</maven.compiler.source><maven.compiler.target>1.8</maven.compiler.target></properties>`n</project>`n",
        $utf8
    )
    [IO.File]::WriteAllText(
        (Join-Path $workspace ".gitignore"),
        "target/`n.gradle/`nbuild/`n.opticcode/`n",
        $utf8
    )

    git -C $workspace init --quiet
    Assert-LastExitCode 0 "Unable to initialize the Qwen smoke repository"
    git -C $workspace add --all
    Assert-LastExitCode 0 "Unable to stage the Qwen smoke fixture"
    git -C $workspace `
        -c "user.name=OpticCode Test" `
        -c "user.email=opticcode@example.invalid" `
        commit --quiet -m fixture
    Assert-LastExitCode 0 "Unable to commit the Qwen smoke fixture"

    $baseline = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash
    $env:LOCALAPPDATA = $state
    $env:OPTICCODE_POLICY_STATE_DIR = Join-Path $state "policy"
    $request = @{
        schema_version = 1
        protocol = "opticcode.chat"
        request_id = "qwen-chat-edit-smoke-001"
        workspace_id = "workspace-qwen-chat-edit-smoke"
        workspace_root = $workspace
        command = "fix"
        prompt = "In src/main/java/test/Demo.java, change only the integer returned by Demo#value from 1 to 2."
        profile = "minecraft-java-1.8"
        provider = "ollama"
        model = $Model
        context_mode = "legacy"
        references = @(@{
            reference_id = "demo-source"
            inclusion_reason = "Explicit target selected for the bounded Qwen smoke."
            kind = "file"
            path = $sourceRelative
        })
        history = @()
        budgets = @{
            max_history_turns = 12
            max_history_chars = 32768
            max_history_tokens = 8192
            max_references = 24
            max_reference_bytes = 1048576
            max_prompt_tokens = 32768
            rag_hits = 0
        }
        generation = @{
            max_output_tokens = $MaxOutputTokens
            temperature = 0.0
            seed = 1
            brief = $true
            compare_generate = $false
        }
        security_mode = "read_only"
        client = @{
            name = "opticcode-vscode"
            version = "0.2.0"
            vscode_version = "1.125.0"
            session_id = "qwen-chat-edit-smoke-session"
            locale = "en"
            recent_run_ids = @()
            previous_repository_state = $null
        }
        expected_protocols = @{ chat = 1; assistant = 1; discovery = 1; llm = 1 }
    }
    $requestLine = $request | ConvertTo-Json -Depth 12 -Compress
    $rawEvents = $requestLine | & $binary chat `
        --protocol-jsonl `
        --keep-alive 0 `
        --rag-index (Join-Path $runRoot "missing-rag") `
        --http-timeout-ms $HttpTimeoutMs
    $chatExit = $LASTEXITCODE
    $events = @($rawEvents | ForEach-Object { $_ | ConvertFrom-Json })
    $terminals = @($events | Where-Object { $_.type -in @("completed", "failed", "cancelled") })
    $proposals = @($events | Where-Object { $_.type -eq "proposal_stored" })
    $verifications = @($events | Where-Object { $_.type -eq "verification_completed" })
    $formatCorrections = @($events | Where-Object {
        $_.type -eq "warning" -and $_.code -eq "edit_plan_format_corrected"
    })
    $current = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash
    $status = @(git -C $workspace status --porcelain=v1)
    Assert-LastExitCode 0 "Unable to inspect the Qwen smoke repository"
    $worktreeCount = @(
        git -C $workspace worktree list --porcelain |
            Where-Object { $_ -like "worktree *" }
    ).Count
    Assert-LastExitCode 0 "Unable to inspect Qwen smoke worktrees"
    $leases = & $binary worktrees --json | ConvertFrom-Json
    Assert-LastExitCode 0 "Unable to inspect OpticCode leases after Qwen smoke"

    $success = $chatExit -eq 0 -and
        $terminals.Count -eq 1 -and
        $terminals[0].type -eq "completed" -and
        $proposals.Count -eq 1 -and
        $verifications.Count -eq 1 -and
        $verifications[0].success -eq $true -and
        $formatCorrections.Count -eq 0 -and
        $baseline -eq $current -and
        $status.Count -eq 0 -and
        $worktreeCount -eq 1 -and
        @($leases.leases).Count -eq 0
    if (-not $success) {
        $events | ConvertTo-Json -Depth 12 | Write-Host
        throw "Bounded real Qwen CHAT-EDIT smoke did not complete safely."
    }

    Write-Host "CHAT-EDIT-001 real Qwen smoke passed."
    Write-Host "Model: $Model"
    Write-Host "Proposal: $($proposals[0].proposal_id)"
    Write-Host "Events: $($events.Count)"
    Write-Host "Elapsed: $($terminals[0].elapsed_ms) ms"
    Write-Host "Format corrections: $($formatCorrections.Count)"
    Write-Host "Original source unchanged: $($baseline -eq $current)"
    Write-Host "Remaining worktrees: $worktreeCount"
    Write-Host "Remaining leases: $(@($leases.leases).Count)"
} finally {
    $env:LOCALAPPDATA = $previousLocalAppData
    if ($null -eq $previousPolicyState) {
        Remove-Item Env:OPTICCODE_POLICY_STATE_DIR -ErrorAction SilentlyContinue
    } else {
        $env:OPTICCODE_POLICY_STATE_DIR = $previousPolicyState
    }
    Remove-ControlledTemp $runRoot
}
