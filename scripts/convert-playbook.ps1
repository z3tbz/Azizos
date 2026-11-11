<#
  convert-playbook.ps1 - convert an AME-style playbook (Configuration/*.yml + playbook.conf)
  into Azizos tweak-schema JSON files (data/tweaks/*.json) + an unconverted report.

  Usage:
    powershell -File convert-playbook.ps1 -PlaybookRoot <dir with Configuration> -OutDir <data/tweaks>

  Handles the bulk inline tags: registryValue, service, appx, file, taskKill, cmd.
  Multi-line run/powerShell/download/software are gathered best-effort; anything it
  cannot map cleanly is logged to unconverted.json so nothing is silently dropped.

  Display names / Arabic translations live in option-names.json (UTF-8) so this
  script stays pure ASCII (Windows PowerShell 5.1 misreads non-ASCII .ps1 files).
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory)] [string]$PlaybookRoot,
  [Parameter(Mandatory)] [string]$OutDir
)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force $OutDir | Out-Null

$namesPath  = Join-Path $PSScriptRoot 'option-names.json'
$names      = Get-Content -LiteralPath $namesPath -Raw -Encoding UTF8 | ConvertFrom-Json
$corePrefix = $names.corePrefix
$OptMap = @{}
foreach ($p in $names.options.PSObject.Properties) { $OptMap[$p.Name] = $p.Value }
$nativeActions = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'native-actions.json') -Raw -Encoding UTF8 | ConvertFrom-Json

function Map-Hive([string]$p) {
  switch -regex ($p) {
    # long and short root names: HKEY_LOCAL_MACHINE\ and HKLM\ (both appear in playbooks)
    '^(HKEY_LOCAL_MACHINE|HKLM)\\' { return @('hklm', ($p -replace '^(HKEY_LOCAL_MACHINE|HKLM)\\','')) }
    '^(HKEY_CURRENT_USER|HKCU)\\'  { return @('hkcu', ($p -replace '^(HKEY_CURRENT_USER|HKCU)\\','')) }
    '^(HKEY_CLASSES_ROOT|HKCR)\\'  { return @('hkcr', ($p -replace '^(HKEY_CLASSES_ROOT|HKCR)\\','')) }
    '^(HKEY_USERS|HKU)\\'          { return @('hku',  ($p -replace '^(HKEY_USERS|HKU)\\','')) }
    default                        { return @($null, $p) }
  }
}
function Map-RegKind([string]$t) {
  switch ($t.Trim()) {
    'REG_DWORD'     { 'dword' }    'REG_QWORD'     { 'qword' }
    'REG_SZ'        { 'sz' }       'REG_EXPAND_SZ' { 'expand_sz' }
    'REG_MULTI_SZ'  { 'multi_sz' } 'REG_BINARY'    { 'binary' }
    default         { throw "Unsupported registry type '$t'" }
  }
}
function Map-Startup([string]$n) {
  switch ($n.Trim()) { '0'{'boot'} '1'{'system'} '2'{'auto'} '3'{'demand'} '4'{'disabled'} default{throw "Unsupported service startup '$n'"} }
}
function Get-Field([string]$body, [string]$key) {
  # keys are separated by commas/braces (inline) OR whitespace (gathered multi-line);
  # unquoted values stop at the next separator so one key never swallows the next.
  $m = [regex]::Match($body, "(?:^|[\s,{])$key\s*:\s*(?:'([^']*)'|""([^""]*)""|([^,}\s]+))")
  if (-not $m.Success) { return $null }
  foreach ($g in 1..3) { if ($m.Groups[$g].Success) { return $m.Groups[$g].Value } }
  return $null
}

$actionsByTweak = @{}
$meta           = @{}
$unconverted    = New-Object System.Collections.ArrayList
$assetsNeeded   = New-Object System.Collections.ArrayList

function Add-Action($tweakId, $file, $isOption, $action) {
  if (-not $script:actionsByTweak.ContainsKey($tweakId)) {
    $script:actionsByTweak[$tweakId] = New-Object System.Collections.ArrayList
    $script:meta[$tweakId] = @{ file=$file; isOption=$isOption; order=$script:sourceOrder }
  }
  # AME's `ignoreErrors: true` / `errorAction: Ignore`: the playbook author says a
  # failure of this step is harmless. Carried on the action so the engine records
  # the reason without counting an error.
  $ignore = [bool]$script:ignoreCurrent
  if ($action.type -eq 'bundled' -and $nativeActions.PSObject.Properties[$action.file]) {
    foreach ($native in $nativeActions.($action.file)) {
      if ($ignore) {
        $copy = [ordered]@{}
        foreach ($prop in $native.PSObject.Properties) { $copy[$prop.Name] = $prop.Value }
        $copy['ignore_errors'] = $true
        [void]$script:actionsByTweak[$tweakId].Add($copy)
      } else { [void]$script:actionsByTweak[$tweakId].Add($native) }
    }
  } else {
    if ($ignore) { $action['ignore_errors'] = $true }
    [void]$script:actionsByTweak[$tweakId].Add($action)
  }
}

# A command line the playbook ran through cmd.exe. Where it is really a registry,
# file or environment change, emit that native action instead (no child process,
# and the engine can tell "already applied" / "not present" from a real error).
function Add-CmdLine($tweakId, $file, $isOption, [string]$line) {
  $line = $line.Trim()
  $q = { param($s) $s.Trim().Trim('"') }
  if ($line -match '^xcopy\s+"([^"]+)"\s+"([^"]+)"(?:\s+/[EIYH])+\s*$') {
    Add-Action $tweakId $file $isOption ([ordered]@{ type='file'; op='copy'; path=$Matches[1]; to=$Matches[2] }); return
  }
  # "<something> && reg delete "KEY" /f > nul 2>&1"  ->  <something> + delete_key
  if ($line -match '^(.+?)\s*&&\s*reg(?:\.exe)?\s+delete\s+"([^"]+)"\s+/f(?:\s*>\s*nul(?:\s+2>&1)?)?\s*$') {
    $first = $Matches[1]; $key = $Matches[2]
    Add-CmdLine $tweakId $file $isOption $first
    Add-CmdLine $tweakId $file $isOption ('reg delete "' + $key + '" /f')
    return
  }
  if ($line -match '^reg(?:\.exe)?\s+delete\s+"?([^"]+?)"?\s+/f(?:\s*>\s*nul(?:\s+2>&1)?)?\s*$') {
    $hv = Map-Hive $Matches[1]
    if ($hv[0]) { Add-Action $tweakId $file $isOption ([ordered]@{ type='registry'; op='delete_key'; hive=$hv[0]; path=$hv[1] }); return }
  }
  if ($line -match '^setx\s+(\S+)\s+(.+)$') {
    Add-Action $tweakId $file $isOption ([ordered]@{ type='env'; name=$Matches[1]; data=(& $q $Matches[2]); scope='user' }); return
  }
  if ($line -match '^ren\s+("[^"]+"|\S+)\s+("[^"]+"|\S+)$') {
    $src = & $q $Matches[1]; $new = & $q $Matches[2]
    $dir = $src.Substring(0, $src.LastIndexOf('\'))
    Add-Action $tweakId $file $isOption ([ordered]@{ type='file'; op='rename'; path=$src; to=($dir + '\' + $new) }); return
  }
  # fsutil behavior set X N is exactly a FileSystem registry value
  $fsKey = 'SYSTEM\CurrentControlSet\Control\FileSystem'
  if ($line -match '^fsutil(?:\.exe)?\s+behavior\s+set\s+disablelastaccess\s+([0-3])$') {
    # 0..3 -> 0x80000000 + n (user/system managed, enabled/disabled), as fsutil writes it
    Add-Action $tweakId $file $isOption ([ordered]@{ type='registry'; op='set'; hive='hklm'; path=$fsKey; name='NtfsDisableLastAccessUpdate'; kind='dword'; data=([int64]2147483648 + [int64]$Matches[1]) }); return
  }
  if ($line -match '^fsutil(?:\.exe)?\s+behavior\s+set\s+disable8dot3\s+([0-3])$') {
    Add-Action $tweakId $file $isOption ([ordered]@{ type='registry'; op='set'; hive='hklm'; path=$fsKey; name='NtfsDisable8dot3NameCreation'; kind='dword'; data=[int64]$Matches[1] }); return
  }
  if ($line -match '^(\S+?)(?:\.exe)?\s+(.*)$') {
    $tool = $Matches[1]; $rest = $Matches[2]   # copy before the next -match overwrites $Matches
    if ($tool -match '^(DISM|label|fsutil|rundll32|powercfg|sc|reg|bcdedit)$') {
      Add-Action $tweakId $file $isOption ([ordered]@{ type='command'; exe=$tool; args=$rest }); return
    }
  }
  # for %a in ("A" "B") do (if exist "P\%~a\X.exe" ("P\%~a\X.exe" ARGS))  ->  one
  # direct command per folder; the engine reports a missing one as not applicable.
  if ($line -match '^for\s+%(\w)\s+in\s+\(([^)]+)\)\s+do\s+\(if\s+exist\s+"([^"]+)"\s+\("([^"]+)"\s+([^)]*)\)\)$' -and $Matches[3] -eq $Matches[4]) {
    $var = $Matches[1]; $items = $Matches[2]; $template = $Matches[3]; $toolArgs = $Matches[5].Trim()
    foreach ($item in [regex]::Matches($items, '"([^"]+)"')) {
      Add-Action $tweakId $file $isOption ([ordered]@{ type='command'; exe=$template.Replace('%~' + $var, $item.Groups[1].Value); args=$toolArgs })
    }
    return
  }
  # A bare "<file>.exe args" is a file shipped with the app: start it directly
  # (no cmd.exe in between) so its real exit code comes back.
  if ($line -match '^([\w.-]+\.exe)\s*(.*)$') {
    Add-Action $tweakId $file $isOption ([ordered]@{ type='installer'; file=$Matches[1]; args=$Matches[2].Trim() }); return
  }
  Add-Action $tweakId $file $isOption ([ordered]@{ type='command'; exe='cmd'; args=('/c ' + $line) })
}

# Follow !task includes from custom.yml; alphabetical filesystem order puts
# FinalTasks before the files it finalizes. Reject unknown includes/cycles.
$cfgRoot = Join-Path $PlaybookRoot 'Configuration'
$script:orderedFiles = New-Object System.Collections.ArrayList
$script:visited = @{}
$script:visiting = @{}
function Visit-Config([string]$path) {
  $path = [IO.Path]::GetFullPath($path)
  if ($script:visiting[$path]) { throw "Task include cycle: $path" }
  if ($script:visited[$path]) { return }
  if (-not (Test-Path -LiteralPath $path)) { throw "Missing task include: $path" }
  $script:visiting[$path] = $true
  [void]$script:orderedFiles.Add((Get-Item -LiteralPath $path))
  foreach ($line in Get-Content -LiteralPath $path -Encoding UTF8) {
    if ($line -match '^\s*-\s*!task\s*:.*$') {
      $inc = Get-Field $line 'path'
      if (-not $inc) { throw "Unsupported task include: $line" }
      Visit-Config (Join-Path $cfgRoot $inc)
    }
  }
  $script:visiting.Remove($path)
  $script:visited[$path] = $true
}
if (Test-Path (Join-Path $cfgRoot 'custom.yml')) { Visit-Config (Join-Path $cfgRoot 'custom.yml') }
else { foreach ($f in Get-ChildItem -Recurse -Path $cfgRoot -Filter *.yml | Sort-Object FullName) { Visit-Config $f.FullName } }
$cfgFiles = @($script:orderedFiles)
$script:sourceOrder = 0
foreach ($f in $cfgFiles) {
  $lines = Get-Content -LiteralPath $f.FullName -Encoding UTF8
  for ($i=0; $i -lt $lines.Count; $i++) {
    $script:sourceOrder++
    $line = $lines[$i]
    $tagM = [regex]::Match($line, '^\s*-\s*!\s*(\w+)\s*:?\s*(\{.*\})?\s*(?:#.*)?$')
    if (-not $tagM.Success) { continue }
    $tag  = $tagM.Groups[1].Value
    $body = $tagM.Groups[2].Value
    if ($tag -eq 'task' -or $tag -eq 'writeStatus') { continue }

    $opt = if ($body) { Get-Field $body 'option' } else { $null }
    if (-not $body) {
      $blk = @(); $j=$i+1
      while ($j -lt $lines.Count -and $lines[$j] -notmatch '^\s*-\s*!') {
        if ($lines[$j].Trim() -and $lines[$j] -notmatch '^\s*#') { $blk += $lines[$j] }
        $j++
      }
      $body = ($blk -join ' ')
      $opt = Get-Field $body 'option'
    }
    $tweakId = if ($opt) { $opt } else { "core-" + ($f.BaseName.ToLower()) }
    $script:ignoreCurrent = ($body -match '(?:^|[\s,{])ignoreErrors\s*:\s*[''"]?true') -or ($body -match '(?:^|[\s,{])errorAction\s*:\s*[''"]?Ignore')

    switch ($tag) {
      'registryValue' {
        $path = Get-Field $body 'path'; $val = Get-Field $body 'value'
        $type = Get-Field $body 'type'; $data = Get-Field $body 'data'
        $hv = Map-Hive $path
        $operation = Get-Field $body 'operation'
        $regOp = if (-not $operation -or $operation -eq 'add' -or $operation -eq 'set') { 'set' } elseif ($operation -eq 'delete') { 'delete' } else { throw "Unsupported registry operation '$operation'" }
        $act = [ordered]@{ type='registry'; op=$regOp; hive=$hv[0]; path=$hv[1]; name=$val }
        if ($regOp -eq 'set') { $act.kind = Map-RegKind $type }
        if ($regOp -eq 'delete') {
          if (-not $hv[0]) { throw "Unknown registry root: $path" }
          Add-Action $tweakId $f.BaseName ([bool]$opt) $act
          break
        }
        if ($null -eq $data) { $data = '' }
        if ($act.kind -eq 'dword' -or $act.kind -eq 'qword') {
          $n=0; if ([int64]::TryParse($data, [ref]$n)) { $act.data = $n } else { $act.data = "$data" }
        } else { $act.data = "$data" }
        if (-not $hv[0]) { [void]$unconverted.Add(@{file=$f.Name; tag=$tag; body=$body; why='unknown registry root'}); break }
        Add-Action $tweakId $f.BaseName ([bool]$opt) $act
      }
      'service' {
        $nm = Get-Field $body 'name'; $op = Get-Field $body 'operation'; $su = Get-Field $body 'startup'
        $act = [ordered]@{ type='service'; name=$nm }
        if ($op -eq 'delete' -or $op -eq 'stop') { $act.op=$op }
        elseif (-not $op -or $op -eq 'change') { $act.op='change'; $act.startup=(Map-Startup $su) }
        else { throw "Unsupported service operation '$op'" }
        Add-Action $tweakId $f.BaseName ([bool]$opt) $act
      }
      'appx' {
        $nm = Get-Field $body 'name'; $tp = Get-Field $body 'type'; $unreg = Get-Field $body 'unregister'
        $operation = Get-Field $body 'operation'
        $op = if ($operation -eq 'clearCache') {'clear_cache'} elseif ($unreg -eq 'true') {'unregister'} else {'remove'}
        $sc = if ($tp) { $tp } else { 'family' }
        Add-Action $tweakId $f.BaseName ([bool]$opt) ([ordered]@{ type='appx'; name=$nm; scope=$sc; op=$op })
      }
      'file' {
        $path = Get-Field $body 'path'
        $op = Get-Field $body 'operation'
        if (-not $op) { $op = 'delete' }
        if ($op -notin @('delete','copy','rename')) { throw "Unsupported file operation '$op'" }
        $act = [ordered]@{ type='file'; op=$op; path=$path }
        if ($op -ne 'delete') {
          $act.to = Get-Field $body 'destination'
          if (-not $act.to) { throw "Missing destination for $op $path" }
        }
        Add-Action $tweakId $f.BaseName ([bool]$opt) $act
      }
      'taskKill' {
        # in-process (Toolhelp API) in the engine — no taskkill.exe
        $nm = Get-Field $body 'name'
        $act = [ordered]@{ type='kill'; name=$nm }
        $filter = Get-Field $body 'pathContains'
        if ($filter) { $act.path_contains = $filter.Replace('\\','\') }
        Add-Action $tweakId $f.BaseName ([bool]$opt) $act
      }
      'cmd' {
        $cmd = Get-Field $body 'command'
        if ($cmd) { Add-CmdLine $tweakId $f.BaseName ([bool]$opt) $cmd }
        else { [void]$unconverted.Add(@{file=$f.Name; tag=$tag; body=$body}) }
      }
      'powerShell' {
        $scr = Get-Field $body 'command'; $ru = Get-Field $body 'runas'
        $runas = if ($ru -match 'rusted') {'trusted_installer'} else {'user_elevated'}
        if (-not $scr) { [void]$unconverted.Add(@{file=$f.Name; tag=$tag; body=$body}); break }
        # '.\x.ps1 -Arg' and '& .\x.ps1 -Arg' are both bundled scripts: split file/args.
        if ($scr -match '^(?:&\s*)?\.[\\/](\S+)\s*(.*)$') {
          $bf = $Matches[1]; $ba = $Matches[2].Trim()
          if ($bf -ieq 'EDGE.ps1' -and $ba -match '-UninstallEdge' -and $ba -notmatch '-NonInteractive') { $ba += ' -NonInteractive' }
          # Opening the optimizer is done by the app after reboot (RunOnce), not mid-run.
          if ($bf -ieq 'Launch-AzizOptimizer.ps1') { break }
          Add-Action $tweakId $f.BaseName ([bool]$opt) ([ordered]@{ type='bundled'; file=$bf; args=$ba; runas=$runas })
        } elseif ($scr -match '^Get-AppxPackage\s+(\S+)\s*\|\s*Remove-AppxPackage$') {
          # the same removal, through the engine's PackageManager call (no PowerShell)
          $pkg = $Matches[1].Trim("'").Trim('"'); if ($pkg -notmatch '\*') { $pkg = '*' + $pkg + '*' }
          Add-Action $tweakId $f.BaseName ([bool]$opt) ([ordered]@{ type='appx'; name=$pkg; scope='family'; op='remove' })
        } else {
          Add-Action $tweakId $f.BaseName ([bool]$opt) ([ordered]@{ type='powershell'; script=$scr; runas=$runas })
        }
      }
      'run' {
        $exe = Get-Field $body 'exe'; $a = Get-Field $body 'args'; $ed = Get-Field $body 'exeDir'
        $ru = Get-Field $body 'runas'; $runas = if ($ru -match 'rusted') {'trusted_installer'} else {'user_elevated'}
        if ($null -eq $a) { $a = '' }
        $sysExe = @('dism.exe','dism','fsutil','label','rundll32.exe','cmd.exe','reg.exe','reg','powercfg','powercfg.exe','sc.exe','sc')
        if (-not $exe) { [void]$unconverted.Add(@{file=$f.Name; tag=$tag; body=$body}); break }
        if ($exe -match '^bcdedit(\.exe)?$') {
          Add-Action $tweakId $f.BaseName ([bool]$opt) ([ordered]@{ type='bcd'; args=$a })
        } elseif ($exe -match '^cmd(\.exe)?$' -and $a -match '^/c\s+(.+)$') {
          Add-CmdLine $tweakId $f.BaseName ([bool]$opt) $Matches[1]
        } elseif ($sysExe -contains $exe.ToLower()) {
          Add-CmdLine $tweakId $f.BaseName ([bool]$opt) ("$exe $a").Trim()
        } elseif ($ed -eq 'true' -or $exe -match '\.(bat|cmd|ps1|exe)$') {
          Add-Action $tweakId $f.BaseName ([bool]$opt) ([ordered]@{ type='bundled'; file=$exe; args=$a; runas=$runas })
        } else {
          Add-Action $tweakId $f.BaseName ([bool]$opt) ([ordered]@{ type='command'; exe=$exe; args=$a })
        }
      }
      'download' {
        # No internet at runtime: the file must be shipped in assets/. The playbook's
        # next !cmd already runs it with its silent flags, so no action is added here
        # (running it twice, once without flags, would open an interactive installer).
        $dest = Get-Field $body 'destination'
        if ($dest) { [void]$assetsNeeded.Add(@{file=$dest; tweak=$tweakId; url=(Get-Field $body 'url')}) }
        else { [void]$unconverted.Add(@{file=$f.Name; tag=$tag; body=$body}) }
      }
      'software' {
        # Package-manager installs need internet; ship '<pkg>.exe' in assets/ instead.
        $pkg = Get-Field $body 'package'; if (-not $pkg) { $pkg = Get-Field $body 'name' }
        [void]$assetsNeeded.Add(@{file="$pkg.exe"; tweak=$tweakId; url=''})
        Add-Action $tweakId $f.BaseName ([bool]$opt) ([ordered]@{ type='installer'; file="$pkg.exe"; args='/S' })
      }
      default { [void]$unconverted.Add(@{file=$f.Name; tag=$tag; body=$body}) }
    }
  }
}

$utf8 = New-Object System.Text.UTF8Encoding($false)
$written = 0
foreach ($id in $actionsByTweak.Keys) {
  $acts = @($actionsByTweak[$id])
  $isOpt = $meta[$id].isOption
  $desc = $null
  if ($isOpt -and $OptMap.ContainsKey($id)) {
    $o = $OptMap[$id]
    $name = @{ ar=$o.ar; en=$o.en }; $risk=$o.risk; $def=[bool]$o.default
  } else {
    $pretty = ($id -replace '^core-','')
    $o = $names.core.$pretty
    if ($o) { $name = @{ ar=$o.ar; en=$o.en } }
    else { $name = @{ ar=($corePrefix.ar + $pretty); en=($corePrefix.en + $pretty) } }
    $risk='low'; $def=$true
  }
  if ($o -and $o.desc_en) { $desc = @{ ar=$o.desc_ar; en=$o.desc_en } }
  $cat = switch -regex ($id) {
    'defender|uac|mitigation|spectre' { 'security'; break }
    'vpn|bluetooth|print|hyperv'      { 'devices'; break }
    'wu|update'                       { 'updates'; break }
    'services'                        { 'services'; break }
    'edge|onedrive|photos|appx|debloat' { 'debloat'; break }
    'dark|context|shell'              { 'appearance'; break }
    default { 'performance' }
  }
  $reboot = (($acts | Where-Object { $_.type -eq 'service' -or $_.type -eq 'bcd' }) | Measure-Object).Count -gt 0
  $obj = [ordered]@{
    id=$id; order=$meta[$id].order; name=$name; category=$cat; profiles=@(); default=$def;
    requires_reboot=$reboot; risk=$risk; actions=$acts
  }
  if ($desc) { $obj.Insert(2, 'description', $desc) }
  $json = $obj | ConvertTo-Json -Depth 8
  [IO.File]::WriteAllText((Join-Path $OutDir "$id.json"), $json, $utf8)
  $written++
}
$repPath = Join-Path $OutDir '..\unconverted.json'
[IO.File]::WriteAllText($repPath, (ConvertTo-Json -InputObject @($unconverted) -Depth 6), $utf8)
$assetsPath = Join-Path $OutDir '..\assets-needed.json'
[IO.File]::WriteAllText($assetsPath, (ConvertTo-Json -InputObject @($assetsNeeded) -Depth 4), $utf8)

"converted tweaks: $written"
"unconverted entries: $($unconverted.Count)"
$sum = ($actionsByTweak.Values | ForEach-Object { $_.Count } | Measure-Object -Sum).Sum
"actions total: $sum"
