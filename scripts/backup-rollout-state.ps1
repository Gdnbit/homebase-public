# Maintenance backup only. Never decrypts existing DPAPI, launches or restores.
param(
  [Parameter(Mandatory = $true)][string]$ProfileRoot,
  [Parameter(Mandatory = $true)][string]$Destination,
  [switch]$ProgressEvents,
  [switch]$TestVerificationGate
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$script:Code = 'BACKUP_INVALID_INPUT'
$script:CreatedBackup = $false
$script:PrivateAclVerified = $false
function Fail([string]$Code) { $script:Code = $Code; throw 'Redacted backup failure' }
function Safe-Path([string]$File) {
  $current = [IO.Path]::GetFullPath($File)
  while ($current) {
    if (Test-Path -LiteralPath $current) {
      if (([IO.File]::GetAttributes($current) -band [IO.FileAttributes]::ReparsePoint) -ne 0) { Fail 'BACKUP_LINKED_PATH' }
    }
    $parent = [IO.Path]::GetDirectoryName($current)
    if ($parent -eq $current) { break }; $current = $parent
  }
}
function Inventory([string]$Root) {
  $items = [Collections.Generic.List[object]]::new()
  foreach ($entry in @(
    @('assistant/state.dpapi', 32MB, 'ciphertext', $true),
    @('wechat/channel.dpapi', 32MB, 'ciphertext', $false),
    @('assistant/model-routes.json', 2MB, 'protect_metadata', $false),
    @('control/operations.jsonl', 32MB, 'protect_metadata', $false)
  )) {
    $file = Join-Path $Root $entry[0]; Safe-Path $file
    if ([IO.Directory]::Exists($file)) { Fail 'BACKUP_INVALID_SOURCE' }
    if ([IO.File]::Exists($file)) { $items.Add(@{relative=$entry[0];limit=[long]$entry[1];mode=$entry[2]}) }
    elseif ($entry[3]) { Fail 'BACKUP_REQUIRED_STATE_MISSING' }
  }
  foreach ($relative in @('assistant/desktop', 'assistant/desktop/completed-handoffs')) {
    $directory = Join-Path $Root $relative; Safe-Path $directory
    if ([IO.File]::Exists($directory)) { Fail 'BACKUP_INVALID_SOURCE' }
    if (-not [IO.Directory]::Exists($directory)) { continue }
    foreach ($file in [IO.Directory]::GetFiles($directory, '*', [IO.SearchOption]::TopDirectoryOnly)) {
      Safe-Path $file
      $name = [IO.Path]::GetFileName($file)
      # The desktop store keeps this empty file after releasing its write lock.
      if ($relative -ceq 'assistant/desktop' -and $name -ceq 'workspace.lock' -and [IO.FileInfo]::new($file).Length -eq 0) { continue }
      if ($name -cnotmatch '^[a-f0-9]{64}\.draft\.dpapi$') { Fail 'BACKUP_UNKNOWN_DESKTOP_FILE' }
      $items.Add(@{relative="$relative/$name";limit=32MB;mode='ciphertext'})
      if ($items.Count -gt 4096) { Fail 'BACKUP_SIZE_LIMIT' }
    }
    foreach ($child in [IO.Directory]::GetDirectories($directory)) {
      Safe-Path $child
      if ($relative -ne 'assistant/desktop' -or [IO.Path]::GetFileName($child) -cne 'completed-handoffs') { Fail 'BACKUP_UNKNOWN_DESKTOP_DIRECTORY' }
    }
  }
  foreach ($relative in @('assistant/history-v1', 'assistant/documents')) {
    $directory = Join-Path $Root $relative; Safe-Path $directory
    if (Test-Path -LiteralPath $directory) { Fail 'BACKUP_STORAGE_COVERAGE_UNVERIFIED' }
  }
  # Inbox source and receipts have independent protected identities.
  foreach ($relative in @('inbox', 'inbox/receipts')) {
    $directory = Join-Path $Root $relative; Safe-Path $directory
    if ([IO.File]::Exists($directory)) { Fail 'BACKUP_INVALID_SOURCE' }
    if (-not [IO.Directory]::Exists($directory)) { continue }
    foreach ($file in [IO.Directory]::GetFiles($directory, '*', [IO.SearchOption]::TopDirectoryOnly)) {
      Safe-Path $file; $name = [IO.Path]::GetFileName($file)
      if (($relative -ceq 'inbox' -and $name -ceq 'source.writer.lock') -or
          ($relative -ceq 'inbox/receipts' -and $name -ceq 'writer.lock')) { continue }
      $recognized = if ($relative -ceq 'inbox') { $name -ceq 'source.workspace.dpapi' }
        else { $name -cmatch '^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}\.receipt\.dpapi$' }
      if (-not $recognized) { Fail 'BACKUP_UNKNOWN_INBOX_FILE' }
      $items.Add(@{relative="$relative/$name";limit=32MB;mode='ciphertext'})
      if ($items.Count -gt 14500) { Fail 'BACKUP_SIZE_LIMIT' }
    }
    foreach ($child in [IO.Directory]::GetDirectories($directory)) {
      Safe-Path $child
      if ($relative -cne 'inbox' -or [IO.Path]::GetFileName($child) -cne 'receipts') { Fail 'BACKUP_UNKNOWN_INBOX_DIRECTORY' }
    }
  }
  $relative = 'capabilities/development-workspace'
  $directory = Join-Path $Root $relative; Safe-Path $directory
  if ([IO.File]::Exists($directory)) { Fail 'BACKUP_INVALID_SOURCE' }
  if ([IO.Directory]::Exists($directory)) {
    foreach ($file in [IO.Directory]::GetFiles($directory, '*', [IO.SearchOption]::TopDirectoryOnly)) {
      Safe-Path $file; $name = [IO.Path]::GetFileName($file)
      if ($name -ceq 'editor.writer.lock') { continue }
      if ($name -cne 'editor.workspace.dpapi') { Fail 'BACKUP_UNKNOWN_DEVELOPMENT_FILE' }
      $items.Add(@{relative="$relative/$name";limit=8MB;mode='ciphertext'})
    }
    foreach ($child in [IO.Directory]::GetDirectories($directory)) { Safe-Path $child; Fail 'BACKUP_UNKNOWN_DEVELOPMENT_DIRECTORY' }
  }
  return @($items | Sort-Object { $_.relative })
}
function Read-Bounded([string]$File, [long]$Limit, [switch]$AllowEmpty) {
  Safe-Path $File
  $stream = $null; $buffer = $null
  try {
    $stream = [IO.FileStream]::new($File, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    if (($stream.Length -eq 0 -and -not $AllowEmpty) -or $stream.Length -gt $Limit) { Fail 'BACKUP_SIZE_LIMIT' }
    $buffer = [byte[]]::new([int]$stream.Length)
    $stream.ReadExactly($buffer, 0, $buffer.Length)
    if ($stream.Length -ne $buffer.Length) { Fail 'BACKUP_SOURCE_CHANGED' }
    return ,$buffer
  } catch {
    if ($null -ne $buffer) { [Array]::Clear($buffer, 0, $buffer.Length) }
    throw
  } finally { if ($null -ne $stream) { $stream.Dispose() } }
}
function Digest([byte[]]$Bytes) { return [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($Bytes)).ToLowerInvariant() }
function Verify-Cipher-Header([byte[]]$Bytes) {
  # Standard Windows DPAPI provider header; no credential unprotection occurs.
  $header = [Convert]::FromHexString('01000000d08c9ddf0115d1118c7a00c04fc297eb')
  if ($Bytes.Length -lt $header.Length) { Fail 'BACKUP_NOT_DPAPI_CIPHERTEXT' }
  for ($i=0; $i -lt $header.Length; $i++) { if ($Bytes[$i] -ne $header[$i]) { Fail 'BACKUP_NOT_DPAPI_CIPHERTEXT' } }
}
function Write-New([string]$File, [byte[]]$Bytes) {
  Safe-Path $File
  $stream = [IO.FileStream]::new($File, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
  try { $stream.Write($Bytes, 0, $Bytes.Length); $stream.Flush($true) } finally { $stream.Dispose() }
}
try {
  if (-not $IsWindows -or $PSVersionTable.PSVersion -lt [version]'7.4') { Fail 'BACKUP_INVALID_INPUT' }
  foreach ($value in @($ProfileRoot, $Destination)) {
    if ($value -notmatch '^[A-Za-z]:[\\/]' -or -not [IO.Path]::IsPathFullyQualified($value)) { Fail 'BACKUP_INVALID_INPUT' }
    Safe-Path $value
  }
  $sourceProfilePath = [IO.Path]::GetFullPath($ProfileRoot).TrimEnd('\','/')
  $destinationPath = [IO.Path]::GetFullPath($Destination).TrimEnd('\','/')
  if (-not $destinationPath.Replace('\','/').StartsWith('F:/HomeBase-Labs/', [StringComparison]::OrdinalIgnoreCase)) { Fail 'BACKUP_DESTINATION_SCOPE' }
  if ($destinationPath.StartsWith($sourceProfilePath+'\', [StringComparison]::OrdinalIgnoreCase) -or
      $sourceProfilePath.StartsWith($destinationPath+'\', [StringComparison]::OrdinalIgnoreCase) -or
      $sourceProfilePath.Equals($destinationPath, [StringComparison]::OrdinalIgnoreCase)) { Fail 'BACKUP_SOURCE_DESTINATION_OVERLAP' }
  if ($TestVerificationGate -and (-not $ProgressEvents -or
      -not $sourceProfilePath.Replace('\','/').StartsWith('F:/HomeBase-Labs/', [StringComparison]::OrdinalIgnoreCase) -or
      -not $sourceProfilePath.Equals($env:HOMEBASE_TEST_HOME, [StringComparison]::OrdinalIgnoreCase))) { Fail 'BACKUP_TEST_GATE_REJECTED' }
  if (Test-Path -LiteralPath $destinationPath) { Fail 'BACKUP_DESTINATION_EXISTS' }
  $parent = [IO.Path]::GetDirectoryName($destinationPath)
  if (-not [IO.Directory]::Exists($parent)) { Fail 'BACKUP_PARENT_MISSING' }
  $root = Join-Path $sourceProfilePath '.homebase'
  $before = @(Inventory $root)
  $script:Code = 'BACKUP_IO'
  Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class HomeBaseBackupDirectory {
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true, EntryPoint="CreateDirectoryW")]
  [return: MarshalAs(UnmanagedType.Bool)]
  public static extern bool Create(string path, IntPtr security);
}
'@
  $nativeDestination = '\\?\'+$destinationPath
  if (-not [HomeBaseBackupDirectory]::Create($nativeDestination, [IntPtr]::Zero)) { Fail 'BACKUP_DESTINATION_CREATE_FAILED' }
  $script:CreatedBackup = $true
  Safe-Path $destinationPath
  $script:Code = 'BACKUP_PRIVATE_ACL'
  $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User
  $security = [Security.AccessControl.DirectorySecurity]::new()
  $security.SetAccessRuleProtection($true, $false)
  $rule = [Security.AccessControl.FileSystemAccessRule]::new($sid, 'FullControl', 'ContainerInherit,ObjectInherit', 'None', 'Allow')
  $security.AddAccessRule($rule)
  [IO.FileSystemAclExtensions]::SetAccessControl([IO.DirectoryInfo]::new($destinationPath), $security)
  $actual = [IO.FileSystemAclExtensions]::GetAccessControl([IO.DirectoryInfo]::new($destinationPath))
  $rules = @($actual.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier]))
  if ($actual.GetOwner([Security.Principal.SecurityIdentifier]).Value -cne $sid.Value -or
      -not $actual.AreAccessRulesProtected -or $rules.Count -ne 1 -or $rules[0].IdentityReference.Value -cne $sid.Value -or
      $rules[0].AccessControlType -ne 'Allow' -or $rules[0].FileSystemRights -ne 'FullControl') { Fail 'BACKUP_PRIVATE_ACL_UNVERIFIED' }
  $script:PrivateAclVerified = $true
  if ($ProgressEvents) { [Console]::Error.WriteLine('backup:private-directory-ready') }
  $files = [Collections.Generic.List[object]]::new(); $total = 0L; $index = 0
  foreach ($entry in $before) {
    $source = Join-Path $root $entry.relative
    $bytes = $null; $protected = $null
    try {
      $script:Code = 'BACKUP_SOURCE_READ'
      $bytes = Read-Bounded $source $entry.limit -AllowEmpty:($entry.relative -ceq 'control/operations.jsonl')
      $total += $bytes.Length
      if ($total -gt 256MB) { Fail 'BACKUP_SIZE_LIMIT' }
      $sourceHash = Digest $bytes
      if ($entry.mode -eq 'ciphertext') { Verify-Cipher-Header $bytes; $protected = $bytes }
      else { $protected = [Security.Cryptography.ProtectedData]::Protect($bytes, $null, [Security.Cryptography.DataProtectionScope]::CurrentUser) }
      $name = '{0:D4}.dpapi' -f $index; $index++
      $target = Join-Path $destinationPath $name
      $script:Code = 'BACKUP_WRITE'
      Write-New $target $protected
      $checked = Read-Bounded $target ($entry.limit + 1MB)
      try { if ((Digest $checked) -cne (Digest $protected)) { Fail 'BACKUP_COPY_VERIFY_FAILED' } } finally { [Array]::Clear($checked,0,$checked.Length) }
      $files.Add(@{source_relative=$entry.relative;backup=$name;source_sha256=$sourceHash;backup_sha256=(Digest $protected);source_bytes=$bytes.Length;mode=$entry.mode})
    } finally {
      if ($null -ne $bytes) { [Array]::Clear($bytes,0,$bytes.Length) }
      if ($null -ne $protected) { [Array]::Clear($protected,0,$protected.Length) }
    }
  }
  if ($ProgressEvents) { [Console]::Error.WriteLine('backup:copies-verified') }
  if ($TestVerificationGate) {
    [Console]::Error.WriteLine('backup:test-verification-gate')
    if ([Console]::ReadLine() -cne 'continue') { Fail 'BACKUP_TEST_GATE_NOT_RELEASED' }
  }
  $after = @(Inventory $root)
  if (($before.relative -join "`n") -cne ($after.relative -join "`n")) { Fail 'BACKUP_INVENTORY_CHANGED' }
  foreach ($entry in $files) {
    $limit = ($before | Where-Object { $_.relative -ceq $entry.source_relative }).limit
    $bytes = Read-Bounded (Join-Path $root $entry.source_relative) $limit -AllowEmpty:($entry.source_relative -ceq 'control/operations.jsonl')
    try { if ((Digest $bytes) -cne $entry.source_sha256) { Fail 'BACKUP_SOURCE_CHANGED' } } finally { [Array]::Clear($bytes,0,$bytes.Length) }
  }
  $manifest = @{schema_version=1;created_at=[DateTimeOffset]::UtcNow.ToString('o');profile_root=$sourceProfilePath;owner_sid=$sid.Value;files=@($files.ToArray());source_consistent=$true;plaintext_credentials_saved=$false;existing_credentials_decrypted=$false;safe_to_update=$false;automatic_restore_supported=$false;excluded=@('runtime_auth_endpoints','external_secret_stores','workspace_artifacts_and_repositories','shared_configurations_and_agent_accounts')}
  $manifestBytes = [Text.Encoding]::UTF8.GetBytes((ConvertTo-Json -InputObject $manifest -Depth 10 -Compress))
  Write-New (Join-Path $destinationPath 'manifest.json') $manifestBytes
  @{schema_version=1;code='BACKUP_VERIFIED';backup_committed=$true;files=$files.Count;source_bytes=$total;private_acl_verified=$true;source_consistent=$true;existing_credentials_decrypted=$false;plaintext_credentials_saved=$false;source_writes_performed=$false;safe_to_update=$false;automatic_restore_supported=$false} | ConvertTo-Json -Compress
  exit 0
} catch {
  @{schema_version=1;code=$script:Code;backup_committed=$false;source_writes_performed=$false;safe_to_update=$false;partial_backup_retained=$script:CreatedBackup;private_acl_verified=$script:PrivateAclVerified;exception_type=$_.Exception.GetType().FullName;line=$_.InvocationInfo.ScriptLineNumber} | ConvertTo-Json -Compress
  exit 1
}
