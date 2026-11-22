# End-to-end DPAPI fixtures only; no real profile, model or WeChat is opened.
param([Parameter(Mandatory = $true)][string]$Lab)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not [IO.Path]::IsPathFullyQualified($Lab) -or -not ([IO.Path]::GetFullPath($Lab).Replace('\','/').StartsWith('F:/HomeBase-Labs/', [StringComparison]::OrdinalIgnoreCase))) { throw 'Explicit F drive lab required' }
if (Test-Path -LiteralPath $Lab) { throw 'Use a new laboratory directory' }
[IO.Directory]::CreateDirectory($Lab) | Out-Null
$reader = Join-Path $PSScriptRoot 'read-rollout-preflight.ps1'
$canary = 'SYNTHETIC-PRIVATE-CANARY-DO-NOT-EMIT-测试🙂'
$proofs = [Collections.Generic.List[object]]::new()
function State {
  return @{revision=0;settings=@{enabled=$false;route_id=$canary};sessions=@(@{id='session-A';messages=@(@{content=$canary});jobs=@();turns=@(@{id='turn-A';state='completed';steps=@();queued=@();delivery_jobs=@();pending=$null;runtime_uncertain_model=$false})});channel_controls=@();followups=@{entries=@()}}
}
function Draft {
  return @{schema_version=1;revision=1;scope='new';body=@{text=$canary;route='';mode='auto';files=@()};pending=$null;native_pending=$null;voices=@()}
}
function Record-ToolReply($State, [string]$ResultStatus = 'failed') {
  $session = $State.sessions[0]
  $session.source = 'wechat'
  $turn = $session.turns[0]
  $turn.state = 'failed'
  $turn.runtime_uncertain_model = $true
  $turn.rounds = 2
  $turn.next_call = 0
  $name = 'tool-private-fixture'
  $id = 'call-private-fixture'
  $argsValue = [ordered]@{ value = $canary; date_spelling = '2026-11-18T04:00:00+08:00' }
  $result = [ordered]@{ status = $ResultStatus; detail = $canary }
  $fingerprintInput = ConvertTo-Json -InputObject ([ordered]@{ arguments = $argsValue; name = $name }) -Depth 64 -Compress
  $fingerprint = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($fingerprintInput))).ToLowerInvariant()
  $turn.queued = @(@{ id=$id; name=$name; arguments=$argsValue })
  $turn.steps = @(@{ id='assistant-turn-A-8'; name=$name; arguments=$argsValue; state='failed'; result=$result; job_id=$null })
  $turn.runtime_results = @{ $id=@{ fingerprint=$fingerprint; result=$result } }
  $turn.protocol = @(@{ role='tool'; tool_call_id=$id; content=(ConvertTo-Json -InputObject $result -Depth 64 -Compress) })
}
function Protect([string]$File, $Value, [string]$Raw = '') {
  if (-not $Raw) { $Raw = ConvertTo-Json -InputObject $Value -Depth 64 -Compress }
  $plain = [Text.Encoding]::UTF8.GetBytes($Raw)
  try {
    $cipher = [Security.Cryptography.ProtectedData]::Protect($plain,$null,[Security.Cryptography.DataProtectionScope]::CurrentUser)
    [IO.File]::WriteAllBytes($File, $cipher)
  } finally { [Array]::Clear($plain,0,$plain.Length) }
}
function Hashes([string]$Root) {
  $result = [ordered]@{}
  foreach ($file in @([IO.Directory]::GetFiles($Root, '*', [IO.SearchOption]::AllDirectories) | Sort-Object)) { $result[$file] = (Get-FileHash -LiteralPath $file).Hash }
  return ConvertTo-Json -InputObject $result -Compress
}
function Run([string]$Name, [scriptblock]$Arrange, [int]$ExpectedExit, [string]$ExpectedCode, [string]$Category = '', [int]$ExpectedCount = -1, [int]$RecordedCalls = -1, [int]$PendingSteps = -1, [int]$UnknownModels = 1, [string]$Review = '', [hashtable]$ControlExpected) {
  $root = Join-Path $Lab $Name
  $assistant = Join-Path $root '.homebase/assistant'
  $desktop = Join-Path $assistant 'desktop'
  [IO.Directory]::CreateDirectory($desktop) | Out-Null
  $state = State; $draft = Draft
  $stateFile = Join-Path $assistant 'state.dpapi'
  $draftFile = Join-Path $desktop (([Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes('new')))).ToLowerInvariant()+'.draft.dpapi')
  & $Arrange $state $draft $assistant
  if ($Name -ne 'missing-state') { Protect $stateFile $state }
  if ($Name -ne 'no-drafts') { Protect $draftFile $draft }
  if ($Name -in @('completed-handoff','invalid-completed-handoff')) {
    $scope='handoff:01234567-89ab-4def-8012-3456789abcde'
    $archived=Draft;$archived.scope=$scope
    if($Name -eq 'completed-handoff'){$archived.body.text=''}
    $completed=Join-Path $desktop 'completed-handoffs';[IO.Directory]::CreateDirectory($completed)|Out-Null
    $filename=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($scope))).ToLowerInvariant()+'.draft.dpapi'
    Protect (Join-Path $completed $filename) $archived
  }
  $injected = $Name -in @('source-change','inventory-change','ledger-source-change')
  $control = $Name.StartsWith('ledger-', [StringComparison]::Ordinal)
  $ledger = Join-Path $root '.homebase/control/operations.jsonl'
  if ($control -and $Name -cne 'ledger-missing') {
    [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($ledger)) | Out-Null
    $rows = @(
      @{id='old-job';request_id='old-request';command='synthetic';created_at='2026-11-17T00:00:00Z';fingerprint=$canary;status='running'}
      for ($i=0; $i -lt 105; $i++) {
        @{id="job-$i";request_id="request-$i";command='synthetic';created_at='2026-11-18T00:00:00Z';fingerprint=$canary;status='succeeded'}
      }
    )
    if ($Name -ceq 'ledger-finalized') { $rows += ($rows[0].Clone()); $rows[-1].status='failed' }
    if ($Name -ceq 'ledger-unknown') { $rows[0].status='future-state' }
    if ($Name -ceq 'ledger-identity-conflict') { $rows[-1].request_id='old-request' }
    $serialized = $rows | ForEach-Object { ConvertTo-Json -InputObject $_ -Compress }
    if ($Name -ceq 'ledger-partial-line') { $serialized += '{"id":' }
    if ($Name -ceq 'ledger-empty') { $serialized = @() }
    [IO.File]::WriteAllLines($ledger,[string[]]$serialized,[Text.UTF8Encoding]::new($false))
  }
  if ($injected) {
    # Enough independent protected files to observe a source mutation after the
    # reader's fixed event, without timing guesses or pausing the reader.
    for ($n=0;$n -lt 128;$n++) {
      $extra=Draft;$extra.scope=[guid]::NewGuid().ToString()
      $filename=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($extra.scope))).ToLowerInvariant()+'.draft.dpapi'
      Protect (Join-Path $desktop $filename) $extra
    }
  }
  switch ($Name) {
    'bad-cipher' { [IO.File]::WriteAllBytes($stateFile, [byte[]]@(1,2,3)) }
    'duplicate-json' { Protect $stateFile $null ('{"revision":0,"revision":1,"settings":{},"sessions":[]}') }
    'case-collision' { Protect $stateFile $null ('{"revision":0,"Revision":1,"settings":{},"sessions":[]}') }
    'bad-utf8' {
      $cipher=[Security.Cryptography.ProtectedData]::Protect([byte[]]@(255,254),$null,[Security.Cryptography.DataProtectionScope]::CurrentUser)
      [IO.File]::WriteAllBytes($stateFile,$cipher)
    }
    'oversized' { $stream=[IO.File]::Create($stateFile);try{$stream.SetLength(32MB+1)}finally{$stream.Dispose()} }
    'segmented' { [IO.Directory]::CreateDirectory((Join-Path $assistant 'history-v1'))|Out-Null;[IO.File]::WriteAllBytes((Join-Path $assistant 'history-v1/head.dpapi'),[byte[]]@(1)) }
  }
  $before = Hashes $root
  $readRoot=$root
  if($Name -eq 'linked-root'){$readRoot=Join-Path $Lab 'linked-root-alias';New-Item -ItemType Junction -Path $readRoot -Target $root|Out-Null}
  $info = [Diagnostics.ProcessStartInfo]::new((Join-Path $PSHOME 'pwsh.exe'))
  $info.UseShellExecute=$false;$info.CreateNoWindow=$true;$info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true
  foreach($arg in @('-NoLogo','-NoProfile','-NonInteractive','-File',$reader,'-ProfileRoot',$readRoot)){[void]$info.ArgumentList.Add($arg)}
  if($control){[void]$info.ArgumentList.Add('-IncludeControlLedger')}
  if($injected){[void]$info.ArgumentList.Add('-ProgressEvents')}
  foreach($key in @($info.Environment.Keys)){if($key -notin @('SystemRoot','WINDIR','COMSPEC','PATH','PATHEXT','ProgramFiles','ProgramFiles(x86)','ProgramW6432')){[void]$info.Environment.Remove($key)}}
  # PowerShell writes its own startup cache even with -NoProfile. Keep that
  # runtime separate from the application source whose immutability is tested.
  $shellProfile=Join-Path (Join-Path $Lab 'shell-profiles') $Name
  foreach($directory in @('AppData/Local','AppData/Roaming','tmp')){[IO.Directory]::CreateDirectory((Join-Path $shellProfile $directory))|Out-Null}
  $info.Environment['HOME']=$shellProfile;$info.Environment['USERPROFILE']=$shellProfile
  $info.Environment['APPDATA']=Join-Path $shellProfile 'AppData/Roaming'
  $info.Environment['LOCALAPPDATA']=Join-Path $shellProfile 'AppData/Local'
  $info.Environment['TEMP']=Join-Path $shellProfile 'tmp';$info.Environment['TMP']=$info.Environment['TEMP']
  $info.Environment['POWERSHELL_TELEMETRY_OPTOUT']='1'
  $process=[Diagnostics.Process]::Start($info)
  $outTask=$process.StandardOutput.ReadToEndAsync()
  $diagnostics=''
  if($injected){
    $eventTask=$process.StandardError.ReadLineAsync()
    if(-not $eventTask.Wait(10000)){throw "Owned preflight event unconfirmed; inspect PID $($process.Id), no replacement launched"}
    $event=$eventTask.GetAwaiter().GetResult()
    if($event -cne 'preflight:inventory-snapshotted'){throw 'Expected fixed preflight event was not observed'}
    $diagnostics=$event
    if($Name -eq 'source-change'){Protect $stateFile $state}
    elseif($Name -eq 'ledger-source-change') { [IO.File]::AppendAllText($ledger,"`n") }
    else {
      $extra=Draft;$extra.scope=[guid]::NewGuid().ToString()
      $filename=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($extra.scope))).ToLowerInvariant()+'.draft.dpapi'
      Protect (Join-Path $desktop $filename) $extra
    }
  }
  $errTask=$process.StandardError.ReadToEndAsync()
  if(-not $process.WaitForExit(20000)){throw 'Owned preflight observation timed out; do not start a replacement'}
  $stdout=$outTask.GetAwaiter().GetResult();$stderr=$diagnostics+$errTask.GetAwaiter().GetResult()
  if($stdout.Contains($canary)-or $stderr.Contains($canary)){throw 'Private fixture escaped redaction'}
  if($process.ExitCode -ne $ExpectedExit){throw "Unexpected preflight exit for $Name"}
  $result=ConvertFrom-Json -InputObject $stdout -AsHashtable
  if($result.code -cne $ExpectedCode -or $result.safe_to_update -cne $false -or $result.writes_performed -cne $false){throw "Unexpected structured result for $Name"}
  if($Category -and $result.blocker_categories -cnotcontains $Category){throw "Expected blocker category missing for $Name"}
  if($ExpectedCount -ge 0 -and $result.counts[$Category] -ne $ExpectedCount){throw "The same entity was counted more than once for $Name"}
  if($Review -and $result.assistant_review -cne $Review){throw "Unexpected assistant review classification for $Name"}
  if($ControlExpected){
    if($result.coverage -cnotcontains 'persisted_control_ledger' -or $result.ciphertexts_checked -ne 2){throw "Unexpected control coverage for $Name"}
    foreach($key in $ControlExpected.Keys){if($result.control_ledger[$key] -ne $ControlExpected[$key]){throw "Unexpected control count for $Name"}}
  }
  if($RecordedCalls -ge 0){
    if($result.counts.recorded_tool_calls -ne $RecordedCalls -or $result.counts.pending_steps -ne $PendingSteps){throw "Unexpected tool evidence classification for $Name"}
    if($result.unknown_model_by_source.wechat -ne $UnknownModels -or $result.unknown_model_by_source.desktop -ne 0 -or $result.blocker_categories -contains 'recorded_tool_calls'){throw "Unexpected model or blocker projection for $Name"}
  }
  if($ExpectedExit -eq 1 -and $null -ne $result.counts){throw 'Failure must not resemble empty successful inspection'}
  $unchanged=(Hashes $root)-ceq $before
  if($unchanged -eq $injected){throw "Unexpected source mutation result for $Name"}
  $proofs.Add(@{case=$Name;exit=$process.ExitCode;code=$result.code;redacted=$true;source_unchanged=$unchanged;expected_injected_change=$injected})
}
Run 'no-drafts' {} 0 'PREFLIGHT_NO_ASSISTANT_BLOCKERS'
Run 'saved-draft' {} 0 'PREFLIGHT_NO_ASSISTANT_BLOCKERS'
Run 'active-turn' {param($s,$d,$a) $s.sessions[0].turns[0].state='running'} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'active_turns'
Run 'queued-turn' {param($s,$d,$a) $s.sessions[0].turns[0].state='waiting_input_queue'} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'active_turns'
Run 'unknown-turn' {param($s,$d,$a) $s.sessions[0].turns[0].state='future-state'} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'unknown_states'
Run 'uncertain-turn' {param($s,$d,$a) $s.sessions[0].turns[0].state='interrupted_unknown'} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'uncertain_turns'
Run 'uncertain-model' {param($s,$d,$a) $s.sessions[0].turns[0].runtime_uncertain_model=$true} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'uncertain_turns'
Run 'approval' {param($s,$d,$a) $s.sessions[0].turns[0].pending=@{message='private'}} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'pending_approvals'
Run 'queued-step' {param($s,$d,$a) $s.sessions[0].turns[0].queued=@(@{command='synthetic'})} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'pending_steps'
Run 'unknown-step' {param($s,$d,$a) $s.sessions[0].turns[0].steps=@(@{state='unknown'})} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'pending_steps'
Run 'orphan-control-intent' {param($s) $s.sessions[0].turns[0].control_intents=@{'synthetic-intent'=$canary}} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'unresolved_control_intents' 1 -Review 'pending_or_unconfirmed'
Run 'malformed-control-intent-map' {param($s) $s.sessions[0].turns[0].control_intents=@()} 1 'PREFLIGHT_INVALID_SCHEMA'
Run 'terminal-step-missing-job' {param($s) $s.sessions[0].turns[0].steps=@(@{state='completed';job_id='synthetic-missing'})} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'unresolved_step_receipts' 1
Run 'terminal-step-known-job' {param($s) $s.sessions[0].jobs=@(@{id='synthetic-job';state='succeeded'});$s.sessions[0].turns[0].steps=@(@{state='completed';job_id='synthetic-job'})} 0 'PREFLIGHT_NO_ASSISTANT_BLOCKERS' -Review 'none'
Run 'terminal-step-with-still-running-result' {param($s) $s.sessions[0].turns[0].steps=@(@{state='completed';result=@{result=@{state='running';detail=$canary}}})} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'unresolved_step_receipts' 1
Run 'recorded-tool-keeps-model-uncertainty' {param($s) Record-ToolReply $s} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'uncertain_turns' 1 1 0 -Review 'historical_model_outcome_only'
Run 'closed-model-unknown-and-orphan-intent' {param($s) Record-ToolReply $s; $s.sessions[0].turns[0].control_intents=@{'other-synthetic-step'=$canary}} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'unresolved_control_intents' 1 1 0 -Review 'pending_or_unconfirmed'
Run 'completed-with-model-uncertainty' {param($s) $s.sessions[0].turns[0].runtime_uncertain_model=$true} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'uncertain_turns' 1 -Review 'pending_or_unconfirmed'
Run 'recorded-tool-is-not-a-blocker' {param($s) Record-ToolReply $s; $s.sessions[0].turns[0].runtime_uncertain_model=$false} 0 'PREFLIGHT_NO_ASSISTANT_BLOCKERS' '' -1 1 0 0 -Review 'none'
Run 'recorded-tool-wrong-fingerprint' {param($s) Record-ToolReply $s; $t=$s.sessions[0].turns[0]; $t.runtime_results[$t.queued[0].id].fingerprint='f'*64} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'pending_steps' 1 0 1
Run 'recorded-tool-different-arguments' {param($s) Record-ToolReply $s; $s.sessions[0].turns[0].queued[0].arguments=@{value=$canary;date_spelling='2026-11-17T20:00:00Z'}} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'pending_steps' 1 0 1
Run 'recorded-tool-mismatched-protocol' {param($s) Record-ToolReply $s; $s.sessions[0].turns[0].protocol[0].content='{}'} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'pending_steps' 1 0 1
Run 'recorded-tool-unknown-result' {param($s) Record-ToolReply $s 'unknown'} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'pending_steps' 1 0 1
Run 'recorded-tool-nested-unknown-result' {
  param($s)
  Record-ToolReply $s
  $turn = $s.sessions[0].turns[0]
  $value = $turn.steps[0].result
  $value['result'] = @{status='unknown';detail=$canary}
  $turn.protocol[0].content = ConvertTo-Json -InputObject $value -Depth 64 -Compress
} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'pending_steps' 1 0 1 -Review 'pending_or_unconfirmed'
Run 'recorded-tool-unverified-job' {param($s) Record-ToolReply $s; $s.sessions[0].turns[0].steps[0].job_id='original-job'} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'pending_steps' 1 0 1
Run 'recorded-tool-control-intent' {param($s) Record-ToolReply $s; $s.sessions[0].turns[0].control_intents=@{'assistant-turn-A-8'='original-command'}} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'pending_steps' 1 0 1
Run 'recorded-tool-duplicate-queue' {param($s) Record-ToolReply $s; $t=$s.sessions[0].turns[0]; $t.queued=@($t.queued[0],$t.queued[0])} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'pending_steps' 2 0 2
Run 'recorded-tool-active-turn' {param($s) Record-ToolReply $s; $s.sessions[0].turns[0].state='running'} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'pending_steps' 1 0 1
Run 'job' {param($s,$d,$a) $s.sessions[0].jobs=@(@{id='job';state='queued';remote=$null})} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'pending_jobs'
Run 'remote-query' {param($s,$d,$a) $s.sessions[0].jobs=@(@{id='job';state='succeeded';remote=@{finished=$true;query_job='pending'}})} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'pending_jobs'
Run 'missing-delivery' {param($s,$d,$a) $s.sessions[0].turns[0].delivery_jobs=@('missing')} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'unresolved_deliveries'
Run 'finished-delivery' {param($s,$d,$a) $s.sessions[0].jobs=@(@{id='delivery';state='succeeded';remote=$null});$s.sessions[0].turns[0].delivery_jobs=@('delivery')} 0 'PREFLIGHT_NO_ASSISTANT_BLOCKERS'
Run 'pending-control' {param($s,$d,$a) $s.channel_controls=@(@{response=$null})} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'pending_controls'
Run 'followup' {param($s,$d,$a) $s.followups.entries=@(@{state='running';runs=@(@{state='prepared'})})} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'active_followups'
Run 'future-followup' {param($s,$d,$a) $s.followups.entries=@(@{state='scheduled';runs=@()})} 0 'PREFLIGHT_NO_ASSISTANT_BLOCKERS'
Run 'pending-send' {param($s,$d,$a) $d.pending=@{state='prepared';request=@{text='private'}}} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'pending_sends'
Run 'native-send' {param($s,$d,$a) $d.native_pending=@{text='private'}} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'pending_native_sends'
Run 'ready-voice' {param($s,$d,$a) $d.voices=@(@{phase='ready';transcript='private'})} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'pending_voice'
Run 'finished-voice' {param($s,$d,$a) $d.voices=@(@{phase='applied'})} 0 'PREFLIGHT_NO_ASSISTANT_BLOCKERS'
Run 'unknown-voice' {param($s,$d,$a) $d.voices=@(@{phase='future'})} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'pending_voice'
Run 'duplicate-session' {param($s,$d,$a) $s.sessions=@($s.sessions[0],$s.sessions[0])} 1 'PREFLIGHT_DUPLICATE_OR_MISSING_ID'
Run 'invalid-type' {param($s,$d,$a) $s.sessions[0].turns[0].runtime_uncertain_model=1} 1 'PREFLIGHT_INVALID_SCHEMA'
Run 'draft-version' {param($s,$d,$a) $d.schema_version=2} 1 'PREFLIGHT_DRAFT_VERSION_UNVERIFIED'
Run 'draft-scope' {param($s,$d,$a) $d.scope='other'} 1 'PREFLIGHT_DRAFT_SCOPE_MISMATCH'
Run 'bad-cipher' {} 1 'PREFLIGHT_CRYPTO_FAILED'
Run 'duplicate-json' {} 1 'PREFLIGHT_DUPLICATE_JSON_KEY'
Run 'case-collision' {} 1 'PREFLIGHT_DUPLICATE_JSON_KEY'
Run 'bad-utf8' {} 1 'PREFLIGHT_INVALID_JSON'
Run 'oversized' {} 1 'PREFLIGHT_SIZE_LIMIT'
Run 'missing-state' {} 1 'PREFLIGHT_STATE_MISSING'
Run 'segmented' {} 1 'PREFLIGHT_SEGMENTED_STORAGE_UNVERIFIED'
Run 'draft-type' {param($s,$d,$a) $d.schema_version=$true} 1 'PREFLIGHT_DRAFT_VERSION_UNVERIFIED'
Run 'completed-handoff' {} 0 'PREFLIGHT_NO_ASSISTANT_BLOCKERS'
Run 'invalid-completed-handoff' {} 1 'PREFLIGHT_INVALID_COMPLETED_HANDOFF'
Run 'linked-root' {} 1 'PREFLIGHT_LINKED_PATH'
Run 'source-change' {} 1 'PREFLIGHT_SOURCE_CHANGED'
Run 'inventory-change' {} 1 'PREFLIGHT_SOURCE_CHANGED'
Run 'combined-uncertain' {param($s,$d,$a) $s.sessions[0].turns[0].state='interrupted_unknown';$s.sessions[0].turns[0].runtime_uncertain_model=$true} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'uncertain_turns' 1
Run 'combined-job' {param($s,$d,$a) $s.sessions[0].jobs=@(@{id='job';state='running';remote=@{finished=$false;query_job='pending'}})} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'pending_jobs' 1
Run 'combined-followup' {param($s,$d,$a) $s.followups.entries=@(@{state='running';runs=@(@{state='prepared'},@{state='accepted'})})} 2 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' 'active_followups' 1
Run 'ledger-incomplete' {} 2 'PREFLIGHT_CONTROL_REQUIRES_REVIEW' -Review 'none' -ControlExpected @{rows=106;jobs=106;terminal=105;without_terminal_receipt=1;to_review=0}
Run 'ledger-finalized' {} 0 'PREFLIGHT_NO_ASSISTANT_BLOCKERS' -ControlExpected @{rows=107;jobs=106;terminal=106;without_terminal_receipt=0;to_review=0}
Run 'ledger-unknown' {} 2 'PREFLIGHT_CONTROL_REQUIRES_REVIEW' -ControlExpected @{rows=106;jobs=106;terminal=105;without_terminal_receipt=0;to_review=1}
Run 'ledger-empty' {} 0 'PREFLIGHT_NO_ASSISTANT_BLOCKERS' -ControlExpected @{rows=0;jobs=0;terminal=0;without_terminal_receipt=0;to_review=0}
Run 'ledger-missing' {} 1 'PREFLIGHT_CONTROL_LEDGER_MISSING'
Run 'ledger-partial-line' {} 1 'PREFLIGHT_CONTROL_LEDGER_INVALID_LINE'
Run 'ledger-identity-conflict' {} 1 'PREFLIGHT_CONTROL_LEDGER_IDENTITY_CONFLICT'
Run 'ledger-source-change' {} 1 'PREFLIGHT_SOURCE_CHANGED'
$report=@{passed=$true;cases=$proofs.Count;proofs=$proofs;real_profiles_read=0;real_model_calls=0;real_wechat_messages=0}
$json=ConvertTo-Json -InputObject $report -Depth 6
[IO.File]::WriteAllText((Join-Path $Lab 'preflight-test-results.json'),$json,[Text.UTF8Encoding]::new($false))
$json
