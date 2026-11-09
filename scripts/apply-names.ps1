<#
  apply-names.ps1 - refresh the display name + description of every tweak in
  data/tweaks/*.json from scripts/option-names.json, without re-converting the
  playbook (actions are left untouched). Then rebuilds the UI catalog.
  Pure ASCII on purpose (PowerShell 5.1).
#>
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$utf8 = New-Object System.Text.UTF8Encoding($false)
$names = [IO.File]::ReadAllText((Join-Path $PSScriptRoot 'option-names.json'), $utf8) | ConvertFrom-Json

$n = 0
foreach ($f in Get-ChildItem (Join-Path $root 'data\tweaks') -Filter *.json) {
  $t = [IO.File]::ReadAllText($f.FullName, $utf8) | ConvertFrom-Json
  $o = $names.options.($t.id)
  if (-not $o -and $t.id -like 'core-*') { $o = $names.core.($t.id -replace '^core-','') }
  if (-not $o) { continue }
  $t.name = [pscustomobject]@{ ar = $o.ar; en = $o.en }
  if ($o.desc_en) {
    $d = [pscustomobject]@{ ar = $o.desc_ar; en = $o.desc_en }
    if ($t.PSObject.Properties['description']) { $t.description = $d }
    else { $t | Add-Member -NotePropertyName description -NotePropertyValue $d }
  }
  [IO.File]::WriteAllText($f.FullName, ($t | ConvertTo-Json -Depth 10), $utf8)
  $n++
}
"names applied: $n tweaks"
& (Join-Path $PSScriptRoot 'build-ui-catalog.ps1')
