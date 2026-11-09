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

function Map-Hive([string]$p) {
  switch -regex ($p) {
    '^HKEY_LOCAL_MACHINE\\' { return @('hklm', ($p -replace '^HKEY_LOCAL_MACHINE\\','')) }
    '^HKEY_CURRENT_USER\\'  { return @('hkcu', ($p -replace '^HKEY_CURRENT_USER\\','')) }
    '^HKEY_CLASSES_ROOT\\'  { return @('hkcr', ($p -replace '^HKEY_CLASSES_ROOT\\','')) }
    '^HKEY_USERS\\'         { return @('hku',  ($p -replace '^HKEY_USERS\\','')) }
    default                 { return @($null, $p) }
  }
}
function Map-RegKind([string]$t) {
  switch ($t.Trim()) {
    'REG_DWORD'     { 'dword' }    'REG_QWORD'     { 'qword' }
    'REG_SZ'        { 'sz' }       'REG_EXPAND_SZ' { 'expand_sz' }
    'REG_MULTI_SZ'  { 'multi_sz' } 'REG_BINARY'    { 'binary' }
    default         { 'sz' }
  }
}
function Map-Startup([string]$n) {
  switch ($n.Trim()) { '0'{'boot'} '1'{'system'} '2'{'auto'} '3'{'demand'} '4'{'disabled'} default{'disabled'} }
}
function Get-Field([string]$body, [string]$key) {
  # keys are separated by commas/braces (inline) OR whitespace (gathered multi-line);
  # unquoted values stop at the next separator so one key never swallows the next.
  $m = [regex]::Match($body, "(?:^|[\s,{])$key\s*:\s*(?:'([^']*)'|""([^""]*)""|([^,}\s]+))")
  if (-not $m.Success) { return $null }
  foreach ($g in 1..3) { if ($m.Groups[$g].Success) { return $m.Groups[$g].Value.Trim() } }
  return $null
}

$actionsByTweak = @{}
$meta           = @{}
$unconverted    = New-Object System.Collections.ArrayList
$assetsNeeded   = New-Object System.Collections.ArrayList

function Add-Action($tweakId, $file, $isOption, $action) {
  if (-not $script:actionsByTweak.ContainsKey($tweakId)) {
    $script:actionsByTweak[$tweakId] = New-Object System.Collections.ArrayList
    $script:meta[$tweakId] = @{ file=$file; isOption=$isOption }
  }
  [void]$script:actionsByTweak[$tweakId].Add($action)
}

$cfgFiles = Get-ChildItem -Recurse -Path (Join-Path $PlaybookRoot 'Configuration') -Filter *.yml
foreach ($f in $cfgFiles) {
  $lines = Get-Content -LiteralPath $f.FullName
  for ($i=0; $i -lt $lines.Count; $i++) {
    $line = $lines[$i]
    $tagM = [regex]::Match($line, '!\s*(\w+)\s*:?\s*(\{.*\})?\s*$')
    if (-not $tagM.Success) { continue }
    $tag  = $tagM.Groups[1].Value
    $body = $tagM.Groups[2].Value
    if ($tag -eq 'task' -or $tag -eq 'writeStatus') { continue }

    $opt = if ($body) { Get-Field $body 'option' } else { $null }
    if (-not $body -and ($tag -eq 'run' -or $tag -eq 'powerShell' -or $tag -eq 'cmd' -or $tag -eq 'download' -or $tag -eq 'software' -or $tag -eq 'file' -or $tag -eq 'service' -or $tag -eq 'appx')) {
      $blk = @(); $j=$i+1
      while ($j -lt $lines.Count -and $lines[$j] -notmatch '^\s*-\s*!' -and $lines[$j].Trim() -ne '') { $blk += $lines[$j]; $j++ }
      $body = ($blk -join ' ')
      $opt = Get-Field $body 'option'
    }
    $tweakId = if ($opt) { $opt } else { "core-" + ($f.BaseName.ToLower()) }

    switch ($tag) {
      'registryValue' {
        $path = Get-Field $body 'path'; $val = Get-Field $body 'value'
        $type = Get-Field $body 'type'; $data = Get-Field $body 'data'
        $hv = Map-Hive $path
        $act = [ordered]@{ type='registry'; op='set'; hive=$hv[0]; path=$hv[1]; name=$val; kind=(Map-RegKind $type) }
        if ($null -eq $data) { $data = '' }
        if ($act.kind -eq 'dword' -or $act.kind -eq 'qword') {
          $n=0; if ([int64]::TryParse($data, [ref]$n)) { $act.data = $n } else { $act.data = "$data" }
        } else { $act.data = "$data" }
        Add-Action $tweakId $f.BaseName ([bool]$opt) $act
      }
      'service' {
        $nm = Get-Field $body 'name'; $op = Get-Field $body 'operation'; $su = Get-Field $body 'startup'
        $act = [ordered]@{ type='service'; name=$nm }
        if ($op -eq 'delete') { $act.op='delete' } else { $act.op='change'; $act.startup=(Map-Startup $su) }
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
        Add-Action $tweakId $f.BaseName ([bool]$opt) ([ordered]@{ type='file'; op='delete'; path=$path })
      }
      'taskKill' {
        $nm = Get-Field $body 'name'
        Add-Action $tweakId $f.BaseName ([bool]$opt) ([ordered]@{ type='command'; exe='taskkill'; args=("/f /im " + '"' + $nm + '"') })
      }
      'cmd' {
        $cmd = Get-Field $body 'command'
        if ($cmd) { Add-Action $tweakId $f.BaseName ([bool]$opt) ([ordered]@{ type='command'; exe='cmd'; args=("/c " + $cmd) }) }
        else { [void]$unconverted.Add(@{file=$f.Name; tag=$tag; body=$body}) }
      }
      'powerShell' {
        $scr = Get-Field $body 'command'; $ru = Get-Field $body 'runas'
        $runas = if ($ru -match 'rusted') {'trusted_installer'} else {'user_elevated'}
        if (-not $scr) { [void]$unconverted.Add(@{file=$f.Name; tag=$tag; body=$body}); break }
        # '.\x.ps1 -Arg' and '& .\x.ps1 -Arg' are both bundled scripts: split file/args.
        if ($scr -match '^(?:&\s*)?\.[\\/](\S+)\s*(.*)$') {
          $bf = $Matches[1]; $ba = $Matches[2].Trim()
          # Opening the optimizer is done by the app after reboot (RunOnce), not mid-run.
          if ($bf -ieq 'Launch-AzizOptimizer.ps1') { break }
          Add-Action $tweakId $f.BaseName ([bool]$opt) ([ordered]@{ type='bundled'; file=$bf; args=$ba; runas=$runas })
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
        } elseif ($sysExe -contains $exe.ToLower()) {
          Add-Action $tweakId $f.BaseName ([bool]$opt) ([ordered]@{ type='command'; exe=$exe; args=$a })
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
  if ($isOpt -and $OptMap.ContainsKey($id)) {
    $o = $OptMap[$id]
    $name = @{ ar=$o.ar; en=$o.en }; $risk=$o.risk; $def=[bool]$o.default
  } else {
    $pretty = ($id -replace '^core-','')
    $name = @{ ar=($corePrefix.ar + $pretty); en=($corePrefix.en + $pretty) }; $risk='low'; $def=$true
  }
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
    id=$id; name=$name; category=$cat; profiles=@(); default=$def;
    requires_reboot=$reboot; risk=$risk; actions=$acts
  }
  $json = $obj | ConvertTo-Json -Depth 8
  [IO.File]::WriteAllText((Join-Path $OutDir "$id.json"), $json, $utf8)
  $written++
}
$repPath = Join-Path $OutDir '..\unconverted.json'
[IO.File]::WriteAllText($repPath, ($unconverted | ConvertTo-Json -Depth 6), $utf8)
$assetsPath = Join-Path $OutDir '..\assets-needed.json'
[IO.File]::WriteAllText($assetsPath, (ConvertTo-Json -InputObject @($assetsNeeded) -Depth 4), $utf8)

"converted tweaks: $written"
"unconverted entries: $($unconverted.Count)"
$sum = ($actionsByTweak.Values | ForEach-Object { $_.Count } | Measure-Object -Sum).Sum
"actions total: $sum"
