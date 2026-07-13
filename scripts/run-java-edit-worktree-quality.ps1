[CmdletBinding()]
param(
    [switch]$Full
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

Push-Location $projectRoot
try {
    Invoke-CargoChecked -Arguments @("fmt", "--all", "--", "--check")
    Invoke-CargoChecked -Arguments @(
        "clippy", "-p", "opticcode-tools", "-p", "opticcode-cli", "--all-targets", "--", "-D", "warnings"
    )
    Invoke-CargoChecked -Arguments @(
        "test", "-p", "opticcode-tools", "java_edit_worktree"
    )
    Invoke-CargoChecked -Arguments @(
        "test", "-p", "opticcode-cli", "--test", "java_edits_verify_cli", "--", "--test-threads=1"
    )
    Invoke-CargoChecked -Arguments @(
        "test", "-p", "opticcode-cli", "--test", "worktree_verify_cli", "--", "--test-threads=1"
    )

    if ($Full) {
        Invoke-CargoChecked -Arguments @("test", "--workspace")
        Invoke-CargoChecked -Arguments @("build", "--workspace", "--release")
    }

    Write-Host "CODE-001B3 Java edit worktree quality checks passed."
}
finally {
    Pop-Location
}
