[CmdletBinding()]
param(
    [string]$PackageRoot = $PSScriptRoot,
    [switch]$Json
)

$ErrorActionPreference = 'Stop'
$root = [System.IO.Path]::GetFullPath($PackageRoot)
$checks = [System.Collections.Generic.List[object]]::new()

function Add-Check {
    param([string]$Id, [bool]$Ok, [string]$Detail)
    $checks.Add([pscustomobject]@{ id = $Id; ok = $Ok; detail = $Detail })
}

$required = [ordered]@{
    'homebase' = 'homebase.exe'
    'bundle_manifest' = 'distribution\bundle-manifest.json'
    'python' = 'distribution\runtime\python\python.exe'
    'ffmpeg' = 'distribution\tools\ffmpeg.exe'
    'sops' = 'distribution\tools\sops.exe'
    'age_keygen' = 'distribution\tools\age-keygen.exe'
    'reveal' = 'distribution\bin\homebase-reveal.exe'
    'asr_runner' = 'distribution\aihub\tools\local-asr\runner\asr_task_runner.py'
    'model_manager' = 'distribution\aihub\tools\model-manager\model_manager.py'
    'asr_presets' = 'distribution\aihub\capabilities\asr-presets.json'
    'model_catalog' = 'distribution\aihub\capabilities\model-download-catalog.json'
}

foreach ($entry in $required.GetEnumerator()) {
    $path = Join-Path $root $entry.Value
    Add-Check $entry.Key (Test-Path -LiteralPath $path -PathType Leaf) $path
}

$python = Join-Path $root $required.python
if (Test-Path -LiteralPath $python -PathType Leaf) {
    $import = & $python -c "import av, ctranslate2, faster_whisper, huggingface_hub, requests; print('runtime-import-ok')" 2>&1
    Add-Check 'python_imports' ($LASTEXITCODE -eq 0 -and "$import" -match 'runtime-import-ok') "$import"

    $runner = Join-Path $root $required.asr_runner
    $help = & $python $runner --help 2>&1
    Add-Check 'asr_runner_cli' ($LASTEXITCODE -eq 0 -and "$help" -match 'preflight') (($help | Select-Object -First 2) -join ' ')
}

$ffmpeg = Join-Path $root $required.ffmpeg
if (Test-Path -LiteralPath $ffmpeg -PathType Leaf) {
    $version = & $ffmpeg -version 2>&1 | Select-Object -First 1
    Add-Check 'ffmpeg_runtime' ($LASTEXITCODE -eq 0 -and "$version" -match '^ffmpeg version') "$version"
}

$failed = @($checks | Where-Object { -not $_.ok })
$result = [pscustomobject]@{
    schema_version = 1
    package_root = $root
    ready = $failed.Count -eq 0
    check_count = $checks.Count
    failed_count = $failed.Count
    checks = $checks
}

if ($Json) {
    $result | ConvertTo-Json -Depth 5
} else {
    $checks | Format-Table id, ok, detail -AutoSize
    if ($result.ready) {
        Write-Host 'HomeBase package doctor: READY'
    } else {
        Write-Host "HomeBase package doctor: FAILED ($($failed.Count))"
    }
}

if (-not $result.ready) { exit 1 }
