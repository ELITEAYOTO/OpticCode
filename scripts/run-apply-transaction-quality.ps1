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

    if ($Full) {
        Invoke-CargoChecked -Arguments @(
            "clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings"
        )
        Invoke-CargoChecked -Arguments @("test", "--workspace")
        Invoke-CargoChecked -Arguments @("build", "--workspace", "--release")
    }
    else {
        Invoke-CargoChecked -Arguments @(
            "clippy", "-p", "opticcode-tools", "-p", "opticcode-cli", "--all-targets", "--", "-D", "warnings"
        )
        Invoke-CargoChecked -Arguments @("test", "-p", "opticcode-tools", "apply_transaction")
        Invoke-CargoChecked -Arguments @(
            "test", "-p", "opticcode-cli", "--test", "apply_transaction_cli"
        )
    }

    Write-Host "APPLY-001 quality checks passed."
}
finally {
    Pop-Location
}
