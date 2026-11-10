<#
  fetch-vendor-assets.ps1 - download the third-party installers the catalog
  references by exact filename (data/assets-needed.json) straight from their
  official sources, into assets/. Run once per machine before a release build
  (or in CI) so the shipped .exe is fully self-contained and the user never
  downloads anything separately at install time.

  Re-run any time to refresh to the pinned versions; existing files are
  overwritten. Checks each download is a real PE executable, not an error page.
#>
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$dst  = Join-Path $root 'assets'
New-Item -ItemType Directory -Force $dst | Out-Null

$needed = Get-Content (Join-Path $root 'data\assets-needed.json') -Raw -Encoding UTF8 | ConvertFrom-Json

foreach ($n in $needed) {
  if (-not $n.url) { "skip '$($n.file)': no url in assets-needed.json (add one or place it manually)"; continue }
  $out = Join-Path $dst $n.file
  Write-Host "downloading $($n.file) <- $($n.url)"
  Invoke-WebRequest -Uri $n.url -OutFile $out -UseBasicParsing
  $bytes = [IO.File]::ReadAllBytes($out)
  if ($bytes.Length -lt 2 -or $bytes[0] -ne 0x4D -or $bytes[1] -ne 0x5A) {   # 'MZ'
    Remove-Item $out -Force
    throw "$($n.file): downloaded file is not a Windows executable (bad URL or blocked?) — aborted"
  }
  "{0}: {1:N1} MB OK" -f $n.file, ($bytes.Length / 1MB)
}
