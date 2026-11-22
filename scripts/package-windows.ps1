[CmdletBinding()]
param(
    [string]$OutputRoot = (Join-Path $PSScriptRoot '..\release'),
    [switch]$SkipTests,
    [switch]$ForceDownload
)

$ErrorActionPreference = 'Stop'
$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$version = (Get-Content -LiteralPath (Join-Path $repoRoot 'package.json') -Raw -Encoding utf8 | ConvertFrom-Json).version
$outputBase = [System.IO.Path]::GetFullPath($OutputRoot)
$releaseRoot = [System.IO.Path]::GetFullPath((Join-Path $outputBase $version))
$stageRoot = Join-Path $repoRoot 'src-tauri\resources\distribution'
$targetRelease = Join-Path $repoRoot 'src-tauri\target\release'
$portableName = "HomeBase-$version-windows-x64-portable"
$portableRoot = Join-Path $releaseRoot $portableName
$utf8 = [System.Text.UTF8Encoding]::new($false)

if (-not $releaseRoot.StartsWith($outputBase + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to replace release directory outside $outputBase"
}
Push-Location $repoRoot
try {
    Write-Host '=== Preparing distributable resources ==='
    $prepareArgs = @('-NoProfile', '-File', (Join-Path $repoRoot 'scripts\prepare-windows-bundle.ps1'))
    if ($ForceDownload) { $prepareArgs += '-ForceDownload' }
    & pwsh @prepareArgs
    if ($LASTEXITCODE -ne 0) { throw 'Resource preparation failed.' }

    Write-Host '=== Building frontend assets for native builds ==='
    & npm.cmd run build
    if ($LASTEXITCODE -ne 0) { throw 'Frontend build failed.' }

    if (-not $SkipTests) {
        Write-Host '=== Running release gates ==='
        & npm.cmd run test:asr
        if ($LASTEXITCODE -ne 0) { throw 'ASR frontend tests failed.' }
        Push-Location (Join-Path $repoRoot 'src-tauri')
        try {
            & cargo test --locked --lib
            if ($LASTEXITCODE -ne 0) { throw 'Rust tests failed.' }
        } finally {
            Pop-Location
        }
    }

    Write-Host '=== Building HomeBase and helper binaries ==='
    Push-Location (Join-Path $repoRoot 'src-tauri')
    try {
        & cargo build --locked --release --bins
        if ($LASTEXITCODE -ne 0) { throw 'Release helper build failed.' }
    } finally {
        Pop-Location
    }
    $stageBin = Join-Path $stageRoot 'bin'
    New-Item -ItemType Directory -Path $stageBin -Force | Out-Null
    foreach ($name in @('homebase-reveal.exe', 'homebase-mcp-image.exe', 'homebase-mcp-secrets.exe')) {
        $source = Join-Path $targetRelease $name
        if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw "Missing release helper: $source" }
        Copy-Item -LiteralPath $source -Destination (Join-Path $stageBin $name) -Force
    }

    $bundleManifestPath = Join-Path $stageRoot 'bundle-manifest.json'
    $bundleManifest = Get-Content -LiteralPath $bundleManifestPath -Raw -Encoding utf8 | ConvertFrom-Json -AsHashtable
    $bundleManifest['homebase_version'] = $version
    $bundleManifest['helper_hashes'] = [ordered]@{}
    Get-ChildItem -LiteralPath $stageBin -File | Sort-Object Name | ForEach-Object {
        $bundleManifest['helper_hashes'][$_.Name] = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
    }
    [System.IO.File]::WriteAllText($bundleManifestPath, ($bundleManifest | ConvertTo-Json -Depth 8), $utf8)

    Write-Host '=== Building NSIS installer ==='
    & npx.cmd tauri build --config src-tauri/tauri.distribution.conf.json --bundles nsis -- --locked
    if ($LASTEXITCODE -ne 0) { throw 'Tauri NSIS build failed.' }

    $installer = Get-ChildItem -LiteralPath (Join-Path $targetRelease 'bundle\nsis') -File -Filter '*setup.exe' |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1
    if (-not $installer) { throw 'NSIS installer was not generated.' }

    if (Test-Path -LiteralPath $releaseRoot) {
        Remove-Item -LiteralPath $releaseRoot -Recurse -Force
    }
    New-Item -ItemType Directory -Path $releaseRoot -Force | Out-Null
    $installerTarget = Join-Path $releaseRoot "HomeBase_${version}_x64-setup.exe"
    Copy-Item -LiteralPath $installer.FullName -Destination $installerTarget -Force

    Write-Host '=== Assembling portable package ==='
    New-Item -ItemType Directory -Path $portableRoot -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $targetRelease 'homebase.exe') -Destination (Join-Path $portableRoot 'homebase.exe') -Force
    Copy-Item -LiteralPath $stageRoot -Destination (Join-Path $portableRoot 'distribution') -Recurse -Force
    Copy-Item -LiteralPath (Join-Path $repoRoot 'packaging\windows\Start-HomeBase.cmd') -Destination $portableRoot -Force
    Copy-Item -LiteralPath (Join-Path $repoRoot 'packaging\windows\Test-HomeBasePackage.ps1') -Destination $portableRoot -Force
    Copy-Item -LiteralPath (Join-Path $repoRoot 'packaging\windows\README-安装与迁移.md') -Destination $portableRoot -Force
    Copy-Item -LiteralPath (Join-Path $repoRoot 'packaging\windows\THIRD-PARTY-NOTICES.md') -Destination $portableRoot -Force

    & pwsh -NoProfile -File (Join-Path $portableRoot 'Test-HomeBasePackage.ps1') -PackageRoot $portableRoot -Json |
        Set-Content -LiteralPath (Join-Path $releaseRoot 'PACKAGE-DOCTOR.json') -Encoding utf8NoBOM
    if ($LASTEXITCODE -ne 0) { throw 'Portable package doctor failed.' }

    $portableZip = Join-Path $releaseRoot "$portableName.zip"
    Compress-Archive -LiteralPath $portableRoot -DestinationPath $portableZip -CompressionLevel Optimal

    $releaseManifest = [ordered]@{
        schema_version = 1
        product = 'HomeBase'
        version = $version
        platform = 'windows-x64'
        built_at = (Get-Date).ToString('o')
        signed = $false
        installer = [System.IO.Path]::GetFileName($installerTarget)
        portable = [System.IO.Path]::GetFileName($portableZip)
        runtime = $bundleManifest
        validation = 'PACKAGE-DOCTOR.json'
    }
    [System.IO.File]::WriteAllText(
        (Join-Path $releaseRoot 'release-manifest.json'),
        ($releaseManifest | ConvertTo-Json -Depth 10),
        $utf8
    )
    Copy-Item -LiteralPath (Join-Path $repoRoot 'packaging\windows\README-安装与迁移.md') -Destination $releaseRoot -Force
    Copy-Item -LiteralPath (Join-Path $repoRoot 'packaging\windows\THIRD-PARTY-NOTICES.md') -Destination $releaseRoot -Force

    $hashTargets = Get-ChildItem -LiteralPath $releaseRoot -File | Where-Object { $_.Name -ne 'SHA256SUMS.txt' }
    $hashLines = $hashTargets | Sort-Object Name | ForEach-Object {
        $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        "$hash  $($_.Name)"
    }
    [System.IO.File]::WriteAllLines((Join-Path $releaseRoot 'SHA256SUMS.txt'), $hashLines, $utf8)

    Write-Host "Release ready: $releaseRoot"
} finally {
    Pop-Location
}
