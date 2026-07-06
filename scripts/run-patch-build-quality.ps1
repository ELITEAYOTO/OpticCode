param(
    [string]$ProjectPath = "benchmarks/mini-bukkit-plugin"
)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$runsDir = Join-Path $root "benchmarks\runs"
New-Item -ItemType Directory -Force -Path $runsDir | Out-Null

$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$runDir = Join-Path $runsDir "patch-build-quality-$timestamp"
$sourceWorkspace = Join-Path $runDir "source"
$targetWorkspace = Join-Path $runDir "target"
$summaryPath = Join-Path $runDir "summary.md"
$jsonPath = Join-Path $runDir "result.json"
$applyOutputPath = Join-Path $runDir "apply-output.txt"
$beforeBuildPath = Join-Path $runDir "build-before.txt"
$afterBuildPath = Join-Path $runDir "build-after.txt"

New-Item -ItemType Directory -Force -Path $runDir | Out-Null

$sourceProject = Join-Path $root $ProjectPath
Copy-Item -Path $sourceProject -Destination $sourceWorkspace -Recurse

$resolvedRunDir = [System.IO.Path]::GetFullPath($runDir)
$resolvedSource = [System.IO.Path]::GetFullPath($sourceWorkspace)
$resolvedTarget = [System.IO.Path]::GetFullPath($targetWorkspace)
if (-not $resolvedSource.StartsWith($resolvedRunDir, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to operate outside benchmark run directory."
}
if (-not $resolvedTarget.StartsWith($resolvedRunDir, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to copy outside benchmark run directory."
}

$copiedBuildDir = Join-Path $sourceWorkspace "target"
if (Test-Path $copiedBuildDir) {
    Remove-Item -LiteralPath $copiedBuildDir -Recurse -Force
}

$listenerPath = Join-Path $sourceWorkspace "src\main\java\dev\opticcode\benchmark\listener\JoinListener.java"
$listener = Get-Content -Raw -Path $listenerPath
$brokenBlock = @'
        player.getInventory().addItem(new ItemStack(Material.GUNPOWDER, 1));
        player.getInventory().addItem(new ItemStack(Material.NETHER_WART, 1));
        player.getInventory().addItem(new ItemStack(Material.SPAWNER, 1));
        player.getInventory().addItem(new ItemStack(Material.WOODEN_SHOVEL, 1));
        player.getInventory().addItem(new ItemStack(Material.SPAWN_EGG, 1));
'@
$listener = $listener.Replace(
    "        player.getInventory().addItem(new ItemStack(Material.SULPHUR, 1));",
    $brokenBlock.TrimEnd()
)
Set-Content -Path $listenerPath -Value $listener

Push-Location $root
try {
    & cargo run -q -- build --path $sourceWorkspace 1> $beforeBuildPath 2>&1
    $beforeBuildExit = $LASTEXITCODE

    & cargo run -q -- apply --path $sourceWorkspace --copy-to $targetWorkspace --yes 1> $applyOutputPath 2>&1
    $applyExit = $LASTEXITCODE
} finally {
    Pop-Location
}

Push-Location $root
try {
    & cargo run -q -- build --path $targetWorkspace 1> $afterBuildPath 2>&1
    $afterBuildExit = $LASTEXITCODE
} finally {
    Pop-Location
}

$patchedListenerPath = Join-Path $targetWorkspace "src\main\java\dev\opticcode\benchmark\listener\JoinListener.java"
$patchedListener = Get-Content -Raw -Path $patchedListenerPath
$sourceListener = Get-Content -Raw -Path $listenerPath
$expected = @(
    "Material.SULPHUR",
    "Material.NETHER_STALK",
    "Material.MOB_SPAWNER",
    "Material.WOOD_SPADE",
    "Material.MONSTER_EGG"
)
$forbidden = @(
    "Material.GUNPOWDER",
    "Material.NETHER_WART",
    "Material.SPAWNER",
    "Material.WOODEN_SHOVEL",
    "Material.SPAWN_EGG"
)

$missing = @($expected | Where-Object { -not $patchedListener.Contains($_) })
$remainingModern = @($forbidden | Where-Object { $patchedListener.Contains($_) })
$sourceStillBroken = $sourceListener.Contains("Material.GUNPOWDER")

$success = $beforeBuildExit -ne 0 `
    -and $applyExit -eq 0 `
    -and $afterBuildExit -eq 0 `
    -and $missing.Count -eq 0 `
    -and $remainingModern.Count -eq 0 `
    -and $sourceStillBroken

$result = [ordered]@{
    timestamp = (Get-Date).ToString("o")
    project_path = $ProjectPath
    source_workspace = $sourceWorkspace
    target_workspace = $targetWorkspace
    before_build_exit = $beforeBuildExit
    apply_exit = $applyExit
    after_build_exit = $afterBuildExit
    expected = $expected
    missing = $missing
    remaining_modern = $remainingModern
    source_still_broken = $sourceStillBroken
    success = $success
    summary_path = $summaryPath
    apply_output_path = $applyOutputPath
    before_build_path = $beforeBuildPath
    after_build_path = $afterBuildPath
}

$result | ConvertTo-Json -Depth 6 | Set-Content -Path $jsonPath

$summary = New-Object System.Collections.Generic.List[string]
$summary.Add("# OpticCode - Qualite patch + build")
$summary.Add("")
$summary.Add("Date : $(Get-Date -Format o)")
$summary.Add("")
$summary.Add("| Etape | Exit | Attendu |")
$summary.Add("| --- | ---: | --- |")
$summary.Add("| build avant patch | $beforeBuildExit | echec |")
$summary.Add("| apply --copy-to --yes | $applyExit | succes |")
$summary.Add("| build apres patch | $afterBuildExit | succes |")
$summary.Add("")
$summary.Add("Succes global : $success")
$summary.Add("")
$summary.Add("Symboles attendus : $($expected -join ', ')")
$summary.Add("")
$summary.Add("Manquants : $(if ($missing.Count -eq 0) { '-' } else { $missing -join ', ' })")
$summary.Add("")
$summary.Add("Symboles modernes restants : $(if ($remainingModern.Count -eq 0) { '-' } else { $remainingModern -join ', ' })")
$summary.Add("")
$summary.Add("Source conservee cassee : $sourceStillBroken")
$summary.Add("")
$summary.Add("Source temporaire : $sourceWorkspace")
$summary.Add("")
$summary.Add("Cible appliquee : $targetWorkspace")
$summary.Add("")
$summary.Add("Apply output : $applyOutputPath")
$summary.Add("")
$summary.Add("Build avant : $beforeBuildPath")
$summary.Add("")
$summary.Add("Build apres : $afterBuildPath")
$summary | Set-Content -Path $summaryPath

Write-Output "Summary: $summaryPath"
Write-Output "JSON: $jsonPath"
if (-not $success) {
    exit 1
}
