# All contents and credentials below are synthetic; no real profile is opened.
param([Parameter(Mandatory = $true)][string]$Lab)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$labPath = [IO.Path]::GetFullPath($Lab)
if (-not $IsWindows -or -not $labPath.Replace('\','/').StartsWith('F:/HomeBase-Labs/', [StringComparison]::OrdinalIgnoreCase) -or
    (Test-Path -LiteralPath $labPath)) { throw 'A new F drive laboratory is required' }
[IO.Directory]::CreateDirectory($labPath) | Out-Null
$scriptPath = Join-Path $PSScriptRoot 'backup-rollout-state.ps1'
$canary = 'SYNTHETIC-BACKUP-PRIVATE-测试🙂-DO-NOT-EMIT'
$results = [Collections.Generic.List[object]]::new()
function Protect([string]$File) {
  [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($File)) | Out-Null
  $plain = [Text.Encoding]::UTF8.GetBytes('{"synthetic":"'+$canary+'"}')
  try {
    $cipher = [Security.Cryptography.ProtectedData]::Protect($plain,$null,[Security.Cryptography.DataProtectionScope]::CurrentUser)
    [IO.File]::WriteAllBytes($File,$cipher)
  } finally { [Array]::Clear($plain,0,$plain.Length) }
}
function Hashes([string]$Root) {
  $map = [ordered]@{}
  foreach ($file in @([IO.Directory]::GetFiles($Root,'*',[IO.SearchOption]::AllDirectories) | Sort-Object)) {
    $map[[IO.Path]::GetRelativePath($Root,$file)] = (Get-FileHash -LiteralPath $file).Hash
  }
  return ConvertTo-Json -InputObject $map -Compress
}
function Check([bool]$Value) { if (-not $Value) { throw ('Synthetic backup assertion failed at line '+$MyInvocation.ScriptLineNumber) } }
function Run([string]$Name,[string]$Expected,[scriptblock]$Arrange={},[string]$Mutation='') {
  $casePath = Join-Path $labPath $Name
  $sourcePath = Join-Path $casePath 'source'
  $destinationPath = Join-Path $casePath 'backup'
  $root = Join-Path $sourcePath '.homebase'
  [IO.Directory]::CreateDirectory((Join-Path $root 'assistant')) | Out-Null
  Protect (Join-Path $root 'assistant/state.dpapi')
  Protect (Join-Path $root 'wechat/channel.dpapi')
  [IO.File]::WriteAllText((Join-Path $root 'assistant/model-routes.json'),'{"synthetic":"'+$canary+'"}',[Text.UTF8Encoding]::new($false))
  [IO.Directory]::CreateDirectory((Join-Path $root 'shared')) | Out-Null
  [IO.File]::WriteAllText((Join-Path $root 'shared/.ipc-secrets.json'),$canary)
  $held = $null
  & $Arrange $sourcePath $destinationPath $root
  if ($Name -eq 'outside-destination') { $destinationPath='C:/Not-A-HomeBase-Backup/never-created';Check (-not (Test-Path -LiteralPath $destinationPath)) }
  if ($Name -eq 'overlap-destination') { $destinationPath=Join-Path $sourcePath 'backup' }
  if ($Name -eq 'linked-root') { $alias=Join-Path $casePath 'source-alias'; New-Item -ItemType Junction -Path $alias -Target $sourcePath | Out-Null; $sourcePath=$alias }
  if ($Name -eq 'long-unicode') {
    $deep=Join-Path $casePath (('中文🙂segment-'*8))
    [IO.Directory]::CreateDirectory($deep) | Out-Null
    $deep=Join-Path $deep (('深层segment-'*8));[IO.Directory]::CreateDirectory($deep)|Out-Null
    $destinationPath=Join-Path $deep 'backup'
  }
  if ($Name -eq 'missing-parent') { $destinationPath=Join-Path $casePath 'missing/backup' }
  if ($Name -eq 'linked-parent') {
    $physical=Join-Path $casePath 'physical';[IO.Directory]::CreateDirectory($physical)|Out-Null
    $alias=Join-Path $casePath 'alias';New-Item -ItemType Junction -Path $alias -Target $physical|Out-Null
    $destinationPath=Join-Path $alias 'backup'
  }
  $before = Hashes $root
  if ($Name -eq 'locked-source') { $held=[IO.FileStream]::new((Join-Path $root 'assistant/state.dpapi'),'Open','ReadWrite','None') }
  $destinationBefore = if ([IO.Directory]::Exists($destinationPath)) { Hashes $destinationPath } else { '' }
  $aclBefore = if ([IO.Directory]::Exists($destinationPath)) { (Get-Acl -LiteralPath $destinationPath).Sddl } else { '' }
  $info=[Diagnostics.ProcessStartInfo]::new((Join-Path $PSHOME 'pwsh.exe'))
  $info.UseShellExecute=$false;$info.CreateNoWindow=$true
  $info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true;$info.RedirectStandardInput=$true
  foreach($arg in @('-NoLogo','-NoProfile','-NonInteractive','-File',$scriptPath,'-ProfileRoot',$sourcePath,'-Destination',$destinationPath)){[void]$info.ArgumentList.Add($arg)}
  $hostPath=Join-Path $casePath 'host'
  foreach($environmentName in @('HOME','USERPROFILE','APPDATA','LOCALAPPDATA','TEMP','TMP')) {
    $value=Join-Path $hostPath $environmentName;[IO.Directory]::CreateDirectory($value)|Out-Null;$info.Environment[$environmentName]=$value
  }
  $info.Environment['HOMEBASE_TEST_HOME']=$sourcePath
  if ($Mutation) { [void]$info.ArgumentList.Add('-ProgressEvents');[void]$info.ArgumentList.Add('-TestVerificationGate') }
  if ($Name -eq 'gate-rejected') {
    [void]$info.ArgumentList.Add('-ProgressEvents');[void]$info.ArgumentList.Add('-TestVerificationGate');$info.Environment['HOMEBASE_TEST_HOME']=$hostPath
  }
  $child=[Diagnostics.Process]::new();$child.StartInfo=$info
  try {
    Check ($child.Start())
    $outputTask=$child.StandardOutput.ReadToEndAsync()
    if ($Mutation) {
      while ($true) {
        $line=$child.StandardError.ReadLine()
        if ($null -eq $line) { throw 'Owned test gate was not observed' }
        Check (-not $line.Contains($canary))
        if ($line -ceq 'backup:test-verification-gate') { break }
      }
      switch($Mutation) {
        'replace' { Protect (Join-Path $root 'assistant/state.dpapi') }
        'routes' { [IO.File]::WriteAllText((Join-Path $root 'assistant/model-routes.json'),'changed-'+$canary) }
        'add' { Protect (Join-Path $root ('assistant/desktop/'+('b'*64)+'.draft.dpapi')) }
        'delete' { [IO.File]::Delete((Join-Path $root 'wechat/channel.dpapi')) }
        'inbox-add' { Protect (Join-Path $root 'inbox/receipts/87562a38-8fb4-4276-bdda-bd0d85bef544.receipt.dpapi') }
        'inbox-replace' { Protect (Join-Path $root 'inbox/source.workspace.dpapi') }
        'development-add' { Protect (Join-Path $root 'capabilities/development-workspace/editor.workspace.dpapi') }
        'development-replace' { Protect (Join-Path $root 'capabilities/development-workspace/editor.workspace.dpapi') }
        'ledger-append' { [IO.File]::AppendAllText((Join-Path $root 'control/operations.jsonl'),"{}`n") }
        'no-release' { }
      }
      if ($Mutation -ne 'no-release') { $child.StandardInput.WriteLine('continue') }
    }
    $child.StandardInput.Close()
    $errorsTask=$child.StandardError.ReadToEndAsync()
    Check ($child.WaitForExit(30000))
    $output=$outputTask.GetAwaiter().GetResult();$errors=$errorsTask.GetAwaiter().GetResult()
    Check (-not $output.Contains($canary) -and -not $errors.Contains($canary))
    $report=ConvertFrom-Json -InputObject $output -AsHashtable
    [Console]::Error.WriteLine('fixture-result:'+ $Name + ':' + $report.code)
    Check ($report.code -ceq $Expected -and $report.safe_to_update -ceq $false -and $report.source_writes_performed -ceq $false)
    if ($Expected -ne 'BACKUP_VERIFIED') {
      $createdByCall = -not $destinationBefore -and [IO.Directory]::Exists($destinationPath)
      Check ($report.partial_backup_retained -ceq $createdByCall)
      if ($report.private_acl_verified) {
        $directoryAcl=Get-Acl -LiteralPath $destinationPath
        $directoryRules=@($directoryAcl.GetAccessRules($true,$true,[Security.Principal.SecurityIdentifier]))
        Check ($createdByCall -and $directoryAcl.AreAccessRulesProtected -and $directoryRules.Count -eq 1 -and
          $directoryRules[0].IdentityReference.Value -ceq [Security.Principal.WindowsIdentity]::GetCurrent().User.Value)
      } elseif ($createdByCall) { Check ([IO.Directory]::GetFiles($destinationPath).Count -eq 0) }
    }
    Check ($child.ExitCode -eq $(if($Expected -eq 'BACKUP_VERIFIED'){0}else{1}))
    if ($null -ne $held) { $held.Dispose();$held=$null }
    if (-not $Mutation -or $Mutation -eq 'no-release') { Check ((Hashes $root) -ceq $before) }
    if ($destinationBefore) { Check ((Hashes $destinationPath) -ceq $destinationBefore); Check ((Get-Acl -LiteralPath $destinationPath).Sddl -ceq $aclBefore) }
    $manifestFile=Join-Path $destinationPath 'manifest.json'
    if (-not $destinationBefore -and [IO.Directory]::Exists($destinationPath)) {
      foreach($file in [IO.Directory]::GetFiles($destinationPath)) { Check (-not [Text.Encoding]::UTF8.GetString([IO.File]::ReadAllBytes($file)).Contains($canary)) }
    }
    if ($Expected -eq 'BACKUP_VERIFIED') {
      Check ([IO.File]::Exists($manifestFile))
      $manifest=ConvertFrom-Json -InputObject ([IO.File]::ReadAllText($manifestFile)) -AsHashtable
      Check ($manifest.files.Count -gt 0 -and $manifest.existing_credentials_decrypted -ceq $false -and $manifest.plaintext_credentials_saved -ceq $false)
      if ($Name -eq 'draft-and-handoff') {
        Check ($manifest.files.Count -eq 5)
        Check ($manifest.files.source_relative -ccontains ('assistant/desktop/'+('a'*64)+'.draft.dpapi'))
        Check (-not ($manifest.files.source_relative -match '\.lock$'))
      }
      if ($Name -eq 'inbox-covered') {
        Check ($manifest.files.Count -eq 5)
        Check ($manifest.files.source_relative -ccontains 'inbox/source.workspace.dpapi')
        Check ($manifest.files.source_relative -ccontains 'inbox/receipts/87562a38-8fb4-4276-bdda-bd0d85bef544.receipt.dpapi')
        Check (-not ($manifest.files.source_relative -match '\.lock$'))
      }
      if ($Name -eq 'development-covered') {
        Check ($manifest.files.Count -eq 4)
        Check ($manifest.files.source_relative -ccontains 'capabilities/development-workspace/editor.workspace.dpapi')
        Check (-not ($manifest.files.source_relative -match '\.lock$'))
      }
      if ($Name -in @('control-ledger','empty-control-ledger')) {
        Check ($manifest.files.Count -eq 4)
        $ledger = @($manifest.files | Where-Object source_relative -CEQ 'control/operations.jsonl')
        Check ($ledger.Count -eq 1 -and $ledger[0].mode -ceq 'protect_metadata')
      }
      $sid=[Security.Principal.WindowsIdentity]::GetCurrent().User
      foreach($file in @($manifestFile)+@($manifest.files | ForEach-Object { Join-Path $destinationPath $_.backup })) {
        $acl=Get-Acl -LiteralPath $file;$rules=@($acl.GetAccessRules($true,$true,[Security.Principal.SecurityIdentifier]))
        Check ($rules.Count -eq 1 -and $rules[0].IdentityReference.Value -ceq $sid.Value -and $rules[0].AccessControlType -eq 'Allow' -and $rules[0].FileSystemRights -eq 'FullControl')
      }
      foreach($entry in $manifest.files) {
        Check ($entry.source_relative -notmatch 'ipc-secrets|endpoint|shared|\.env')
        $bytes=[IO.File]::ReadAllBytes((Join-Path $destinationPath $entry.backup))
        if ($entry.mode -eq 'protect_metadata') {
          Check (-not [Text.Encoding]::UTF8.GetString($bytes).Contains($canary))
          $plain=[Security.Cryptography.ProtectedData]::Unprotect($bytes,$null,[Security.Cryptography.DataProtectionScope]::CurrentUser)
          try { Check ([Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($plain)) -ceq (Get-FileHash -LiteralPath (Join-Path $root $entry.source_relative)).Hash) }
          finally { [Array]::Clear($plain,0,$plain.Length) }
        } else { Check ((Get-FileHash -LiteralPath (Join-Path $root $entry.source_relative)).Hash -ceq (Get-FileHash -LiteralPath (Join-Path $destinationPath $entry.backup)).Hash) }
      }
    } else { Check (-not [IO.File]::Exists($manifestFile)) }
    $results.Add(@{name=$Name;passed=$true;code=$report.code;source_change_injected=([bool]$Mutation -and $Mutation -ne 'no-release');private_canary_emitted=$false})
  } finally { if($null -ne $held){$held.Dispose()};$child.Dispose() }
}
Run 'complete' 'BACKUP_VERIFIED'
Run 'draft-and-handoff' 'BACKUP_VERIFIED' {param($s,$d,$r)
  Protect (Join-Path $r ('assistant/desktop/'+('a'*64)+'.draft.dpapi'))
  Protect (Join-Path $r ('assistant/desktop/completed-handoffs/'+('c'*64)+'.draft.dpapi'))
  [IO.File]::WriteAllBytes((Join-Path $r 'assistant/desktop/workspace.lock'),[byte[]]@())
}
Run 'desktop-lock-has-data' 'BACKUP_UNKNOWN_DESKTOP_FILE' {param($s,$d,$r)
  Protect (Join-Path $r 'assistant/desktop/workspace.lock')
}
Run 'desktop-linked-lock' 'BACKUP_LINKED_PATH' {param($s,$d,$r)
  $physical = Join-Path $s 'lock-target'
  [IO.Directory]::CreateDirectory($physical) | Out-Null
  [IO.Directory]::CreateDirectory((Join-Path $r 'assistant/desktop')) | Out-Null
  New-Item -ItemType Junction -Path (Join-Path $r 'assistant/desktop/workspace.lock') -Target $physical | Out-Null
}
Run 'minimal' 'BACKUP_VERIFIED' {param($s,$d,$r) [IO.File]::Delete((Join-Path $r 'wechat/channel.dpapi'));[IO.File]::Delete((Join-Path $r 'assistant/model-routes.json'))}
Run 'existing-target' 'BACKUP_DESTINATION_EXISTS' {param($s,$d,$r) [IO.Directory]::CreateDirectory($d)|Out-Null;[IO.File]::WriteAllText((Join-Path $d 'sentinel'),$canary)}
Run 'missing-state' 'BACKUP_REQUIRED_STATE_MISSING' {param($s,$d,$r) [IO.File]::Delete((Join-Path $r 'assistant/state.dpapi'))}
Run 'plain-state' 'BACKUP_NOT_DPAPI_CIPHERTEXT' {param($s,$d,$r) [IO.File]::WriteAllText((Join-Path $r 'assistant/state.dpapi'),$canary)}
Run 'plain-wechat' 'BACKUP_NOT_DPAPI_CIPHERTEXT' {param($s,$d,$r) [IO.File]::WriteAllText((Join-Path $r 'wechat/channel.dpapi'),$canary)}
Run 'state-is-directory' 'BACKUP_INVALID_SOURCE' {param($s,$d,$r) [IO.File]::Delete((Join-Path $r 'assistant/state.dpapi'));[IO.Directory]::CreateDirectory((Join-Path $r 'assistant/state.dpapi'))|Out-Null}
Run 'empty-state' 'BACKUP_SIZE_LIMIT' {param($s,$d,$r) [IO.File]::WriteAllBytes((Join-Path $r 'assistant/state.dpapi'),[byte[]]@())}
Run 'oversized-state' 'BACKUP_SIZE_LIMIT' {param($s,$d,$r) $f=[IO.File]::Create((Join-Path $r 'assistant/state.dpapi'));try{$f.SetLength(32MB+1)}finally{$f.Dispose()}}
Run 'oversized-routes' 'BACKUP_SIZE_LIMIT' {param($s,$d,$r) $f=[IO.File]::Create((Join-Path $r 'assistant/model-routes.json'));try{$f.SetLength(2MB+1)}finally{$f.Dispose()}}
Run 'locked-source' 'BACKUP_SOURCE_READ'
Run 'unknown-draft' 'BACKUP_UNKNOWN_DESKTOP_FILE' {param($s,$d,$r) [IO.Directory]::CreateDirectory((Join-Path $r 'assistant/desktop'))|Out-Null;[IO.File]::WriteAllText((Join-Path $r 'assistant/desktop/unexpected'),$canary)}
Run 'unknown-desktop-directory' 'BACKUP_UNKNOWN_DESKTOP_DIRECTORY' {param($s,$d,$r) [IO.Directory]::CreateDirectory((Join-Path $r 'assistant/desktop/other'))|Out-Null}
Run 'history-unverified' 'BACKUP_STORAGE_COVERAGE_UNVERIFIED' {param($s,$d,$r) [IO.Directory]::CreateDirectory((Join-Path $r 'assistant/history-v1'))|Out-Null}
Run 'documents-unverified' 'BACKUP_STORAGE_COVERAGE_UNVERIFIED' {param($s,$d,$r) [IO.Directory]::CreateDirectory((Join-Path $r 'assistant/documents'))|Out-Null}
Run 'outside-destination' 'BACKUP_DESTINATION_SCOPE'
Run 'overlap-destination' 'BACKUP_SOURCE_DESTINATION_OVERLAP'
Run 'missing-parent' 'BACKUP_PARENT_MISSING'
Run 'linked-root' 'BACKUP_LINKED_PATH'
Run 'linked-parent' 'BACKUP_LINKED_PATH'
Run 'long-unicode' 'BACKUP_VERIFIED'
Run 'gate-rejected' 'BACKUP_TEST_GATE_REJECTED'
Run 'source-replaced' 'BACKUP_SOURCE_CHANGED' {} 'replace'
Run 'routes-replaced' 'BACKUP_SOURCE_CHANGED' {} 'routes'
Run 'inventory-added' 'BACKUP_INVENTORY_CHANGED' {} 'add'
Run 'inventory-deleted' 'BACKUP_INVENTORY_CHANGED' {} 'delete'
Run 'gate-eof' 'BACKUP_TEST_GATE_NOT_RELEASED' {} 'no-release'
Run 'inbox-covered' 'BACKUP_VERIFIED' {param($s,$d,$r)
  Protect (Join-Path $r 'inbox/source.workspace.dpapi')
  Protect (Join-Path $r 'inbox/receipts/87562a38-8fb4-4276-bdda-bd0d85bef544.receipt.dpapi')
  [IO.File]::WriteAllText((Join-Path $r 'inbox/source.writer.lock'),'')
  [IO.File]::WriteAllText((Join-Path $r 'inbox/receipts/writer.lock'),'')
}
Run 'inbox-unknown-file' 'BACKUP_UNKNOWN_INBOX_FILE' {param($s,$d,$r) Protect (Join-Path $r 'inbox/unrecognized.dpapi')}
Run 'inbox-unknown-directory' 'BACKUP_UNKNOWN_INBOX_DIRECTORY' {param($s,$d,$r) [IO.Directory]::CreateDirectory((Join-Path $r 'inbox/unrecognized'))|Out-Null}
Run 'inbox-plain-source' 'BACKUP_NOT_DPAPI_CIPHERTEXT' {param($s,$d,$r) [IO.Directory]::CreateDirectory((Join-Path $r 'inbox'))|Out-Null;[IO.File]::WriteAllText((Join-Path $r 'inbox/source.workspace.dpapi'),$canary)}
Run 'inbox-inventory-added' 'BACKUP_INVENTORY_CHANGED' {} 'inbox-add'
Run 'inbox-source-replaced' 'BACKUP_SOURCE_CHANGED' {param($s,$d,$r) Protect (Join-Path $r 'inbox/source.workspace.dpapi')} 'inbox-replace'
Run 'development-covered' 'BACKUP_VERIFIED' {param($s,$d,$r)
  Protect (Join-Path $r 'capabilities/development-workspace/editor.workspace.dpapi')
  [IO.File]::WriteAllText((Join-Path $r 'capabilities/development-workspace/editor.writer.lock'),'')
}
Run 'development-unknown-file' 'BACKUP_UNKNOWN_DEVELOPMENT_FILE' {param($s,$d,$r) Protect (Join-Path $r 'capabilities/development-workspace/unrecognized.dpapi')}
Run 'development-unknown-directory' 'BACKUP_UNKNOWN_DEVELOPMENT_DIRECTORY' {param($s,$d,$r) [IO.Directory]::CreateDirectory((Join-Path $r 'capabilities/development-workspace/unrecognized'))|Out-Null}
Run 'development-plain-source' 'BACKUP_NOT_DPAPI_CIPHERTEXT' {param($s,$d,$r) [IO.Directory]::CreateDirectory((Join-Path $r 'capabilities/development-workspace'))|Out-Null;[IO.File]::WriteAllText((Join-Path $r 'capabilities/development-workspace/editor.workspace.dpapi'),$canary)}
Run 'development-inventory-added' 'BACKUP_INVENTORY_CHANGED' {} 'development-add'
Run 'development-source-replaced' 'BACKUP_SOURCE_CHANGED' {param($s,$d,$r) Protect (Join-Path $r 'capabilities/development-workspace/editor.workspace.dpapi')} 'development-replace'
Run 'control-ledger' 'BACKUP_VERIFIED' {param($s,$d,$r)
  [IO.Directory]::CreateDirectory((Join-Path $r 'control')) | Out-Null
  [IO.File]::WriteAllText((Join-Path $r 'control/operations.jsonl'),('{"request_id":"'+$canary+'"}'+"`r`n"))
  [IO.File]::WriteAllText((Join-Path $r 'control/endpoint.json'),$canary)
}
Run 'empty-control-ledger' 'BACKUP_VERIFIED' {param($s,$d,$r)
  [IO.Directory]::CreateDirectory((Join-Path $r 'control')) | Out-Null
  [IO.File]::WriteAllBytes((Join-Path $r 'control/operations.jsonl'),[byte[]]@())
}
Run 'control-ledger-appended' 'BACKUP_SOURCE_CHANGED' {param($s,$d,$r)
  [IO.Directory]::CreateDirectory((Join-Path $r 'control')) | Out-Null
  [IO.File]::WriteAllText((Join-Path $r 'control/operations.jsonl'),"{}`n")
} 'ledger-append'
[IO.File]::WriteAllText((Join-Path $labPath 'backup-test-results.json'),(ConvertTo-Json -InputObject @($results.ToArray()) -Depth 8),[Text.UTF8Encoding]::new($false))
@{passed=$true;tests=$results.Count;real_profile_used=$false;real_credentials_used=$false;real_model_calls=0;real_wechat_messages=0;source_changes_only_by_explicit_fixture=$true;private_canary_emitted=$false} | ConvertTo-Json -Compress
