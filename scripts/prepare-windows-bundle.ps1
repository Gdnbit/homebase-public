[CmdletBinding()]
param(
    [switch]$ForceDownload
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$resourcesBase = [System.IO.Path]::GetFullPath((Join-Path $repoRoot 'src-tauri\resources'))
$stageRoot = [System.IO.Path]::GetFullPath((Join-Path $resourcesBase 'distribution'))
$cacheRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot '.tmp\bundle-cache'))
$utf8 = [System.Text.UTF8Encoding]::new($false)

$bubblewrapPath = $env:HOMEBASE_BWRAP_SOURCE
if (-not $bubblewrapPath) { $bubblewrapPath = Join-Path $repoRoot 'agent-runtime\tools\bwrap' }
if (-not (Test-Path -LiteralPath $bubblewrapPath -PathType Leaf)) {
    throw 'Provide the pinned WSL bubblewrap executable using HOMEBASE_BWRAP_SOURCE. See agent-runtime/tools/NOTICE.md.'
}

if (-not $stageRoot.StartsWith($resourcesBase + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to replace staging directory outside $resourcesBase"
}
if (Test-Path -LiteralPath $stageRoot) {
    Remove-Item -LiteralPath $stageRoot -Recurse -Force
}
New-Item -ItemType Directory -Path $stageRoot, $cacheRoot -Force | Out-Null

function Assert-FileHash {
    param([string]$Path, [string]$Expected)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "Missing required file: $Path" }
    $actual = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash
    if ($actual -ne $Expected) { throw "SHA-256 mismatch for $Path. Expected $Expected, got $actual" }
}

function Copy-RequiredFile {
    param([string]$Source, [string]$RelativeTarget)
    if (-not (Test-Path -LiteralPath $Source -PathType Leaf)) { throw "Missing source file: $Source" }
    $target = Join-Path $stageRoot $RelativeTarget
    New-Item -ItemType Directory -Path (Split-Path -Parent $target) -Force | Out-Null
    Copy-Item -LiteralPath $Source -Destination $target -Force
}

function Get-VerifiedDownload {
    param([string]$Url, [string]$Target, [string]$Sha256)
    if ($ForceDownload -and (Test-Path -LiteralPath $Target)) { Remove-Item -LiteralPath $Target -Force }
    if (-not (Test-Path -LiteralPath $Target -PathType Leaf)) {
        Write-Host "Downloading $Url"
        Invoke-WebRequest -Uri $Url -OutFile $Target
    }
    Assert-FileHash -Path $Target -Expected $Sha256
}

Write-Host 'Preparing CPython 3.13.14 embeddable runtime...'
$pythonArchive = Join-Path $cacheRoot 'python-3.13.14-embed-amd64.zip'
Get-VerifiedDownload `
    -Url 'https://www.python.org/ftp/python/3.13.14/python-3.13.14-embed-amd64.zip' `
    -Target $pythonArchive `
    -Sha256 '90B4E5B9898B72D744650524BFF92377C367F44BD5FBD09E3148656C080AD907'
$pythonRoot = Join-Path $stageRoot 'runtime\python'
New-Item -ItemType Directory -Path $pythonRoot -Force | Out-Null
Expand-Archive -LiteralPath $pythonArchive -DestinationPath $pythonRoot -Force
$pthPath = Join-Path $pythonRoot 'python313._pth'
[System.IO.File]::WriteAllLines(
    $pthPath,
    @('python313.zip', '.', 'Lib\site-packages', 'import site'),
    $utf8
)

$builderPython = if (Test-Path -LiteralPath 'C:\HomeBase\Tools\Python313\python.exe') {
    'C:\HomeBase\Tools\Python313\python.exe'
} else {
    (Get-Command python -ErrorAction Stop).Source
}
$requirements = Join-Path $repoRoot 'packaging\windows\runtime-requirements.txt'
$sitePackages = Join-Path $pythonRoot 'Lib\site-packages'
New-Item -ItemType Directory -Path $sitePackages -Force | Out-Null
Write-Host 'Installing pinned cloud and faster-whisper runtime packages...'
& $builderPython -m pip install `
    --disable-pip-version-check `
    --ignore-installed `
    --no-compile `
    --only-binary=:all: `
    --upgrade `
    --target $sitePackages `
    -r $requirements
if ($LASTEXITCODE -ne 0) { throw 'Pinned Python runtime installation failed.' }
$packageList = & $builderPython -m pip list --path $sitePackages --format freeze --disable-pip-version-check
if ($LASTEXITCODE -ne 0) { throw 'Could not inventory bundled Python packages.' }
[System.IO.File]::WriteAllLines((Join-Path $stageRoot 'python-packages.txt'), $packageList, $utf8)

Write-Host 'Copying AIHub ASR, model manager, MCP, catalog, and preset resources...'
$aihub = 'C:\HomeBase\AIHub'
$runnerSource = Join-Path $aihub 'tools\local-asr\runner'
Get-ChildItem -LiteralPath $runnerSource -File -Filter '*.py' | ForEach-Object {
    Copy-RequiredFile $_.FullName ("aihub\tools\local-asr\runner\" + $_.Name)
}
Copy-RequiredFile (Join-Path $aihub 'tools\local-asr\README.md') 'aihub\tools\local-asr\README.md'
Copy-RequiredFile (Join-Path $aihub 'tools\model-manager\model_manager.py') 'aihub\tools\model-manager\model_manager.py'
Copy-RequiredFile (Join-Path $aihub 'tools\model-manager\README.md') 'aihub\tools\model-manager\README.md'
Copy-RequiredFile (Join-Path $aihub 'capabilities\asr-presets.json') 'aihub\capabilities\asr-presets.json'
Copy-RequiredFile (Join-Path $aihub 'capabilities\model-download-catalog.json') 'aihub\capabilities\model-download-catalog.json'
Copy-RequiredFile (Join-Path $aihub 'mcp\servers\local-asr\server.py') 'aihub\mcp\servers\local-asr\server.py'
Copy-RequiredFile (Join-Path $aihub 'mcp\servers\local-asr\README.md') 'aihub\mcp\servers\local-asr\README.md'
Copy-RequiredFile (Join-Path $aihub 'mcp\servers.d\local-asr.json') 'aihub\mcp\servers.d\local-asr.json'

Write-Host 'Copying pinned native tools...'
$ffmpegSource = 'C:\HomeBase\Runtimes\ffmpeg\bin\ffmpeg.exe'
$sopsSource = 'C:\HomeBase\Tools\sops\bin\sops.exe'
$ageSource = 'C:\HomeBase\Tools\age\age-keygen.exe'
Assert-FileHash $ffmpegSource '228D7A8556258DE907FDB55F36850078EBC7680B84EC30D84EA02E99BEC1D1EB'
Assert-FileHash $sopsSource '5E777B1854AB2A6271D8F66375970E1FE3EEA838251C309DE151D16A2BDF13A2'
Assert-FileHash $ageSource '8B9C27EF2AB6F215F689BF1E609BF82C8FAF4C041F32452FA80396B3F8C4F687'
Copy-RequiredFile $ffmpegSource 'tools\ffmpeg.exe'
Copy-RequiredFile $sopsSource 'tools\sops.exe'
Copy-RequiredFile $ageSource 'tools\age-keygen.exe'

$licenses = Join-Path $stageRoot 'licenses'
New-Item -ItemType Directory -Path $licenses -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $pythonRoot 'LICENSE.txt') -Destination (Join-Path $licenses 'CPython-LICENSE.txt') -Force
Copy-Item -LiteralPath 'C:\HomeBase\Runtimes\ffmpeg-extract\ffmpeg-8.1.1-essentials_build\LICENSE' -Destination (Join-Path $licenses 'FFmpeg-build-LICENSE.txt') -Force
Invoke-WebRequest -Uri 'https://raw.githubusercontent.com/getsops/sops/v3.12.2/LICENSE' -OutFile (Join-Path $licenses 'SOPS-LICENSE.txt')
Invoke-WebRequest -Uri 'https://raw.githubusercontent.com/FiloSottile/age/v1.3.1/LICENSE' -OutFile (Join-Path $licenses 'age-LICENSE.txt')
Copy-RequiredFile (Join-Path $repoRoot 'packaging\windows\THIRD-PARTY-NOTICES.md') 'THIRD-PARTY-NOTICES.md'
Copy-RequiredFile (Join-Path $repoRoot 'packaging\windows\README-安装与迁移.md') 'README-安装与迁移.md'

$embeddedPython = Join-Path $pythonRoot 'python.exe'
$runtimeProbe = & $embeddedPython -c "import av, ctranslate2, faster_whisper, huggingface_hub, requests; print('runtime-ok')" 2>&1
if ($LASTEXITCODE -ne 0 -or "$runtimeProbe" -notmatch 'runtime-ok') {
    throw "Embedded Python import probe failed: $runtimeProbe"
}
$runner = Join-Path $stageRoot 'aihub\tools\local-asr\runner\asr_task_runner.py'
$runnerHelp = & $embeddedPython $runner --help 2>&1
if ($LASTEXITCODE -ne 0 -or "$runnerHelp" -notmatch 'preflight') {
    throw "Packaged ASR runner probe failed: $runnerHelp"
}

Write-Host 'Preparing the pinned local agent runtime without install scripts...'
$nativeAgentStage = Join-Path $stageRoot 'native-agent-runtime'
& node (Join-Path $repoRoot 'scripts/package-native-agents.mjs') $nativeAgentStage
if ($LASTEXITCODE -ne 0) { throw 'Pinned native agent runtime packaging failed; run npm ci --ignore-scripts in native-agent-runtime first.' }
foreach ($pipelineFile in @('development-capability.mjs','repo-capabilities.mjs','repo-capabilities-store.mjs','repo-capabilities-analysis.mjs','repo-capabilities-process.mjs','repo-capabilities-sandbox.py','control-validation.mjs')) {
    Copy-RequiredFile (Join-Path $repoRoot "scripts\$pipelineFile") (Join-Path 'capability-pipeline' $pipelineFile)
}
$agentSource = Join-Path $repoRoot 'agent-runtime'
$agentStage = Join-Path $stageRoot 'agent-runtime'
foreach ($agentFile in @('pi-worker.mjs', 'package.json', 'package-lock.json')) {
    Copy-RequiredFile (Join-Path $agentSource $agentFile) (Join-Path 'agent-runtime' $agentFile)
}
foreach ($agentDir in @('local', 'tools')) {
    Get-ChildItem -LiteralPath (Join-Path $agentSource $agentDir) -File -Recurse | ForEach-Object {
        $agentRelative = [System.IO.Path]::GetRelativePath($agentSource, $_.FullName)
        if ($agentRelative -ne 'tools\bwrap') {
            Copy-RequiredFile $_.FullName (Join-Path 'agent-runtime' $agentRelative)
        }
    }
}
Assert-FileHash -Path $bubblewrapPath -Expected 'e318903862396f96de3df57264e0158682b952fd3fb53ac23d876413e7b30f71'
Copy-RequiredFile $bubblewrapPath 'agent-runtime\tools\bwrap'
$agentNode = (Get-Command node -ErrorAction Stop).Source
$agentNodeVersion = (& $agentNode --version).Trim()
if ([version]$agentNodeVersion.TrimStart('v') -lt [version]'22.19.0') { throw 'Agent runtime requires Node >=22.19.0.' }
Copy-RequiredFile $agentNode 'agent-runtime\node.exe'
Push-Location $agentStage
try {
    & npm.cmd ci --ignore-scripts --omit=dev --no-audit --no-fund
    if ($LASTEXITCODE -ne 0) { throw 'Agent runtime dependency installation failed.' }
    & $agentNode --input-type=module -e "await import('@earendil-works/pi-agent-core'); await import('silk-wasm'); console.log('agent-runtime-ok')"
    if ($LASTEXITCODE -ne 0) { throw 'Packaged agent runtime import failed.' }
} finally { Pop-Location }
Copy-RequiredFile (Join-Path $agentStage 'node_modules/silk-wasm/LICENSE') 'licenses/silk-wasm-LICENSE'

$manifest = [ordered]@{
    schema_version = 1
    package = 'HomeBase Windows x64'
    generated_at = (Get-Date).ToString('o')
    signed = $false
    model_weights_included = $false
    secrets_included = $false
    components = [ordered]@{
        python = '3.13.14'
        faster_whisper = '1.2.1'
        ffmpeg = '8.1.1-essentials_build'
        sops = '3.12.2'
        age = '1.3.1'
        agent_node = $agentNodeVersion
        agent_node_sha256 = (Get-FileHash -LiteralPath $agentNode -Algorithm SHA256).Hash
        pi_agent_core = '1.6.0'
        pi_ai = '1.6.0'
        silk_wasm = '3.7.1'
    }
    source_roots = [ordered]@{
        aihub = 'bundled snapshot; copied to per-user data on first run'
        models = 'target-machine download'
        output = 'target-machine Wiki root'
        secrets = 'target-machine SOPS store'
    }
}
[System.IO.File]::WriteAllText(
    (Join-Path $stageRoot 'bundle-manifest.json'),
    ($manifest | ConvertTo-Json -Depth 6),
    $utf8
)

$size = (Get-ChildItem -LiteralPath $stageRoot -Recurse -File | Measure-Object Length -Sum).Sum
Write-Host ("Bundle resources ready: {0:N1} MB at {1}" -f ($size / 1MB), $stageRoot)
