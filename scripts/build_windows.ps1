param(
    [string]$Python = 'python',
    [switch]$DebugBuild,
    [switch]$SkipChecks
)
$ErrorActionPreference = 'Stop'
$buildArgs = @((Join-Path $PSScriptRoot 'build_rust_desktop.py'))
if ($DebugBuild) { $buildArgs += '--debug' }
if ($SkipChecks) { $buildArgs += '--skip-checks' }
& $Python @buildArgs
exit $LASTEXITCODE
