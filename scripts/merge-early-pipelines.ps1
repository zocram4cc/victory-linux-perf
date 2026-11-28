# Adds the pipelines a play's cache asked for to installer/early-pipelines.txt,
# the list the zip ships (the launchers add a changed one to each player's own
# cache list). Keys already listed stay where they are; new ones go at the end.
#   powershell -ExecutionPolicy Bypass -File scripts\merge-early-pipelines.ps1 -Cache "<cache folder>\pipelines\early-pipelines.txt"
# The launchers' cache folder is %LOCALAPPDATA%\victory-amd\cache.
param(
    [string]$Cache = (Join-Path $env:LOCALAPPDATA 'victory-amd\cache\pipelines\early-pipelines.txt'),
    [string]$List = (Join-Path $PSScriptRoot '..\installer\early-pipelines.txt')
)
$ErrorActionPreference = 'Stop'
$key = '^[0-9a-f]{32}$'
$shipped = [IO.File]::ReadAllText($List) -split "`r?`n" | Where-Object { $_ -match $key }
$played = [IO.File]::ReadAllText($Cache) -split "`r?`n" | Where-Object { $_ -match $key }
$seen = [Collections.Generic.HashSet[string]]::new()
$out = [Collections.Generic.List[string]]::new()
foreach ($k in @($shipped) + @($played)) { if ($seen.Add($k)) { $out.Add($k) } }
$added = $out.Count - @($shipped).Count
if ($added -gt 0) { [IO.File]::WriteAllText((Resolve-Path $List), ($out -join "`n") + "`n") }
"$added pipelines added; the list has $($out.Count)"
