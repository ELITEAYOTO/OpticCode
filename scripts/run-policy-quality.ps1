param(
    [switch]$Full
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$extensionRoot = Join-Path $repoRoot "extensions\vscode-opticcode"
$tempBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$runRoot = Join-Path $tempBase ("opticcode-policy-gate-" + [Guid]::NewGuid().ToString("N"))
$workspace = Join-Path $runRoot "Projet test Unicode"
$state = Join-Path $runRoot "state"
$previousState = $env:OPTICCODE_POLICY_STATE_DIR

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

Set-Location -LiteralPath $repoRoot
$statusBefore = git status --porcelain=v1
Assert-LastExitCode 0 "Unable to capture Git status before POLICY-001 gate"
$processesBefore = @(
    Get-Process -Name opticcode,node -ErrorAction SilentlyContinue |
        Select-Object -ExpandProperty Id |
        Sort-Object
)
$ragCurrent = Join-Path $repoRoot "data\index\CURRENT"
$ragHashBefore = if (Test-Path -LiteralPath $ragCurrent) {
    (Get-FileHash -LiteralPath $ragCurrent -Algorithm SHA256).Hash
} else {
    $null
}

try {
    New-Item -ItemType Directory -Path (Join-Path $workspace "src") -Force | Out-Null
    New-Item -ItemType Directory -Path $state -Force | Out-Null
    Set-Content -LiteralPath (Join-Path $workspace "src\Main.java") -Value "class Main {}" -Encoding utf8
    Set-Content -LiteralPath (Join-Path $workspace ".env") -Value "VALUE=test-only" -Encoding utf8
    $env:OPTICCODE_POLICY_STATE_DIR = $state

    git diff --check
    Assert-LastExitCode 0 "Git diff check failed"
    cargo fmt --all -- --check
    Assert-LastExitCode 0 "Rust formatting failed"
    cargo clippy -p opticcode-policy --all-targets --all-features -- -D warnings
    Assert-LastExitCode 0 "Policy Clippy failed"
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    Assert-LastExitCode 0 "Workspace Clippy failed"
    cargo test -p opticcode-policy
    Assert-LastExitCode 0 "Policy tests failed"
    cargo test -p opticcode-core chat_runtime::tests --lib
    Assert-LastExitCode 0 "Chat Policy tests failed"
    cargo test -p opticcode-cli --test policy_cli --test chat_cli --test discovery_cli
    Assert-LastExitCode 0 "Policy CLI, Chat CLI, or discovery tests failed"
    if ($Full) {
        cargo test --workspace
        Assert-LastExitCode 0 "Workspace tests failed"
    }
    cargo build --workspace --release
    Assert-LastExitCode 0 "Release build failed"

    foreach ($arguments in @(
        @("policy", "--help"),
        @("policy", "check", "--help"),
        @("policy", "explain", "--help"),
        @("policy", "audit", "--help")
    )) {
        & .\target\debug\opticcode.exe @arguments | Out-Null
        Assert-LastExitCode 0 "Debug Policy help failed"
        & .\target\release\opticcode.exe @arguments | Out-Null
        Assert-LastExitCode 0 "Release Policy help failed"
    }

    $base = @{
        schema_version = 1
        protocol = "opticcode.policy"
        request_id = "policy-gate-request"
        action_id = "policy-gate-action"
        origin = "cli"
        profile = "minecraft-java-1.8"
        client = @{ name = "policy-gate"; version = "1.0.0" }
        mode = "read_only"
        workspace = @{
            workspace_id = "policy-gate-workspace"
            root = $workspace
            repository = $null
            active_worktree = $null
            working_tree_digest = $null
            repository_clean = $null
        }
        approval_id = $null
    }
    $allow = $base.Clone()
    $allow.action = @{
        type = "read_file"
        data = @{ root = $workspace; path = "src/Main.java" }
    }
    $allowJson = $allow | ConvertTo-Json -Depth 12 -Compress
    $rawReport = $allowJson | & .\target\release\opticcode.exe policy check --json
    Assert-LastExitCode 0 "Release Policy allow smoke failed"
    $report = ($rawReport -join "`n") | ConvertFrom-Json
    if ($report.protocol -ne "opticcode.policy" -or
        $report.decision.decision -ne "allow" -or
        $report.decision.risk -ne "low" -or
        [string]::IsNullOrWhiteSpace($report.audit_event_id)) {
        throw "Release Policy allow smoke returned an invalid JSON report."
    }

    $deny = $base.Clone()
    $deny.request_id = "policy-gate-deny"
    $deny.action = @{
        type = "read_file"
        data = @{ root = $workspace; path = ".env" }
    }
    $denyJson = $deny | ConvertTo-Json -Depth 12 -Compress
    $rawDenied = $denyJson | & .\target\release\opticcode.exe policy check --json
    Assert-LastExitCode 11 "Release Policy deny smoke failed"
    $denied = ($rawDenied -join "`n") | ConvertFrom-Json
    if ($denied.decision.decision -ne "deny" -or $denied.decision.rule_id -ne "path.sensitive") {
        throw "Release Policy deny smoke returned an invalid JSON report."
    }

    $rawAudit = & .\target\release\opticcode.exe policy audit --json --workspace-hash $report.workspace_hash
    Assert-LastExitCode 0 "Release Policy audit smoke failed"
    $audit = ($rawAudit -join "`n") | ConvertFrom-Json
    if (@($audit.events).Count -ne 2 -or @($audit.events | Where-Object { -not $_.risk }).Count -ne 0) {
        throw "Release Policy audit smoke returned incomplete events."
    }

    Push-Location -LiteralPath $extensionRoot
    try {
        npm run compile
        Assert-LastExitCode 0 "TypeScript compilation failed"
        npm test
        Assert-LastExitCode 0 "Extension protocol tests failed"
    } finally {
        Pop-Location
    }

    $leases = & .\target\release\opticcode.exe worktrees --json | ConvertFrom-Json
    Assert-LastExitCode 0 "Worktree lease inspection failed"
    if (@($leases.leases).Count -ne 0) {
        throw "POLICY-001 left an OpticCode worktree lease."
    }
} finally {
    if ($null -eq $previousState) {
        Remove-Item Env:OPTICCODE_POLICY_STATE_DIR -ErrorAction SilentlyContinue
    } else {
        $env:OPTICCODE_POLICY_STATE_DIR = $previousState
    }
    Remove-ControlledTemp $runRoot
}

$processesAfter = @(
    Get-Process -Name opticcode,node -ErrorAction SilentlyContinue |
        Select-Object -ExpandProperty Id |
        Sort-Object
)
if (($processesBefore -join ",") -ne ($processesAfter -join ",")) {
    throw "POLICY-001 left an opticcode.exe or Node process."
}
$ragHashAfter = if (Test-Path -LiteralPath $ragCurrent) {
    (Get-FileHash -LiteralPath $ragCurrent -Algorithm SHA256).Hash
} else {
    $null
}
if ($ragHashBefore -ne $ragHashAfter) {
    throw "POLICY-001 changed the active RAG generation pointer."
}
$trackedSensitive = @(git ls-files | Where-Object {
    $_ -match '(^|/)(\.env($|\.)|id_rsa|id_ed25519|credentials\.json$)' -or
    $_ -match '\.(pem|p12|pfx|kdbx)$'
})
Assert-LastExitCode 0 "Unable to inspect tracked paths"
if ($trackedSensitive.Count -ne 0) {
    throw "Sensitive credential-like paths are tracked: $($trackedSensitive -join ', ')"
}
$statusAfter = git status --porcelain=v1
Assert-LastExitCode 0 "Unable to capture Git status after POLICY-001 gate"
if (($statusBefore -join "`n") -ne ($statusAfter -join "`n")) {
    throw "POLICY-001 quality gate changed repository state."
}

Write-Host "POLICY-001 quality gate passed."
Write-Host "Full Rust workspace executed: $Full"
