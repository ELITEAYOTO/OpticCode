[CmdletBinding()]
param(
    [switch]$WithExtensionHost,
    [switch]$Full
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$extensionRoot = Join-Path $repoRoot "extensions\vscode-opticcode"
$artifact = Join-Path $repoRoot "artifacts\opticcode-vscode-0.2.1.vsix"
Set-Location -LiteralPath $repoRoot

function Assert-LastExitCode([int]$Expected, [string]$Message) {
    if ($LASTEXITCODE -ne $Expected) {
        throw "$Message (expected exit $Expected, got $LASTEXITCODE)."
    }
}

$statusBefore = git status --porcelain=v1
Assert-LastExitCode 0 "Unable to capture Git status before CHAT-EDIT-001 gate"
$processesBefore = @(
    Get-Process -Name opticcode,node -ErrorAction SilentlyContinue |
        Select-Object -ExpandProperty Id |
        Sort-Object
)
$ragCurrent = Join-Path $repoRoot "data\index\CURRENT"
$ragHashBefore = if (Test-Path -LiteralPath $ragCurrent -PathType Leaf) {
    (Get-FileHash -LiteralPath $ragCurrent -Algorithm SHA256).Hash
} else {
    $null
}

$ignoredPentest = @(git -c core.quotepath=false ls-files --others --ignored --exclude-standard | Where-Object {
    $_ -match 'OpticCode_Pentesting_Ultra_Complet\.md$'
})
Assert-LastExitCode 0 "Unable to inspect ignored files"
$trackedPentest = @(git -c core.quotepath=false ls-files | Where-Object {
    $_ -match 'OpticCode_Pentesting_Ultra_Complet\.md$'
})
Assert-LastExitCode 0 "Unable to inspect tracked files"
if ($ignoredPentest.Count -ne 1 -or $trackedPentest.Count -ne 0) {
    throw "The private pentest document must exist exactly once, ignored and untracked."
}
git diff --check
Assert-LastExitCode 0 "Git diff check failed"
cargo fmt --all -- --check
Assert-LastExitCode 0 "Rust formatting failed"
cargo clippy --workspace --all-targets --all-features -- -D warnings
Assert-LastExitCode 0 "Workspace Clippy failed"
cargo test -p opticcode-edit
Assert-LastExitCode 0 "EditPlan, store, worktree, apply, or rollback tests failed"
cargo test -p opticcode-core --test chat_edit_workflow
Assert-LastExitCode 0 "Public Chat edit end-to-end test failed"
cargo test -p opticcode-policy
Assert-LastExitCode 0 "Policy tests failed"
cargo test -p opticcode-cli --test chat_cli --test discovery_cli
Assert-LastExitCode 0 "Chat or discovery CLI tests failed"
if ($Full) {
    cargo test --workspace
    Assert-LastExitCode 0 "Workspace tests failed"
}
cargo build --workspace --release
Assert-LastExitCode 0 "Release build failed"
cargo audit
Assert-LastExitCode 0 "Cargo audit failed"

Push-Location -LiteralPath $extensionRoot
try {
    npm ci
    Assert-LastExitCode 0 "Extension dependency installation failed"
    npm run compile
    Assert-LastExitCode 0 "TypeScript compilation failed"
    npm run lint
    Assert-LastExitCode 0 "Extension lint failed"
    npm test
    Assert-LastExitCode 0 "Extension unit tests failed"
    npm run test:integration
    Assert-LastExitCode 0 "Extension/CLI integration failed"
    if ($WithExtensionHost -or $Full) {
        npm run test:vscode
        Assert-LastExitCode 0 "VS Code Extension Host test failed"
    }
    npm run package
    Assert-LastExitCode 0 "VSIX packaging failed"
} finally {
    Pop-Location
}

if (-not (Test-Path -LiteralPath $artifact -PathType Leaf)) {
    throw "Expected VSIX was not produced: $artifact"
}
$entries = @(tar -tf $artifact)
Assert-LastExitCode 0 "Unable to inspect VSIX contents"
$forbidden = @($entries | Where-Object {
    $_ -match '(^|/)(node_modules|target|models|data|benchmarks|fixtures|reports|audits|proposals-v1)(/|$)' -or
    $_ -match '^extension/(src|test|scripts)/' -or
    $_ -match '^extension/out/test/' -or
    $_ -match '^extension/(package-lock.json|tsconfig.json|eslint.config.mjs)$' -or
    $_ -match '(?i)(pentest|OpticCode_Pentesting_Ultra_Complet)' -or
    $_ -match '(?i)\.(env|pem|key|p12|pfx|kdbx)$'
})
if ($forbidden.Count -ne 0) {
    throw "VSIX contains forbidden entries: $($forbidden -join ', ')"
}

$manifest = (& tar -xOf $artifact "extension/package.json" | Out-String) | ConvertFrom-Json
Assert-LastExitCode 0 "Unable to inspect packaged extension manifest"
if ($manifest.version -ne "0.2.1") {
    throw "Packaged extension version is $($manifest.version), expected 0.2.1."
}
$textEntries = @($entries | Where-Object {
    $_ -eq 'extension.vsixmanifest' -or
    ($_ -match '^extension/' -and $_ -match '\.(js|json|md|txt|xml|svg)$')
})
foreach ($entry in $textEntries) {
    $content = (& tar -xOf $artifact $entry | Out-String)
    Assert-LastExitCode 0 "Unable to inspect VSIX entry $entry"
    if ($content -match '(?i)([a-z]:[\\/]users[\\/]|timot|SparrowMCALL|KhopeSpigot|RAG-1\.8-Minecraft)') {
        throw "VSIX contains a personal path marker in: $entry"
    }
}

$leases = & "$repoRoot\target\release\opticcode.exe" worktrees --json | ConvertFrom-Json
Assert-LastExitCode 0 "Worktree lease inspection failed"
if (@($leases.leases).Count -ne 0) {
    throw "CHAT-EDIT-001 left an OpticCode worktree lease."
}
$processesAfter = @(
    Get-Process -Name opticcode,node -ErrorAction SilentlyContinue |
        Select-Object -ExpandProperty Id |
        Sort-Object
)
if (($processesBefore -join ",") -ne ($processesAfter -join ",")) {
    throw "CHAT-EDIT-001 left an opticcode.exe or Node process."
}
$ragHashAfter = if (Test-Path -LiteralPath $ragCurrent -PathType Leaf) {
    (Get-FileHash -LiteralPath $ragCurrent -Algorithm SHA256).Hash
} else {
    $null
}
if ($ragHashBefore -ne $ragHashAfter) {
    throw "CHAT-EDIT-001 changed the active RAG generation pointer."
}
$statusAfter = git status --porcelain=v1
Assert-LastExitCode 0 "Unable to capture Git status after CHAT-EDIT-001 gate"
if (($statusBefore -join "`n") -ne ($statusAfter -join "`n")) {
    throw "CHAT-EDIT-001 quality gate changed repository state."
}

Write-Host "CHAT-EDIT-001 quality gate passed."
Write-Host "Full Rust workspace executed: $Full"
Write-Host "Extension Host executed: $($WithExtensionHost -or $Full)"
Write-Host "VSIX: $artifact"
Write-Host "VSIX SHA-256: $((Get-FileHash -LiteralPath $artifact -Algorithm SHA256).Hash)"
