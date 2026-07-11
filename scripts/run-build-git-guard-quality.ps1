param()

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$runsDir = Join-Path $root "benchmarks\runs"
New-Item -ItemType Directory -Force -Path $runsDir | Out-Null

$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$runDir = Join-Path $runsDir "build-git-guard-$timestamp"
$projectDir = Join-Path $runDir "fixture-repo"
$fakeBinDir = Join-Path $runDir "fake-bin"
$jsonPath = Join-Path $runDir "build-report.json"
$stderrPath = Join-Path $runDir "build-stderr.txt"
$summaryPath = Join-Path $runDir "summary.md"

New-Item -ItemType Directory -Force -Path $projectDir | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $projectDir "src") | Out-Null
New-Item -ItemType Directory -Force -Path $fakeBinDir | Out-Null

$resolvedRunDir = [System.IO.Path]::GetFullPath($runDir)
$resolvedProjectDir = [System.IO.Path]::GetFullPath($projectDir)
if (-not $resolvedProjectDir.StartsWith($resolvedRunDir, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to create the Git fixture outside its benchmark run directory."
}

@'
<project>
  <modelVersion>4.0.0</modelVersion>
  <groupId>dev.opticcode.test</groupId>
  <artifactId>build-git-guard-fixture</artifactId>
  <version>1.0.0</version>
</project>
'@ | Set-Content -Path (Join-Path $projectDir "pom.xml") -Encoding Ascii

"clean`n" | Set-Content -Path (Join-Path $projectDir "README.md") -Encoding Ascii
"class Main {}`n" | Set-Content -Path (Join-Path $projectDir "src\Main.java") -Encoding Ascii
"<project/>`n" | Set-Content -Path (Join-Path $projectDir "dependency-reduced-pom.xml") -Encoding Ascii

@'
@echo off
> "src\Main.java" echo class Main { int generated; }
> "dependency-reduced-pom.xml" echo ^<project^>^<generated /^>^</project^>
if not exist "target" mkdir "target"
> "target\generated.txt" echo generated
exit /b 0
'@ | Set-Content -Path (Join-Path $fakeBinDir "mvn.cmd") -Encoding Ascii

Push-Location $projectDir
try {
    & git -c core.autocrlf=false init --quiet
    if ($LASTEXITCODE -ne 0) { throw "git init failed" }
    & git -c core.autocrlf=false add --all
    if ($LASTEXITCODE -ne 0) { throw "git add failed" }
    & git `
        -c core.autocrlf=false `
        -c commit.gpgsign=false `
        -c user.name="OpticCode Test" `
        -c user.email="opticcode-test@example.invalid" `
        commit --quiet --no-verify -m "build guard fixture"
    if ($LASTEXITCODE -ne 0) { throw "git commit failed" }
} finally {
    Pop-Location
}

"pre-existing user change`n" | Set-Content -Path (Join-Path $projectDir "README.md") -Encoding Ascii

Push-Location $root
try {
    & cargo build -q -p opticcode-cli
    if ($LASTEXITCODE -ne 0) { throw "OpticCode debug build failed" }
} finally {
    Pop-Location
}

$opticcode = Join-Path $root "target\debug\opticcode.exe"
$previousPath = $env:PATH
$env:PATH = "$fakeBinDir;$previousPath"
try {
    & $opticcode build `
        --path $projectDir `
        --fail-on-worktree-change `
        --json 1> $jsonPath 2> $stderrPath
    $buildExit = $LASTEXITCODE
} finally {
    $env:PATH = $previousPath
}

if ($buildExit -eq 0) {
    throw "Strict build should fail when clean tracked files change."
}

$report = Get-Content -Raw -Path $jsonPath | ConvertFrom-Json
if (-not $report.build_success) {
    throw "The simulated Maven process should succeed."
}
if ($report.overall_success) {
    throw "Overall success should be false after a strict Git violation."
}
if ($report.git_guard.status -ne "captured") {
    throw "Git guard should be captured."
}
if ($report.git_guard.strict_policy.passed) {
    throw "Strict policy should fail."
}

$originsByPath = @{}
foreach ($classified in $report.git_guard.diff.changes_after) {
    $originsByPath[$classified.change.path] = $classified.origin
}

$expectedOrigins = [ordered]@{
    "README.md" = "pre_existing"
    "dependency-reduced-pom.xml" = "build_generated"
    "src/Main.java" = "tracked_changed"
    "target/generated.txt" = "untracked_generated"
}

foreach ($entry in $expectedOrigins.GetEnumerator()) {
    if ($originsByPath[$entry.Key] -ne $entry.Value) {
        throw "Unexpected origin for $($entry.Key): expected $($entry.Value), got $($originsByPath[$entry.Key])"
    }
}

$strictCandidates = $report.git_guard.diff.counts.strict_candidates
if ($strictCandidates -ne 2) {
    throw "Expected 2 strict candidates, got $strictCandidates."
}

$summary = @(
    "# OpticCode - Build Git State Guard quality"
    ""
    "Date : $(Get-Date -Format o)"
    ""
    "- simulated Maven exit: 0"
    "- OpticCode strict exit: $buildExit"
    "- build success: $($report.build_success)"
    "- overall success: $($report.overall_success)"
    "- strict policy passed: $($report.git_guard.strict_policy.passed)"
    "- strict candidates: $strictCandidates"
    "- pre-existing: $($report.git_guard.diff.counts.pre_existing)"
    "- build-generated: $($report.git_guard.diff.counts.build_generated)"
    "- tracked-changed: $($report.git_guard.diff.counts.tracked_changed)"
    "- untracked-generated: $($report.git_guard.diff.counts.untracked_generated)"
    ""
    "JSON report: $jsonPath"
    ""
    "Fixture repository: $projectDir"
)
$summary | Set-Content -Path $summaryPath -Encoding UTF8

Write-Output "Summary: $summaryPath"
Write-Output "JSON: $jsonPath"
exit 0
