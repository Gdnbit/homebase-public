param(
    [Parameter(Mandatory)][string]$SettingsPath,
    [switch]$VerifyOnly,
    [switch]$Rollback
)
$ErrorActionPreference = 'Stop'
try {
    if ($env:HOMEBASE_TEST_HOME) { throw 'LAUNCH_TEST_PROFILE_REFUSED' }
    $settingsFile = [IO.Path]::GetFullPath($SettingsPath)
    $settings = [IO.File]::ReadAllText($settingsFile) | ConvertFrom-Json
    if ($settings.schema_version -ne 1) { throw 'LAUNCH_SETTINGS_INVALID' }
    $requiredNames = @('HOMEBASE_AIHUB_ROOT','HOMEBASE_MODEL_ROOT','HOMEBASE_RUNTIME_ROOT','HOMEBASE_ASR_OUTPUT_ROOT','HOMEBASE_MODEL_CACHE','HOMEBASE_ASR_PYTHON','HOMEBASE_FFMPEG','HOMEBASE_REVEAL_EXE','HOMEBASE_SECRETS_FILE')
    $actualNames = @($settings.process_env.PSObject.Properties.Name)
    if (@(Compare-Object ($requiredNames | Sort-Object) ($actualNames | Sort-Object)).Count) { throw 'LAUNCH_ENVIRONMENT_INVALID' }
    $selected = if ($Rollback) { $settings.fallback } else { $settings.current }
    # A fallback that lacks a protected editor format must not hide its input or
    # pending identities. Only these exact, reviewed relative paths are allowed.
    if ($Rollback -and $settings.PSObject.Properties.Name -contains 'fallback_requires_absent') {
        $absent = @($settings.fallback_requires_absent)
        if (-not $absent.Count) { throw 'LAUNCH_FALLBACK_REQUIRES_REVIEW' }
        foreach ($relative in $absent) {
            if ($relative -cnotin @('.homebase/inbox', '.homebase/capabilities/development-workspace')) { throw 'LAUNCH_FALLBACK_GUARD_INVALID' }
            if (Test-Path -LiteralPath (Join-Path $env:USERPROFILE $relative)) { throw 'LAUNCH_FALLBACK_NEW_FORMAT_PRESENT' }
        }
    }
    # A legacy version may ignore newer authorization fields. Never use it after
    # the guarded state changed; do not restore an older snapshot over new data.
    if ($Rollback -and $settings.PSObject.Properties.Name -contains 'fallback_state_guard') {
        $guards = @($settings.fallback_state_guard.PSObject.Properties)
        if (-not $guards.Count) { throw 'LAUNCH_FALLBACK_REQUIRES_REVIEW' }
        foreach ($guard in $guards) {
            if ((Get-FileHash -LiteralPath $guard.Name -Algorithm SHA256).Hash -ne $guard.Value) { throw 'LAUNCH_FALLBACK_STATE_CHANGED' }
        }
    }
    $appRoot = [IO.Path]::GetFullPath([string]$selected.root)
    $allowedRoots = @('C:\HomeBase-Labs\', 'D:\HomeBase-Labs\', 'F:\HomeBase-Labs\')
    if (-not ($allowedRoots | Where-Object { $appRoot.StartsWith($_,[StringComparison]::OrdinalIgnoreCase) })) { throw 'LAUNCH_ROOT_INVALID' }
    $binary = Join-Path $appRoot 'homebase.exe'
    if ((Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash -ne $selected.binary_sha256) { throw 'LAUNCH_BINARY_MISMATCH' }
    foreach ($entry in $selected.critical_files.PSObject.Properties) {
        $target = [IO.Path]::GetFullPath((Join-Path $appRoot $entry.Name))
        if (-not $target.StartsWith($appRoot.TrimEnd('\') + '\',[StringComparison]::OrdinalIgnoreCase)) { throw 'LAUNCH_FILE_OUTSIDE_ROOT' }
        if ((Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash -ne $entry.Value) { throw 'LAUNCH_RESOURCE_MISMATCH' }
    }
    # Both versions were checked with legacy storage. Never silently migrate it.
    $history = Join-Path $env:USERPROFILE '.homebase/assistant/history-v1'
    if (Test-Path -LiteralPath $history) { throw 'LAUNCH_MIGRATED_PROFILE_REQUIRES_REVIEW' }
    if ($VerifyOnly) {
        @{ verified=$true; fallback=[bool]$Rollback; launch_performed=$false; environment_paths=@($actualNames).Count; critical_files=@($selected.critical_files.PSObject.Properties).Count } | ConvertTo-Json -Compress
        return
    }
    $running = @(Get-Process -Name homebase,homebase-rollback -ErrorAction SilentlyContinue)
    if ($running.Count) {
        if ($running.Count -eq 1 -and $running[0].Path -eq $binary) {
            $shell = New-Object -ComObject WScript.Shell
            if ($shell.AppActivate([int]$running[0].Id)) { return }
        }
        throw 'LAUNCH_APP_ALREADY_RUNNING'
    }
    foreach ($name in @('HOMEBASE_ASSISTANT_SEGMENTS','HOMEBASE_ASSISTANT_CONTEXT_SEGMENTS','HOMEBASE_ASSISTANT_TASK_ARCHIVE','HOMEBASE_ASSISTANT_CAPACITY','HOMEBASE_OPEN_DEVTOOLS','HOMEBASE_DOCUMENT_NATIVE_LAB','HOMEBASE_ENABLE_WECHAT_SELFTEST','WEBVIEW2_USER_DATA_FOLDER')) {
        Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue
    }
    foreach ($entry in $settings.process_env.PSObject.Properties) {
        [Environment]::SetEnvironmentVariable($entry.Name,[string]$entry.Value,'Process')
    }
    [Environment]::SetEnvironmentVariable('HOMEBASE_CONTROL_PROJECT_ROOT',$appRoot,'Process')
    $started = Start-Process -FilePath $binary -WorkingDirectory $appRoot -WindowStyle Hidden -PassThru
    @{ started=$true; pid=$started.Id; fallback=[bool]$Rollback; binary_sha256=$selected.binary_sha256 } | ConvertTo-Json -Compress
} catch {
    # No exception text or source line: a malformed path may contain private data.
    @{ started=$false; verified=$false; code='LOCAL_LAUNCH_REFUSED'; exception_type=$_.Exception.GetType().Name } | ConvertTo-Json -Compress
    exit 1
}
