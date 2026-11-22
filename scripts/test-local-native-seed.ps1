param([Parameter(Mandatory)][string]$Lab)
$ErrorActionPreference = 'Stop'
$root = [System.IO.Path]::GetFullPath($Lab)
if (-not $root.StartsWith('F:\HomeBase-Labs\', [System.StringComparison]::OrdinalIgnoreCase)) { throw 'Only the explicit isolated lab is accepted.' }
$seed = Join-Path $root 'seed-state.json'
$target = Join-Path $root '.homebase\assistant\state.dpapi'
if (Test-Path -LiteralPath $target) { throw 'Refuse to overwrite any existing encrypted state.' }
$bytes = [System.IO.File]::ReadAllBytes($seed)
$parsed = [System.Text.Encoding]::UTF8.GetString($bytes) | ConvertFrom-Json
if ($parsed.settings.enabled -or $parsed.sessions.Count -ne 0 -or $parsed.local.revision -ne 0) { throw 'Not a disabled synthetic initial state.' }
Add-Type -AssemblyName System.Security
$cipher = [System.Security.Cryptography.ProtectedData]::Protect($bytes, $null, [System.Security.Cryptography.DataProtectionScope]::CurrentUser)
[System.IO.Directory]::CreateDirectory([System.IO.Path]::GetDirectoryName($target)) | Out-Null
[System.IO.File]::WriteAllBytes($target, $cipher)
$fixture = Join-Path $root 'synthetic-key-manager.json'
if (Test-Path -LiteralPath $fixture) {
    $data = [System.Text.Encoding]::UTF8.GetString([System.IO.File]::ReadAllBytes($fixture)) | ConvertFrom-Json
    if ($data.value -isnot [string] -or -not $data.value.StartsWith('SYNTHETIC-') -or $data.value.Length -gt 128) { throw 'Only synthetic fixture text is accepted.' }
    $keyRoot = Join-Path $root '.homebase\service-credentials'
    $keyFile = Join-Path $keyRoot 'providers\openrouter-key.dpapi'
    if (Test-Path -LiteralPath $keyFile) { throw 'Refuse to overwrite an existing credential file.' }
    [System.IO.Directory]::CreateDirectory([System.IO.Path]::GetDirectoryName($keyFile)) | Out-Null
    $raw = [System.Text.Encoding]::Unicode.GetBytes($data.value)
    $protected = [System.Security.Cryptography.ProtectedData]::Protect($raw, $null, [System.Security.Cryptography.DataProtectionScope]::CurrentUser)
    [System.Array]::Clear($raw, 0, $raw.Length)
    # Existing API-Key-Manager uses ConvertFrom-SecureString's UTF-8 hex envelope.
    [System.IO.File]::WriteAllText($keyFile, [System.Convert]::ToHexString($protected).ToLowerInvariant(), [System.Text.UTF8Encoding]::new($false))
    $registry = @{ version = 1; providers = @(@{ id = 'openrouter'; file = $keyFile }) } | ConvertTo-Json -Depth 5 -Compress
    [System.IO.File]::WriteAllText((Join-Path $keyRoot 'registry.json'), $registry, [System.Text.UTF8Encoding]::new($false))
}
