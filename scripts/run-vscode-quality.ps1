param(
    [switch]$WithLlm,
    [switch]$WithExtensionHost
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$extensionRoot = Join-Path $repoRoot "extensions\vscode-opticcode"
$artifact = Join-Path $repoRoot "artifacts\opticcode-vscode-0.2.0.vsix"
Set-Location -LiteralPath $repoRoot

$statusBefore = git status --porcelain=v1
if ($LASTEXITCODE -ne 0) { throw "Unable to capture Git status before VSCODE-001 gate." }

cargo fmt --all -- --check
if ($LASTEXITCODE -ne 0) { throw "Rust formatting failed." }
cargo clippy --workspace --all-targets --all-features -- -D warnings
if ($LASTEXITCODE -ne 0) { throw "Rust Clippy failed." }
cargo test --workspace
if ($LASTEXITCODE -ne 0) { throw "Rust tests failed." }
cargo build --workspace --release
if ($LASTEXITCODE -ne 0) { throw "Rust release build failed." }

Push-Location -LiteralPath $extensionRoot
try {
    npm ci
    if ($LASTEXITCODE -ne 0) { throw "Extension dependency installation failed." }
    npm run compile
    if ($LASTEXITCODE -ne 0) { throw "TypeScript compilation failed." }
    npm run lint
    if ($LASTEXITCODE -ne 0) { throw "Extension lint failed." }
    npm test
    if ($LASTEXITCODE -ne 0) { throw "Extension unit tests failed." }
    npm run test:integration
    if ($LASTEXITCODE -ne 0) { throw "Real OpticCode integration failed." }
    if ($WithLlm) {
        npm run test:assistant
        if ($LASTEXITCODE -ne 0) { throw "Real Ask/Plan streaming smoke failed." }
    }
    if ($WithExtensionHost) {
        npm run test:vscode
        if ($LASTEXITCODE -ne 0) { throw "VS Code Extension Development Host test failed." }
    }
    npm run package
    if ($LASTEXITCODE -ne 0) { throw "VSIX packaging failed." }
} finally {
    Pop-Location
}

if (-not (Test-Path -LiteralPath $artifact -PathType Leaf)) {
    throw "Expected VSIX was not produced: $artifact"
}
$entries = @(tar -tf $artifact)
if ($LASTEXITCODE -ne 0) { throw "Unable to inspect VSIX contents." }
$forbidden = @($entries | Where-Object {
    $_ -match '(^|/)(node_modules|target|models|data/index|benchmarks/runs|Id.es-Vrac)(/|$)' -or
    $_ -match '^extension/(src|test|scripts)/' -or
    $_ -match '^extension/out/test/' -or
    $_ -match '^extension/(package-lock.json|tsconfig.json|eslint.config.mjs)$' -or
    $_ -match '\.(env|pem|key)$'
})
if ($forbidden.Count -ne 0) {
    throw "VSIX contains forbidden entries: $($forbidden -join ', ')"
}

$textEntries = @($entries | Where-Object {
    $_ -eq 'extension.vsixmanifest' -or
    ($_ -match '^extension/' -and $_ -match '\.(js|json|md|txt|xml|svg)$')
})
foreach ($entry in $textEntries) {
    $content = (& tar -xOf $artifact $entry | Out-String)
    if ($LASTEXITCODE -ne 0) { throw "Unable to inspect VSIX entry: $entry" }
    if ($content -match '(?i)([a-z]:[\\/]users[\\/]|timot|SparrowMCALL|KhopeSpigot|RAG-1\.8-Minecraft)') {
        throw "VSIX contains a personal path marker in: $entry"
    }
}

$leases = & "$repoRoot\target\release\opticcode.exe" worktrees --json | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or @($leases.leases).Count -ne 0) {
    throw "VSCODE-001 left an OpticCode worktree lease."
}

$statusAfter = git status --porcelain=v1
if ($LASTEXITCODE -ne 0) { throw "Unable to capture Git status after VSCODE-001 gate." }
if (($statusBefore -join "`n") -ne ($statusAfter -join "`n")) {
    throw "VSCODE-001 quality gate changed tracked repository state."
}

Write-Host "VSCODE-001 quality gate passed."
Write-Host "VSIX: $artifact"
Write-Host "VSIX SHA-256: $((Get-FileHash -LiteralPath $artifact -Algorithm SHA256).Hash)"
Write-Host "Real LLM smoke executed: $WithLlm"
Write-Host "Extension Development Host executed: $WithExtensionHost"
