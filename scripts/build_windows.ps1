param(
    [string]$Python = 'python',
    [switch]$DebugBuild,
    [switch]$SkipChecks
)
$ErrorActionPreference = 'Stop'
$buildArgs = @((Join-Path $PSScriptRoot 'build_rust_desktop.py'))
if ($DebugBuild) { $buildArgs += '--debug' }
if ($SkipChecks) { $buildArgs += '--skip-checks' }
# Windows PowerShell 5.1 turns a native command's stderr, such as cargo's
# progress, into errors when the output is redirected: under 'Stop' they
# would end a build that succeeds. Its exit code alone tells.
$ErrorActionPreference = 'Continue'
& $Python @buildArgs
exit $LASTEXITCODE
