[CmdletBinding()]
param(
    [switch]$Full
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$extensionRoot = Join-Path $repoRoot "extensions\vscode-opticcode"
Set-Location -LiteralPath $repoRoot

function Assert-Exit([string]$Message) {
    if ($LASTEXITCODE -ne 0) { throw "$Message (exit $LASTEXITCODE)." }
}

$statusBefore = git status --porcelain=v1
Assert-Exit "Unable to capture Git status before CHAT-METRICS gate"
cargo fmt --all -- --check
Assert-Exit "Rust formatting failed"
cargo test -p opticcode-core chat_runtime
Assert-Exit "Rust chat timing tests failed"
if ($Full) {
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    Assert-Exit "Workspace Clippy failed"
}

Push-Location -LiteralPath $extensionRoot
try {
    npm run compile
    Assert-Exit "TypeScript compilation failed"
    npm run lint
    Assert-Exit "Extension lint failed"
    npm test
    Assert-Exit "Extension timing tests failed"
} finally {
    Pop-Location
}

$statusAfter = git status --porcelain=v1
Assert-Exit "Unable to capture Git status after CHAT-METRICS gate"
if (($statusBefore -join "`n") -ne ($statusAfter -join "`n")) {
    throw "CHAT-METRICS quality gate changed repository state."
}
Write-Host "CHAT-METRICS quality gate passed."
