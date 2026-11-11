# Regression test of the bridge using a synthetic playbook, never executed.
$ErrorActionPreference = 'Stop'
$testRoot = Join-Path $env:TEMP ('AzizosConverterTest-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path (Join-Path $testRoot 'Configuration') -Force | Out-Null
New-Item -ItemType Directory -Path (Join-Path $testRoot 'data/tweaks') -Force | Out-Null
$utf8 = New-Object System.Text.UTF8Encoding($false)
$fixture = @'
actions:
  - !registryValue: {path: 'HKCU\Software\AzizosFixture', value: 'Old', operation: delete} # inline comment
  - !service: {name: 'AzizosFixture', operation: stop}
  - !file: {path: 'source', operation: copy, destination: 'target'}
  - !taskKill: {name: 'setup', pathContains: '\Edge'}
  - !run: {exe: 'DEVMANVIEW.CMD', exeDir: true}
  - !taskKill: {name: 'msedge', ignoreErrors: true}
  - !cmd: {command: 'vcpp.exe /ai'}
  - !run: {exe: 'cmd.exe', args: '/c for %a in ("SysWOW64" "System32") do (if exist "%windir%\%~a\OneDriveSetup.exe" ("%windir%\%~a\OneDriveSetup.exe" /uninstall))'}
  - !powerShell:
    command: 'Get-AppxPackage MicrosoftWindows.Client.WebExperience | Remove-AppxPackage'
    runas: currentUserElevated
  - !cmd: {command: 'xcopy "Windows" "%windir%" /E /I /Y'}
  - !powerShell:
    command: '.\EDGE.ps1 -UninstallEdge'
    # comments must not be parsed as fields
    option: 'remove-edge'
    errorAction: Ignore
'@
[IO.File]::WriteAllText((Join-Path $testRoot 'Configuration/custom.yml'), $fixture, $utf8)
& (Join-Path $PSScriptRoot 'convert-playbook.ps1') -PlaybookRoot $testRoot -OutDir (Join-Path $testRoot 'data/tweaks')
$core = Get-Content (Join-Path $testRoot 'data/tweaks/core-custom.json') -Raw -Encoding UTF8 | ConvertFrom-Json
$edge = Get-Content (Join-Path $testRoot 'data/tweaks/remove-edge.json') -Raw -Encoding UTF8 | ConvertFrom-Json
function Assert([bool]$pass, [string]$message) { if (-not $pass) { throw $message } }
Assert ($core.actions[0].op -eq 'delete') 'registry delete became a set'
Assert ($core.actions[1].op -eq 'stop' -and -not $core.actions[1].startup) 'service stop became startup disabled'
Assert ($core.actions[2].op -eq 'copy' -and $core.actions[2].to -eq 'target') 'file copy became delete'
Assert ($core.actions[3].path_contains -eq '\Edge') 'process path restriction was dropped'
Assert (@($core.actions | Where-Object type -eq device).Count -eq 18) 'device script was not lowered'
Assert ($core.actions[-1].type -eq 'file' -and $core.actions[-1].op -eq 'copy') 'xcopy was not lowered'
Assert ($edge.actions[0].args -match '-NonInteractive') 'Edge script would prompt in a hidden process'
Assert (((Get-Content (Join-Path $testRoot 'data/unconverted.json') -Raw) -replace '\s','') -eq '[]') 'unexpected unconverted actions'
$kill = @($core.actions | Where-Object { $_.type -eq 'kill' -and $_.name -eq 'msedge' })[0]
Assert ($kill.ignore_errors -eq $true) 'ignoreErrors was dropped'
Assert (-not $core.actions[3].ignore_errors) 'ignoreErrors leaked onto the next action'
Assert ($edge.actions[0].ignore_errors -eq $true) 'errorAction: Ignore was dropped'
$inst = @($core.actions | Where-Object type -eq installer)
Assert ($inst.Count -eq 1 -and $inst[0].file -eq 'vcpp.exe' -and $inst[0].args -eq '/ai') 'shipped installer still goes through cmd.exe'
$od = @($core.actions | Where-Object { $_.type -eq 'command' -and $_.exe -like '*OneDriveSetup.exe' })
Assert ($od.Count -eq 2 -and $od[0].exe -eq '%windir%\SysWOW64\OneDriveSetup.exe' -and $od[1].args -eq '/uninstall') 'for-loop was not lowered to direct commands'
Assert (@($core.actions | Where-Object { $_.type -eq 'appx' -and $_.name -eq '*MicrosoftWindows.Client.WebExperience*' }).Count -eq 1) 'Get-AppxPackage pipeline still starts PowerShell'
Assert (@($core.actions | Where-Object { $_.type -eq 'powershell' -or ($_.type -eq 'command' -and $_.exe -eq 'cmd') }).Count -eq 0) 'a child shell is still used'
'PASS: 15 converter assertions'
# Remove only this test-owned directory after verifying its resolved scope.
$resolvedTest = [IO.Path]::GetFullPath($testRoot)
$expectedParent = [IO.Path]::GetFullPath($env:TEMP).TrimEnd('\') + '\'
if (-not $resolvedTest.StartsWith($expectedParent, [StringComparison]::OrdinalIgnoreCase) -or (Split-Path $resolvedTest -Leaf) -notlike 'AzizosConverterTest-*') { throw 'unsafe cleanup target' }
Remove-Item -LiteralPath $resolvedTest -Recurse -Force
