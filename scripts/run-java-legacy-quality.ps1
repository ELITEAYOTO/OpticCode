param(
    [switch]$Full
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$mavenRoot = Join-Path $env:USERPROFILE ".m2\repository\org\spigotmc\spigot-api"

function Read-ZipEntry {
    param(
        [Parameter(Mandatory = $true)][string]$Archive,
        [Parameter(Mandatory = $true)][string]$EntryName
    )

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [System.IO.Compression.ZipFile]::OpenRead($Archive)
    try {
        $entry = $zip.GetEntry($EntryName)
        if ($null -eq $entry) {
            throw "Missing archive entry $EntryName in $Archive"
        }
        $reader = [System.IO.StreamReader]::new($entry.Open())
        try {
            return $reader.ReadToEnd()
        }
        finally {
            $reader.Dispose()
        }
    }
    finally {
        $zip.Dispose()
    }
}

function Find-PinnedArtifact {
    param([Parameter(Mandatory = $true)][string]$Artifact)

    $matches = @(Get-ChildItem -LiteralPath $mavenRoot -Filter $Artifact -File -Recurse)
    if ($matches.Count -ne 1) {
        throw "Expected one cached $Artifact below $mavenRoot, found $($matches.Count)"
    }
    return $matches[0].FullName
}

Push-Location $repoRoot
try {
    cargo fmt --all -- --check
    if ($LASTEXITCODE -ne 0) { throw "cargo fmt failed" }

    cargo test -p opticcode-tools java_edits::
    if ($LASTEXITCODE -ne 0) { throw "legacy tool tests failed" }

    cargo test -p opticcode-cli --test java_edits_cli
    if ($LASTEXITCODE -ne 0) { throw "legacy CLI tests failed" }

    $catalogJson = cargo run -q -- java-legacy-rules --json
    if ($LASTEXITCODE -ne 0) { throw "legacy catalog command failed" }
    $catalog = $catalogJson | ConvertFrom-Json

    $sourceText = @{}
    foreach ($source in $catalog.sources) {
        $artifactPath = Find-PinnedArtifact -Artifact $source.artifact
        $actualHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $artifactPath).Hash.ToLowerInvariant()
        if ($actualHash -ne $source.sha256) {
            throw "SHA-256 mismatch for $($source.id): expected $($source.sha256), found $actualHash"
        }
        $sourceText[$source.id] = @{
            Material = Read-ZipEntry -Archive $artifactPath -EntryName "org/bukkit/Material.java"
            EntityType = Read-ZipEntry -Archive $artifactPath -EntryName "org/bukkit/entity/EntityType.java"
        }
    }

    foreach ($rule in $catalog.rules) {
        $typeName = if ($rule.owner -eq "org.bukkit.Material") { "Material" } else { "EntityType" }
        $legacyMember = ($rule.legacy -split '\.')[-1]
        $legacyPattern = "(?m)^\s*" + [regex]::Escape($legacyMember) + "\s*\("
        if ($sourceText[$rule.legacy_source_id][$typeName] -notmatch $legacyPattern) {
            throw "Legacy target $($rule.legacy) is absent from source $($rule.legacy_source_id)"
        }

        if ($null -ne $rule.modern_source_id) {
            $modernMember = ($rule.modern -split '\.')[-1]
            $modernPattern = "(?m)^\s*" + [regex]::Escape($modernMember) + "\s*\("
            if ($sourceText[$rule.modern_source_id][$typeName] -notmatch $modernPattern) {
                throw "Modern source $($rule.modern) is absent from source $($rule.modern_source_id)"
            }
        }
    }

    mvn -q -o -f benchmarks/java-legacy-compile/pom.xml test
    if ($LASTEXITCODE -ne 0) { throw "Bukkit 1.8.8 compile fixture failed" }

    if ($Full) {
        cargo clippy --workspace --all-targets --all-features -- -D warnings
        if ($LASTEXITCODE -ne 0) { throw "cargo clippy failed" }
        cargo test --workspace
        if ($LASTEXITCODE -ne 0) { throw "workspace tests failed" }
    }

    Write-Host "LEGACY-002 quality gate passed: $($catalog.rules.Count) rules verified."
}
finally {
    Pop-Location
}
