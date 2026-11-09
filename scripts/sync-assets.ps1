<#
  sync-assets.ps1 - copy the playbook's Executables folder into assets/ so
  app/build.rs embeds it into Azizos.exe (one self-contained file, no internet).

  Usage:
    powershell -File scripts\sync-assets.ps1 -PlaybookRoot <dir that holds Executables>

  assets/ is git-ignored (large binaries). Files the original playbook downloaded
  at run time are listed in data/assets-needed.json: drop them into assets/ too,
  and this script reports any that are still missing.
#>
[CmdletBinding()]
param([Parameter(Mandatory)] [string]$PlaybookRoot)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$src  = Join-Path $PlaybookRoot 'Executables'
$dst  = Join-Path $root 'assets'
if (-not (Test-Path -LiteralPath $src)) { throw "not found: $src" }

New-Item -ItemType Directory -Force $dst | Out-Null
robocopy $src $dst /E /NFL /NDL /NJH /NJS /NP | Out-Null
if ($LASTEXITCODE -ge 8) { throw "robocopy failed ($LASTEXITCODE)" }

$files = Get-ChildItem $dst -Recurse -File
"assets: {0} files, {1:N1} MB" -f $files.Count, (($files | Measure-Object Length -Sum).Sum / 1MB)

$needed = Join-Path $root 'data\assets-needed.json'
if (Test-Path $needed) {
  $list = Get-Content $needed -Raw | ConvertFrom-Json   # PS 5.1: assign first, then enumerate
  foreach ($n in $list) {
    if (-not (Test-Path (Join-Path $dst $n.file))) {
      "MISSING (needed by '$($n.tweak)'): $($n.file)  $($n.url)"
    }
  }
}
