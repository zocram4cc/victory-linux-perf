# One benchmark run: the title started with scripts\play.ps1's settings, scripted presses
# into the same scene, a per-frame record (VICTORY_FRAME_CSV) and the log, stopped after
# -Seconds; then the frames of a fixed window are summed up.
#   scripts\bench.ps1 -Exe <exe> -TitleDir <dump> -Label base -Run 1
#   scripts\bench.ps1 -Exe <exe> -TitleDir <dump> -Label cand -Run 1 -More @{VICTORY_X='1'}
# Only processes this script started are stopped (by process id).
param(
    [Parameter(Mandatory)][string]$Exe,
    [string]$TitleDir = '',
    [string]$Label = 'run',
    [int]$Run = 1,
    [int]$Seconds = 150,
    # The window of frames summed up, in seconds since the first present.
    [double]$From = 90,
    [double]$To = 140,
    # Frames with at least this many draws count as world frames.
    [int]$MinDraws = 100,
    [string]$Keys = '20:Enter,26:Enter,32:Enter,38:Enter,44:Enter,50:Enter',
    [string]$CaptureFrames = '',
    [hashtable]$More = @{},
    [string]$Out = ''
)
$repo = Resolve-Path (Join-Path $PSScriptRoot '..')
if (-not $Out) { $Out = Join-Path $repo 'target\bench' }
New-Item -ItemType Directory -Force $Out | Out-Null
$name = '{0}-{1}' -f $Label, $Run
$csv = Join-Path $Out "$name.csv"
$log = Join-Path $Out "$name.log"
$stdout = Join-Path $Out "$name.out"
Remove-Item $csv, $log, $stdout -ErrorAction SilentlyContinue

$settings = @{ VICTORY_FRAME_CSV = $csv; VICTORY_KEYS = $Keys; VICTORY_LOG = 'info' }
if ($TitleDir) { $settings.VICTORY_TITLE_DIR = $TitleDir }
if ($CaptureFrames) {
    $shots = Join-Path $Out "$name-shots"
    New-Item -ItemType Directory -Force $shots | Out-Null
    $settings.VICTORY_CAPTURE = $shots; $settings.VICTORY_CAPTURE_FRAMES = $CaptureFrames
}
foreach ($k in $More.Keys) { $settings[$k] = $More[$k] }
$pairs = ($settings.GetEnumerator() | ForEach-Object { "'{0}'='{1}'" -f $_.Key, ($_.Value -replace "'", "''") }) -join ';'
$paths = ''
foreach ($p in 'VICTORY_CONFIG', 'VICTORY_GAME_DIR', 'VICTORY_CACHE_DIR', 'VICTORY_DATA_DIR') {
    $v = [Environment]::GetEnvironmentVariable($p)
    if ($v) { $paths += "`$env:$p='$($v -replace "'", "''")';" }
}
$command = "$paths & '$repo\scripts\play.ps1' -Exe '$($Exe -replace "'", "''")' -More @{$pairs}"
$encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($command))
$started = Get-Date
$p = Start-Process powershell -ArgumentList '-NoProfile', '-EncodedCommand', $encoded -PassThru -WindowStyle Minimized `
    -RedirectStandardOutput $stdout -RedirectStandardError $log

# Every process under the one started here, by parent id.
function Get-Tree([int]$root) {
    $all = Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId, CreationDate
    $ids = @($root); $grew = $true
    while ($grew) {
        $more = $all | Where-Object { $ids -contains $_.ParentProcessId -and $ids -notcontains $_.ProcessId -and $_.CreationDate -ge $started.AddSeconds(-2) }
        $grew = [bool]$more
        $ids += @($more | ForEach-Object { [int]$_.ProcessId })
    }
    $ids
}
$tree = @()
$deadline = $started.AddSeconds($Seconds)
while ((Get-Date) -lt $deadline -and -not $p.HasExited) {
    Start-Sleep -Seconds 2
    $tree = @(@($tree) + @(Get-Tree $p.Id) | Select-Object -Unique)
}
$early = $p.HasExited
$tree = @(@($tree) + @(Get-Tree $p.Id) | Select-Object -Unique)
foreach ($id in ($tree | Sort-Object -Descending)) { Stop-Process -Id $id -Force -ErrorAction SilentlyContinue }
Start-Sleep -Seconds 3

# The window's world frames.
function Get-Quantile([double[]]$xs, [double]$q) {
    if (-not $xs.Count) { return [double]::NaN }
    $s = $xs | Sort-Object
    $i = ($s.Count - 1) * $q; $lo = [math]::Floor($i); $hi = [math]::Ceiling($i)
    $s[$lo] + ($s[$hi] - $s[$lo]) * ($i - $lo)
}
$rows = @()
if (Test-Path $csv) { $rows = @(Import-Csv $csv) }
$world = @($rows | Where-Object { [double]$_.t_ms -ge $From * 1000 -and [double]$_.t_ms -lt $To * 1000 -and [int]$_.draws -ge $MinDraws })
$col = { param($n) [double[]]@($world | ForEach-Object { [double]$_.$n }) }
$frame = & $col 'frame_ms'
$worst = [double[]]@($world | Group-Object { [math]::Floor([double]$_.t_ms / 1000) } | ForEach-Object { ($_.Group | ForEach-Object { [double]$_.frame_ms } | Measure-Object -Maximum).Maximum })
$summary = [ordered]@{
    label = $Label; run = $Run; started = $started.ToString('s'); early_exit = $early
    frames_all = $rows.Count; world_frames = $world.Count
    frame_med = Get-Quantile $frame 0.5; frame_p10 = Get-Quantile $frame 0.1; frame_p90 = Get-Quantile $frame 0.9
    worst_per_s_med = Get-Quantile $worst 0.5
    prepare_med = Get-Quantile (& $col 'prepare_ms') 0.5; prepare_p10 = Get-Quantile (& $col 'prepare_ms') 0.1; prepare_p90 = Get-Quantile (& $col 'prepare_ms') 0.9
    gpu_wait_med = Get-Quantile (& $col 'gpu_wait_ms') 0.5
    copy_ms_med = Get-Quantile (& $col 'copy_ms') 0.5; copy_mb_med = Get-Quantile (& $col 'copy_mb') 0.5
    texture_ms_med = Get-Quantile (& $col 'texture_ms') 0.5
    back_ms_med = Get-Quantile (& $col 'back_ms') 0.5; back_mb_med = Get-Quantile (& $col 'back_mb') 0.5
    lock_wait_med = Get-Quantile (& $col 'lock_wait_ms') 0.5
    draws_med = Get-Quantile (& $col 'draws') 0.5
}
$line = [pscustomobject]$summary
$results = Join-Path $Out 'results.csv'
$line | Export-Csv -Append -NoTypeInformation $results
$line | Format-List | Out-String | Write-Host
