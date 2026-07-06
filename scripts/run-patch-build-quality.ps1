param(
    [string]$ProjectPath = "benchmarks/mini-bukkit-plugin"
)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$runsDir = Join-Path $root "benchmarks\runs"
New-Item -ItemType Directory -Force -Path $runsDir | Out-Null

$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$runDir = Join-Path $runsDir "patch-build-quality-$timestamp"
$workspace = Join-Path $runDir "workspace"
$summaryPath = Join-Path $runDir "summary.md"
$jsonPath = Join-Path $runDir "result.json"
$patchOutputPath = Join-Path $runDir "patch-output.txt"
$patchPath = Join-Path $runDir "proposal.patch"
$beforeBuildPath = Join-Path $runDir "build-before.txt"
$afterBuildPath = Join-Path $runDir "build-after.txt"

New-Item -ItemType Directory -Force -Path $runDir | Out-Null

$sourceProject = Join-Path $root $ProjectPath
Copy-Item -Path $sourceProject -Destination $workspace -Recurse

$resolvedRunDir = [System.IO.Path]::GetFullPath($runDir)
$resolvedWorkspace = [System.IO.Path]::GetFullPath($workspace)
if (-not $resolvedWorkspace.StartsWith($resolvedRunDir, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to operate outside benchmark run directory."
}

$copiedTarget = Join-Path $workspace "target"
if (Test-Path $copiedTarget) {
    Remove-Item -LiteralPath $copiedTarget -Recurse -Force
}

$listenerPath = Join-Path $workspace "src\main\java\dev\opticcode\benchmark\listener\JoinListener.java"
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
    & cargo run -q -- build --path $workspace 1> $beforeBuildPath 2>&1
    $beforeBuildExit = $LASTEXITCODE

    & cargo run -q -- patch --path $workspace --check 1> $patchOutputPath 2>&1
    $patchExit = $LASTEXITCODE
} finally {
    Pop-Location
}

$patchOutput = Get-Content -Raw -Path $patchOutputPath
$diffIndex = $patchOutput.IndexOf("diff --git")
if ($diffIndex -lt 0) {
    $diffIndex = $patchOutput.IndexOf("--- a/")
}
if ($diffIndex -lt 0) {
    throw "No unified diff found in patch output."
}

$notesIndex = $patchOutput.IndexOf("`nNotes:", $diffIndex)
if ($notesIndex -lt 0) {
    $notesIndex = $patchOutput.IndexOf("`nPatch check:", $diffIndex)
}
if ($notesIndex -lt 0) {
    $notesIndex = $patchOutput.Length
}

$patchText = $patchOutput.Substring($diffIndex, $notesIndex - $diffIndex).TrimStart()
Set-Content -Path $patchPath -Value $patchText

& git -C $workspace apply --ignore-space-change --ignore-whitespace $patchPath
$applyExit = $LASTEXITCODE

Push-Location $root
try {
    & cargo run -q -- build --path $workspace 1> $afterBuildPath 2>&1
    $afterBuildExit = $LASTEXITCODE
} finally {
    Pop-Location
}

$patchedListener = Get-Content -Raw -Path $listenerPath
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

$success = $beforeBuildExit -ne 0 `
    -and $patchExit -eq 0 `
    -and $applyExit -eq 0 `
    -and $afterBuildExit -eq 0 `
    -and $missing.Count -eq 0 `
    -and $remainingModern.Count -eq 0

$result = [ordered]@{
    timestamp = (Get-Date).ToString("o")
    project_path = $ProjectPath
    workspace = $workspace
    before_build_exit = $beforeBuildExit
    patch_check_exit = $patchExit
    apply_exit = $applyExit
    after_build_exit = $afterBuildExit
    expected = $expected
    missing = $missing
    remaining_modern = $remainingModern
    success = $success
    summary_path = $summaryPath
    patch_path = $patchPath
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
$summary.Add("| patch --check | $patchExit | succes |")
$summary.Add("| git apply | $applyExit | succes |")
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
$summary.Add("Patch : $patchPath")
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
