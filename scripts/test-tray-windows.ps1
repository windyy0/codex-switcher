param([string]$Filter = 'tray::')
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path $PSScriptRoot -Parent

# Tauri embeds its Common Controls v6 manifest in application binaries only.
# Native menu unit tests also need it, or Windows exits before the harness runs
# with STATUS_ENTRYPOINT_NOT_FOUND (TaskDialogIndirect in comctl32.dll).
$mtCommand = Get-Command mt.exe -ErrorAction SilentlyContinue
if ($mtCommand) {
    $manifestTool = $mtCommand.Source
} else {
    $sdkRoot = (Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots').KitsRoot10
    $manifestTool = Get-ChildItem -LiteralPath (Join-Path $sdkRoot 'bin') -Directory |
        Sort-Object Name -Descending |
        ForEach-Object { Join-Path $_.FullName 'x64\mt.exe' } |
        Where-Object { Test-Path -LiteralPath $_ } |
        Select-Object -First 1
}
if (-not $manifestTool) { throw 'Windows SDK mt.exe is required to run native menu tests.' }

$artifacts = & cargo test --manifest-path (Join-Path $projectRoot 'src-tauri\Cargo.toml') --locked --lib --no-run --message-format=json
if ($LASTEXITCODE -ne 0) { throw 'Failed to build the Rust test harness.' }
$testExecutables = @($artifacts | ForEach-Object { $_ | ConvertFrom-Json } |
    Where-Object { $_.reason -eq 'compiler-artifact' -and $_.profile.test -and $_.executable } |
    ForEach-Object { $_.executable })
if ($testExecutables.Count -ne 1) { throw 'Expected exactly one library test executable.' }
$testExecutable = $testExecutables[0]
$manifest = Join-Path $projectRoot 'src-tauri\windows\test.manifest'
& $manifestTool -nologo -manifest $manifest "-outputresource:$testExecutable;#1"
if ($LASTEXITCODE -ne 0) { throw 'Failed to embed the test manifest.' }
& $testExecutable $Filter --nocapture --test-threads=1
exit $LASTEXITCODE
