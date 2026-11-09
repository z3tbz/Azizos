<#
  build-ui-catalog.ps1 - write ui/catalog.js (+ ui/catalog.json) from data/tweaks/*.json.

  The UI only needs what it displays (names, category, risk, reboot, how many
  actions), so the full action lists stay in the engine. One source of truth:
  data/tweaks -> engine (embedded by app/build.rs) and -> this slim UI index.
  Run after convert-playbook.ps1. Pure ASCII on purpose (PowerShell 5.1).
#>
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$utf8 = New-Object System.Text.UTF8Encoding($false)

$catPath = Join-Path $PSScriptRoot 'categories.json'
$categories = [IO.File]::ReadAllText($catPath, $utf8) | ConvertFrom-Json

$order = @{ low = 0; medium = 1; high = 2 }
$tweaks = foreach ($f in Get-ChildItem (Join-Path $root 'data\tweaks') -Filter *.json) {
  $t = [IO.File]::ReadAllText($f.FullName, $utf8) | ConvertFrom-Json
  $o = [ordered]@{
    id              = $t.id
    name            = $t.name
    category        = $t.category
    default         = [bool]$t.default
    risk            = if ($t.risk) { $t.risk } else { 'low' }
    requires_reboot = [bool]$t.requires_reboot
    actions_count   = @($t.actions).Count
  }
  if ($t.description) { $o.description = $t.description }
  [pscustomobject]$o
}
$tweaks = $tweaks | Sort-Object @{ e = { $order[$_.risk] } }, id

$doc = [ordered]@{ categories = $categories; tweaks = @($tweaks) }
$json = ConvertTo-Json -InputObject $doc -Depth 6 -Compress
[IO.File]::WriteAllText((Join-Path $root 'ui\catalog.json'), $json, $utf8)
[IO.File]::WriteAllText((Join-Path $root 'ui\catalog.js'), "window.CATALOG = $json;`n", $utf8)
"ui catalog: $(@($tweaks).Count) tweaks, $([math]::Round($json.Length/1KB,1)) KB"
