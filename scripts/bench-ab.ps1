# Per arm of an A/B batch: mean and spread (min..max) over its runs of each run's medians from
# target\bench\results.csv, n runs, and the change against the first arm; then the log counters
# averaged per arm (scripts\bench-parts.ps1 over each run's world blocks).
#   scripts\bench-ab.ps1 -Prefix ab1 -Arms base,guard,journal
param([Parameter(Mandatory)][string]$Prefix, [Parameter(Mandatory)][string[]]$Arms, [string]$Out = '',
    [string[]]$Parts = @('prepare_ms', 'list_steps', 'refreshes', 'end_heap_wait', 'records_ms', 'snapshots', 'ask_watch',
        'under_flights', 'card_copies', 'planning', 'sending', 'lock_guards', 'lock_journal_nt', 'ref_wall', 'ref_update', 'user_ms', 'kernel_ms'))
$repo = Resolve-Path (Join-Path $PSScriptRoot '..')
if (-not $Out) { $Out = Join-Path $repo 'target\bench' }
$rows = Import-Csv (Join-Path $Out 'results.csv') | Where-Object { $_.label -like "$Prefix-*" }
$inv = [Globalization.CultureInfo]::InvariantCulture
function Get-Num($v) { [double]::Parse(($v -replace ',', '.'), $inv) }
$metrics = 'frame_med', 'frame_p10', 'frame_p90', 'worst_per_s_med', 'prepare_med', 'prepare_p90', 'lock_wait_med', 'back_ms_med', 'copy_ms_med', 'draws_med'
$table = foreach ($arm in $Arms) {
    $r = @($rows | Where-Object { $_.label -eq "$Prefix-$arm" -and [int]$_.world_frames -gt 300 })
    $o = [ordered]@{ arm = $arm; n = $r.Count }
    foreach ($m in $metrics) {
        $xs = @($r | ForEach-Object { Get-Num $_.$m })
        if ($xs.Count) {
            $s = $xs | Measure-Object -Average -Minimum -Maximum
            $o[$m] = '{0:N1} ({1:N1}..{2:N1})' -f $s.Average, $s.Minimum, $s.Maximum
            $o["_$m"] = $s.Average
        }
    }
    [pscustomobject]$o
}
$base = $table[0]
"runs per arm and run medians: mean (min..max)"
$table | Select-Object arm, n, frame_med, frame_p90, worst_per_s_med, prepare_med, prepare_p90, lock_wait_med, back_ms_med | Format-Table -AutoSize | Out-String -Width 250
"change of the mean against $($base.arm):"
foreach ($t in $table | Select-Object -Skip 1) {
    $d = foreach ($m in 'frame_med', 'prepare_med', 'frame_p90', 'worst_per_s_med') {
        if ($t."_$m" -and $base."_$m") { '{0} {1:+0.0;-0.0}%' -f $m, (100 * ($t."_$m" / $base."_$m" - 1)) }
    }
    "  $($t.arm): $($d -join ', ')"
}
""
"log counters per arm (ms a frame unless counted), averaged over world blocks of all its runs:"
$parts = foreach ($arm in $Arms) {
    $logs = @(Get-ChildItem $Out -Filter "$Prefix-$arm-*.log" | ForEach-Object FullName)
    $text = if ($logs.Count) { & (Join-Path $PSScriptRoot 'bench-parts.ps1') -Log $logs } else { @() }
    $o = [ordered]@{ arm = $arm }
    foreach ($p in $Parts) {
        $m = $text | Select-String -Pattern "^$p\s+([\d.,]+)"
        $o[$p] = if ($m) { ($m.Matches[0].Groups[1].Value) } else { '' }
    }
    [pscustomobject]$o
}
$parts | Format-Table -AutoSize | Out-String -Width 300
