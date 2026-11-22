# Read-only metadata inspection. Never repairs, launches, exports or approves rollout.
# Requires PowerShell 7.4+ and Windows current-user DPAPI.
param([Parameter(Mandatory = $true)][string]$ProfileRoot, [switch]$ProgressEvents, [switch]$IncludeControlLedger)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$script:FailureCode = 'PREFLIGHT_INVALID_INPUT'
$script:Observed = [Collections.Generic.Dictionary[string,string]]::new([StringComparer]::OrdinalIgnoreCase)
$script:Limits = [Collections.Generic.Dictionary[string,long]]::new([StringComparer]::OrdinalIgnoreCase)
$script:RecordedCalls = [Collections.Generic.Dictionary[string,int]]::new([StringComparer]::Ordinal)
$script:FinishedTurns = @('completed','failed','canceled','cancelled','rejected','disabled','timed_out')
$script:FinishedSteps = @('completed','failed','canceled','cancelled','rejected')
$script:UnsettledResultStates = @('queued','running','pending','unknown','interrupted_unknown','uncertain','awaiting_confirmation','waiting_jobs','needs_input','partial','preflighting')
$script:TerminalUnknownModels = 0
$script:UnknownModelBySource = [ordered]@{ desktop = 0; wechat = 0; other = 0 }
$script:Counts = [ordered]@{
  sessions = 0; turns = 0; jobs = 0; active_turns = 0; uncertain_turns = 0
  pending_approvals = 0; pending_steps = 0; recorded_tool_calls = 0; pending_jobs = 0; unresolved_deliveries = 0
  pending_controls = 0; active_followups = 0; draft_files = 0; saved_drafts = 0
  unresolved_control_intents = 0; unresolved_step_receipts = 0
  pending_sends = 0; pending_native_sends = 0; pending_voice = 0; unknown_states = 0
}
function Fail([string]$Code) { $script:FailureCode = $Code; throw 'Redacted preflight failure' }
function Safe-Path([string]$File) {
  $current = [IO.Path]::GetFullPath($File)
  while ($current) {
    if (Test-Path -LiteralPath $current) {
      if (([IO.File]::GetAttributes($current) -band [IO.FileAttributes]::ReparsePoint) -ne 0) { Fail 'PREFLIGHT_LINKED_PATH' }
    }
    $parent = [IO.Path]::GetDirectoryName($current)
    if ($parent -eq $current) { break }; $current = $parent
  }
}
function Check-Json([Text.Json.JsonElement]$Value) {
  if ($Value.ValueKind -eq [Text.Json.JsonValueKind]::Object) {
    $names = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($property in $Value.EnumerateObject()) {
      if (-not $names.Add($property.Name)) { Fail 'PREFLIGHT_DUPLICATE_JSON_KEY' }
      Check-Json $property.Value
    }
  } elseif ($Value.ValueKind -eq [Text.Json.JsonValueKind]::Array) {
    foreach ($item in $Value.EnumerateArray()) { Check-Json $item }
  }
}
function Read-ControlLedger([string]$File) {
  Safe-Path $File
  if (-not [IO.File]::Exists($File)) { Fail 'PREFLIGHT_CONTROL_LEDGER_MISSING' }
  $script:FailureCode = 'PREFLIGHT_CONTROL_LEDGER_IO'
  $limit = 32MB
  $stream = [IO.FileStream]::new($File, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
  try {
    if ($stream.Length -gt $limit) { Fail 'PREFLIGHT_CONTROL_LEDGER_LIMIT' }
    $bytes = [byte[]]::new([int]$stream.Length)
    $stream.ReadExactly($bytes, 0, $bytes.Length)
  } finally { $stream.Dispose() }
  $script:Observed[$File] = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant()
  $script:Limits[$File] = $limit

  $script:FailureCode = 'PREFLIGHT_CONTROL_LEDGER_INVALID_LINE'
  $text = [Text.UTF8Encoding]::new($false, $true).GetString($bytes)
  $reader = [IO.StringReader]::new($text)
  $jobs = [Collections.Generic.Dictionary[string,object]]::new([StringComparer]::Ordinal)
  $requests = [Collections.Generic.Dictionary[string,string]]::new([StringComparer]::Ordinal)
  $rows = 0
  try {
    while ($null -ne ($line = $reader.ReadLine())) {
      $document = [Text.Json.JsonDocument]::Parse($line)
      try {
        $entry = $document.RootElement
        Check-Json $entry
        $job = Json-String $entry 'id'
        $request = Json-String $entry 'request_id'
        $command = Json-String $entry 'command'
        $fingerprint = Json-String $entry 'fingerprint'
        $created = Json-String $entry 'created_at'
        $status = Json-String $entry 'status'
        if (!$job -or !$request -or !$command -or !$fingerprint -or !$created -or !$status) { Fail 'PREFLIGHT_CONTROL_LEDGER_INVALID_LINE' }
        if (($requests.ContainsKey($request) -and $requests[$request] -cne $job) -or
            ($jobs.ContainsKey($job) -and ($jobs[$job].request -cne $request -or $jobs[$job].command -cne $command -or
              $jobs[$job].fingerprint -cne $fingerprint -or $jobs[$job].created -cne $created))) {
          Fail 'PREFLIGHT_CONTROL_LEDGER_IDENTITY_CONFLICT'
        }
        $requests[$request] = $job
        $jobs[$job] = @{ request=$request; command=$command; fingerprint=$fingerprint; created=$created; status=$status }
        $rows++
      } finally { $document.Dispose() }
    }
  } finally { $reader.Dispose() }
  $counts = [ordered]@{ rows=$rows; jobs=$jobs.Count; terminal=0; without_terminal_receipt=0; to_review=0 }
  foreach ($entry in $jobs.Values) {
    if ($entry.status -cin @('succeeded','failed','timed_out','canceled')) { $counts.terminal++ }
    elseif ($entry.status -cin @('queued','running')) { $counts.without_terminal_receipt++ }
    else { $counts.to_review++ }
  }
  return $counts
}
function Read-Protected([string]$File, [long]$Limit, [switch]$AssistantState) {
  Safe-Path $File
  if (-not [IO.File]::Exists($File)) { Fail 'PREFLIGHT_STATE_MISSING' }
  $fileInfo = [IO.FileInfo]::new($File)
  if ($fileInfo.Length -le 0 -or $fileInfo.Length -gt $Limit) { Fail 'PREFLIGHT_SIZE_LIMIT' }
  $cipher = $null; $plain = $null; $document = $null
  try {
    # FileShare.Read prevents an ordinary writer/replacement while these bytes are read.
    $stream = [IO.FileStream]::new($File, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    try {
      if ($stream.Length -gt $Limit) { Fail 'PREFLIGHT_SIZE_LIMIT' }
      $cipher = [byte[]]::new([int]$stream.Length)
    $stream.ReadExactly($cipher, 0, $cipher.Length)
    } finally { $stream.Dispose() }
    $script:Observed[$File] = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($cipher)).ToLowerInvariant()
    $script:Limits[$File] = $Limit
    $script:FailureCode = 'PREFLIGHT_CRYPTO_FAILED'
    $plain = [Security.Cryptography.ProtectedData]::Unprotect($cipher, $null, [Security.Cryptography.DataProtectionScope]::CurrentUser)
    if ($plain.Length -gt $Limit) { Fail 'PREFLIGHT_SIZE_LIMIT' }
    $script:FailureCode = 'PREFLIGHT_INVALID_JSON'
    $text = [Text.UTF8Encoding]::new($false, $true).GetString($plain)
    $document = [Text.Json.JsonDocument]::Parse($text)
    Check-Json $document.RootElement
    $value = ConvertFrom-Json -InputObject $text -AsHashtable -Depth 64
    if ($value -isnot [Collections.IDictionary]) { Fail 'PREFLIGHT_INVALID_SCHEMA' }
    $script:FailureCode = 'PREFLIGHT_INVALID_SCHEMA'
    if ($AssistantState) { Find-RecordedCalls $document.RootElement }
    return $value
  } finally {
    if ($document) { $document.Dispose() }
    if ($plain) { [Array]::Clear($plain, 0, $plain.Length) }
    if ($cipher) { [Array]::Clear($cipher, 0, $cipher.Length) }
  }
}
function Require-Array($Object, [string]$Key) {
  if ($Object -isnot [Collections.IDictionary] -or -not $Object.Contains($Key) -or $Object[$Key] -isnot [array]) { Fail 'PREFLIGHT_INVALID_SCHEMA' }
}
function Optional-Array($Object, [string]$Key) {
  if ($Object.Contains($Key)) { Require-Array $Object $Key }
}
function Map-Item($Object) { if ($Object -isnot [Collections.IDictionary]) { Fail 'PREFLIGHT_INVALID_SCHEMA' } }
function Valid-Scope([string]$Scope) {
  if ($Scope -ceq 'new' -or $Scope -cmatch '^native:(codex|claude-code|gemini-cli):[A-Za-z0-9_-]{1,120}$') { return $true }
  $identifier = $Scope
  if ($Scope.StartsWith('handoff:', [StringComparison]::Ordinal)) { $identifier = $Scope.Substring(8) }
  $guid = [guid]::Empty
  return [guid]::TryParse($identifier, [ref]$guid) -and $guid.ToString() -ceq $identifier
}
function Count-State($State, [string[]]$Finished, [string]$Counter) {
  if ($State -isnot [string] -or $State -cnotin $Finished) { $script:Counts[$Counter]++ }
}
function Has-UnresolvedStepReceipt($Step, [Collections.Generic.HashSet[string]]$JobIds) {
  $jobId = $Step['job_id']
  if ($null -ne $jobId -and ($jobId -isnot [string] -or -not $JobIds.Contains($jobId))) { return $true }
  $result = $Step['result']
  if ($result -isnot [Collections.IDictionary]) { return $false }
  foreach ($layer in @($result, $result['result'])) {
    if ($layer -is [Collections.IDictionary] -and
        ($layer['state'] -cin $script:UnsettledResultStates -or $layer['status'] -cin $script:UnsettledResultStates)) {
      return $true
    }
  }
  return $false
}
function Json-Member([Text.Json.JsonElement]$Parent, [string]$Name) {
  $member = [Text.Json.JsonElement]::new()
  if ($Parent.ValueKind -eq 'Object') { [void]$Parent.TryGetProperty($Name, [ref]$member) }
  return $member
}
function Json-String([Text.Json.JsonElement]$Parent, [string]$Name) {
  $member = Json-Member $Parent $Name
  if ($member.ValueKind -eq 'String') { return $member.GetString() }
  return ''
}
function Count-RecordedCalls([Text.Json.JsonElement]$Turn, [string]$TurnId) {
  if ((Json-String $Turn 'state') -cnotin $script:FinishedTurns -or (Json-Member $Turn 'pending').ValueKind -notin @('Null','Undefined')) { return 0 }
  $queue = Json-Member $Turn 'queued'
  $steps = Json-Member $Turn 'steps'
  $protocol = Json-Member $Turn 'protocol'
  $cache = Json-Member $Turn 'runtime_results'
  $intents = Json-Member $Turn 'control_intents'
  $rounds = Json-Member $Turn 'rounds'
  $roundNumber = [uint32]0
  if ($queue.ValueKind -ne 'Array' -or $queue.GetArrayLength() -notin 1..4 -or
      $steps.ValueKind -ne 'Array' -or $protocol.ValueKind -ne 'Array' -or $cache.ValueKind -ne 'Object' -or
      $intents.ValueKind -notin @('Object','Undefined') -or $rounds.ValueKind -ne 'Number' -or
      -not $rounds.TryGetUInt32([ref]$roundNumber) -or -not $roundNumber) { return 0 }
  $ids = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
  foreach ($call in $queue.EnumerateArray()) {
    $id = Json-String $call 'id'
    if (-not $id -or $id.Length -ge 200 -or -not $ids.Add($id)) { return 0 }
  }

  $position = 0
  $recorded = 0
  foreach ($call in $queue.EnumerateArray()) {
    $id = Json-String $call 'id'
    # The assistant allocates tool steps by round and position. The Deep Agents
    # cursor alone does not indicate which of the calls has a saved reply.
    $stepId = 'assistant-' + $TurnId + '-' + ([long]$roundNumber * 4 + $position)
    $position++
    $cached = Json-Member $cache $id
    $result = Json-Member $cached 'result'
    $name = Json-Member $call 'name'
    $arguments = Json-Member $call 'arguments'
    if ($name.ValueKind -ne 'String' -or $arguments.ValueKind -ne 'Object' -or $result.ValueKind -ne 'Object' -or
        (Json-Member $intents $stepId).ValueKind -ne 'Undefined') { continue }
    $unclear = $false
    foreach ($layer in @($result, (Json-Member $result 'result'))) {
      if ((Json-Member $layer 'job_id').ValueKind -notin @('Null','Undefined')) { $unclear = $true }
      foreach ($field in 'state','status') {
        $status = Json-Member $layer $field
        if ($status.ValueKind -notin @('Null','Undefined') -and
            (Json-String $layer $field) -cnotin (@('succeeded') + $script:FinishedSteps)) { $unclear = $true }
      }
    }
    if ($unclear) { continue }

    # Hash the original JSON spellings. A converted PowerShell object can change
    # dates, numbers, or escaping before comparison. The engine uses serde_json.
    $fingerprintInput = '{"arguments":' + $arguments.GetRawText() + ',"name":' + $name.GetRawText() + '}'
    $fingerprint = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($fingerprintInput))).ToLowerInvariant()
    if ((Json-String $cached 'fingerprint') -cne $fingerprint) { continue }
    $matchingStep = @($steps.EnumerateArray() | Where-Object { (Json-String $_ 'id') -ceq $stepId })
    if ($matchingStep.Count -ne 1) { continue }
    $step = $matchingStep[0]
    $stepArgs = Json-Member $step 'arguments'
    $stepResult = Json-Member $step 'result'
    if ((Json-String $step 'name') -cne $name.GetString() -or (Json-String $step 'state') -cnotin $script:FinishedSteps -or
        (Json-Member $step 'job_id').ValueKind -notin @('Null','Undefined') -or
        $stepArgs.ValueKind -ne 'Object' -or $stepArgs.GetRawText() -cne $arguments.GetRawText() -or
        $stepResult.ValueKind -ne 'Object' -or $stepResult.GetRawText() -cne $result.GetRawText()) { continue }
    $messages = @($protocol.EnumerateArray() | Where-Object { (Json-String $_ 'role') -ceq 'tool' -and (Json-String $_ 'tool_call_id') -ceq $id })
    if ($messages.Count -eq 1 -and (Json-String $messages[0] 'content') -ceq $result.GetRawText()) { $recorded++ }
  }
  return $recorded
}
function Find-RecordedCalls([Text.Json.JsonElement]$State) {
  $sessions = Json-Member $State 'sessions'
  if ($sessions.ValueKind -ne 'Array') { return }
  foreach ($session in $sessions.EnumerateArray()) {
    $turns = Json-Member $session 'turns'
    if ($turns.ValueKind -ne 'Array') { continue }
    foreach ($turn in $turns.EnumerateArray()) {
      $turnId = Json-String $turn 'id'
      if (-not $turnId) { continue }
      $recorded = Count-RecordedCalls $turn $turnId
      if ($recorded -gt 0) { $script:RecordedCalls[$turnId] = $recorded }
    }
  }
}
function Inventory([string]$Directory) {
  Safe-Path $Directory
  if (-not [IO.Directory]::Exists($Directory)) { return @() }
  $files = @([IO.Directory]::GetFiles($Directory, '*.draft.dpapi', [IO.SearchOption]::TopDirectoryOnly) | Sort-Object)
  if ($files.Count -gt 4096) { Fail 'PREFLIGHT_SIZE_LIMIT' }
  foreach ($file in $files) { Safe-Path $file }
  return $files
}
try {
  if (-not $IsWindows -or $PSVersionTable.PSVersion -lt [version]'7.4' -or $ProfileRoot -notmatch '^[A-Za-z]:[\\/]' -or -not [IO.Path]::IsPathFullyQualified($ProfileRoot)) { Fail 'PREFLIGHT_INVALID_INPUT' }
  $root = [IO.Path]::GetFullPath($ProfileRoot)
  Safe-Path $root
  $assistant = Join-Path $root '.homebase/assistant'
  $archiveHead = Join-Path $assistant 'history-v1/head.dpapi'
  Safe-Path $archiveHead
  if (Test-Path -LiteralPath $archiveHead) { Fail 'PREFLIGHT_SEGMENTED_STORAGE_UNVERIFIED' }
  $stateFile = Join-Path $assistant 'state.dpapi'
  $state = Read-Protected $stateFile (32MB) -AssistantState
  $script:FailureCode = 'PREFLIGHT_INVALID_SCHEMA'
  Require-Array $state 'sessions'
  if (-not $state.Contains('settings') -or $state.settings -isnot [Collections.IDictionary] -or
      -not $state.Contains('revision') -or $state.revision -isnot [long] -or $state.revision -lt 0) { Fail 'PREFLIGHT_INVALID_SCHEMA' }
  if ($state.Contains('archive_index') -and ($state.archive_index -isnot [Collections.IDictionary] -or $state.archive_index.Count -gt 0)) { Fail 'PREFLIGHT_SEGMENTED_STORAGE_UNVERIFIED' }
  $sessionIds = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
  $turnIds = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
  foreach ($session in $state.sessions) {
    Map-Item $session
    if ($session.id -isnot [string] -or -not $session.id -or -not $sessionIds.Add($session.id)) { Fail 'PREFLIGHT_DUPLICATE_OR_MISSING_ID' }
    $script:Counts.sessions++
    Require-Array $session 'turns'; Require-Array $session 'jobs'; Require-Array $session 'messages'
    $jobIds = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($job in $session.jobs) {
      Map-Item $job
      if ($job.id -isnot [string] -or -not $job.id -or -not $jobIds.Add($job.id)) { Fail 'PREFLIGHT_DUPLICATE_OR_MISSING_ID' }
      $script:Counts.jobs++
      $pendingJob = $job.state -cnotin @('succeeded', 'failed', 'canceled', 'cancelled', 'timed_out')
      if ($job.Contains('remote') -and $null -ne $job.remote) {
        Map-Item $job.remote
        if ($job.remote.finished -isnot [bool]) { Fail 'PREFLIGHT_INVALID_SCHEMA' }
        if ($job.remote.finished -cne $true -or $null -ne $job.remote.query_job) { $pendingJob = $true }
      }
      if ($pendingJob) { $script:Counts.pending_jobs++ }
    }
    foreach ($turn in $session.turns) {
      Map-Item $turn
      if ($turn.id -isnot [string] -or -not $turn.id -or -not $turnIds.Add($turn.id)) { Fail 'PREFLIGHT_DUPLICATE_OR_MISSING_ID' }
      $script:Counts.turns++
      $uncertainTurn = $false
      if ($turn.state -cin @('queued','running','waiting_jobs','awaiting_confirmation','waiting_input_queue')) { $script:Counts.active_turns++ }
      elseif ($turn.state -cin @('unknown','interrupted_unknown','paused_budget','needs_input')) { $uncertainTurn = $true }
      elseif ($turn.state -cnotin $script:FinishedTurns) { $script:Counts.unknown_states++ }
      if ($turn.Contains('runtime_uncertain_model')) {
        if ($turn.runtime_uncertain_model -isnot [bool]) { Fail 'PREFLIGHT_INVALID_SCHEMA' }
        if ($turn.runtime_uncertain_model) {
          $uncertainTurn = $true
          $source = if ($session['source'] -cin @('desktop','wechat')) { $session['source'] } else { 'other' }
          $script:UnknownModelBySource[$source]++
          if ($turn.state -cin @('failed','canceled','cancelled')) { $script:TerminalUnknownModels++ }
        }
      }
      if ($uncertainTurn) { $script:Counts.uncertain_turns++ }
      if ($turn.Contains('pending') -and $null -ne $turn.pending) { $script:Counts.pending_approvals++ }
      if ($turn.Contains('control_intents')) {
        Map-Item $turn.control_intents
        $script:Counts.unresolved_control_intents += $turn.control_intents.Count
      }
      Optional-Array $turn 'queued'
      if ($turn.Contains('queued') -and $turn.queued.Count) {
        $recorded = 0
        [void]$script:RecordedCalls.TryGetValue($turn.id,[ref]$recorded)
        $script:Counts.pending_steps += $turn.queued.Count - $recorded
        $script:Counts.recorded_tool_calls += $recorded
      }
      Require-Array $turn 'steps'
      foreach ($step in $turn.steps) {
        Map-Item $step
        Count-State $step.state $script:FinishedSteps 'pending_steps'
        if ($step.state -cin $script:FinishedSteps -and (Has-UnresolvedStepReceipt $step $jobIds)) {
          $script:Counts.unresolved_step_receipts++
        }
      }
      Optional-Array $turn 'delivery_jobs'
      if ($turn.Contains('delivery_jobs')) {
        foreach ($id in $turn.delivery_jobs) {
          $matches = @($session.jobs | Where-Object { $_.id -ceq $id })
          if ($id -isnot [string] -or $matches.Count -ne 1 -or $matches[0].state -cnotin @('succeeded','failed','canceled','cancelled','timed_out')) { $script:Counts.unresolved_deliveries++ }
        }
      }
    }
  }
  Optional-Array $state 'channel_controls'
  if ($state.Contains('channel_controls')) { foreach ($receipt in $state.channel_controls) { Map-Item $receipt; if (-not $receipt.Contains('response') -or $null -eq $receipt.response) { $script:Counts.pending_controls++ } } }
  if ($state.Contains('followups')) {
    Map-Item $state.followups; Require-Array $state.followups 'entries'
    foreach ($entry in $state.followups.entries) {
      Map-Item $entry
      $activeFollowup = $entry.state -ceq 'running'
      if ($activeFollowup) { }
      elseif ($entry.state -cnotin @('scheduled','waiting_event','completed','paused','canceled','cancelled')) { $script:Counts.unknown_states++ }
      Require-Array $entry 'runs'
      foreach ($run in $entry.runs) { Map-Item $run; if ($run.state -cnotin @('completed','failed','canceled','cancelled','not_submitted')) { $activeFollowup = $true } }
      if ($activeFollowup) { $script:Counts.active_followups++ }
    }
  }
  $desktop = Join-Path $assistant 'desktop'
  $completed = Join-Path $desktop 'completed-handoffs'
  $control = if ($IncludeControlLedger) { Read-ControlLedger (Join-Path $root '.homebase/control/operations.jsonl') }
  $beforeInventory = @(Inventory $desktop) + @(Inventory $completed)
  # Fixed diagnostic event only: no paths, identifiers, arguments or private data.
  if ($ProgressEvents) { [Console]::Error.WriteLine('preflight:inventory-snapshotted') }
  foreach ($file in $beforeInventory) {
    if ([IO.Path]::GetFileName($file) -cnotmatch '^[a-f0-9]{64}\.draft\.dpapi$') { Fail 'PREFLIGHT_INVALID_DRAFT_PATH' }
    $entry = Read-Protected $file (1MB)
    if ($entry.schema_version -isnot [long] -or $entry.schema_version -ne 1 -or $entry.revision -isnot [long] -or $entry.revision -le 0 -or $entry.scope -isnot [string]) { Fail 'PREFLIGHT_DRAFT_VERSION_UNVERIFIED' }
    $expected = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($entry.scope))).ToLowerInvariant() + '.draft.dpapi'
    if ([IO.Path]::GetFileName($file) -cne $expected) { Fail 'PREFLIGHT_DRAFT_SCOPE_MISMATCH' }
    if (-not (Valid-Scope $entry.scope)) { Fail 'PREFLIGHT_INVALID_DRAFT_SCOPE' }
    Map-Item $entry.body; Require-Array $entry.body 'files'
    if ($entry.body.text -isnot [string]) { Fail 'PREFLIGHT_INVALID_SCHEMA' }
    if ([IO.Path]::GetDirectoryName($file).Equals($completed, [StringComparison]::OrdinalIgnoreCase)) {
      if (-not $entry.scope.StartsWith('handoff:', [StringComparison]::Ordinal) -or $entry.body.text.Length -gt 0 -or $entry.body.files.Count -gt 0 -or
          $entry.body.route -cne '' -or $entry.body.mode -cne 'auto' -or $null -ne $entry.pending -or
          ($entry.Contains('native_pending') -and $null -ne $entry.native_pending) -or ($entry.Contains('voices') -and $entry.voices.Count -gt 0)) { Fail 'PREFLIGHT_INVALID_COMPLETED_HANDOFF' }
    }
    $script:Counts.draft_files++
    if ($entry.body.text.Length -gt 0 -or $entry.body.files.Count) { $script:Counts.saved_drafts++ }
    if ($null -ne $entry.pending) { $script:Counts.pending_sends++ }
    if ($entry.Contains('native_pending') -and $null -ne $entry.native_pending) { $script:Counts.pending_native_sends++ }
    Optional-Array $entry 'voices'
    if ($entry.Contains('voices')) { foreach ($voice in $entry.voices) { Map-Item $voice; Count-State $voice.phase @('applied','dismissed','cancelled','failed') 'pending_voice' } }
  }
  $afterInventory = @(Inventory $desktop) + @(Inventory $completed)
  if (($beforeInventory -join '|') -cne ($afterInventory -join '|') -or (Test-Path -LiteralPath $archiveHead)) { Fail 'PREFLIGHT_SOURCE_CHANGED' }
  $script:FailureCode = 'PREFLIGHT_SOURCE_CHANGED'
  foreach ($file in $script:Observed.Keys) {
    Safe-Path $file
    if (-not [IO.File]::Exists($file) -or [IO.FileInfo]::new($file).Length -gt $script:Limits[$file]) { Fail 'PREFLIGHT_SOURCE_CHANGED' }
    $stream = [IO.FileStream]::new($file, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    try {
      if ($stream.Length -gt $script:Limits[$file] -or [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($stream)).ToLowerInvariant() -cne $script:Observed[$file]) { Fail 'PREFLIGHT_SOURCE_CHANGED' }
    } finally { $stream.Dispose() }
  }
  $blockers = @($script:Counts.Keys | Where-Object { $_ -notin @('sessions','turns','jobs','draft_files','saved_drafts','recorded_tool_calls') -and $script:Counts[$_] -gt 0 })
  $review = if (!$blockers.Count) { 'none' }
  elseif ($blockers.Count -eq 1 -and $blockers[0] -ceq 'uncertain_turns' -and
      $script:TerminalUnknownModels -eq $script:Counts.uncertain_turns) { 'historical_model_outcome_only' }
  else { 'pending_or_unconfirmed' }
  $report = [ordered]@{ schema_version = 1; inspection_complete = $true; consistent = $true
    code = $(if ($blockers.Count) { 'PREFLIGHT_ASSISTANT_REQUIRES_REVIEW' } else { 'PREFLIGHT_NO_ASSISTANT_BLOCKERS' })
    safe_to_update = $false; coverage = @('legacy_assistant_state','protected_desktop_drafts')
    unverified = @('live_processes_and_unsaved_input','independent_business_processes','wechat_transport_and_delivery','backup_and_rollout_integrity','online_preservation')
    counts = $script:Counts; blocker_categories = $blockers; unknown_model_by_source = $script:UnknownModelBySource; ciphertexts_checked = $script:Observed.Count
    assistant_review = $review; terminal_unknown_model_turns = $script:TerminalUnknownModels
    state_ciphertext_sha256 = $script:Observed[$stateFile]; private_values_emitted = $false; writes_performed = $false }
  $controlNeedsReview = $false
  if ($IncludeControlLedger) {
    $report['control_ledger'] = $control
    $report.coverage += 'persisted_control_ledger'
    $report.ciphertexts_checked--
    $controlNeedsReview = $control.without_terminal_receipt -gt 0 -or $control.to_review -gt 0
    if ($controlNeedsReview) { $report.code = 'PREFLIGHT_CONTROL_REQUIRES_REVIEW' }
  }
  $report | ConvertTo-Json -Depth 6 -Compress
  if ($blockers.Count -or $controlNeedsReview) { exit 2 }; exit 0
} catch {
  [ordered]@{ schema_version = 1; inspection_complete = $false; consistent = $false
    code = $script:FailureCode; safe_to_update = $false; counts = $null
    private_values_emitted = $false; writes_performed = $false } | ConvertTo-Json -Compress
  exit 1
}
