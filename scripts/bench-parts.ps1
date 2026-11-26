# The parts of a frame from a bench run's log: every 5 s block whose frame line shows world
# frames (draws >= -MinDraws), averaged. Each value is "ms a frame" as the log says it.
#   scripts\bench-parts.ps1 target\bench\base-1.log [-MinDraws 2000]
param([Parameter(Mandatory)][string[]]$Log, [int]$MinDraws = 2000)
$patterns = [ordered]@{
    frame_ms         = ' fps, ([\d.]+) ms/frame'
    prepare_ms       = 'ms/frame \(([\d.]+) preparing'
    copies_ms        = 'preparing: copies ([\d.]+) ms'
    back_ms          = '; ([\d.]+) copying back'
    draws            = '\), (\d+) draws/frame'
    ask_watch        = 'the submitting thread: ([\d.]+) ms asking the write watch'
    under_flights    = 'ask\), ([\d.]+) putting the title'
    list_steps       = 'writes, ([\d.]+) in the lists. steps'
    refreshes        = 'steps \(refreshes ([\d.]+) in'
    refresh_flushes  = 'refreshes [\d.]+ in (\d+) flushes'
    refresh_count    = 'flushes of (\d+),'
    refresh_nothing  = 'flushes of \d+, (\d+) of them known to copy nothing'
    snapshots        = '\), ([\d.]+) snapshots'
    card_copies      = 'snapshots, ([\d.]+) for copies in the card'
    sending          = 'memory, ([\d.]+) sending the lists'
    sub_lock         = 'ahead of these ([\d.]+) waiting for the submission lock'
    planning         = 'lock, ([\d.]+) planning steps'
    wait_flights     = 'card, ([\d.]+) waiting for flights'
    heap_swept       = 'flights, ([\d.]+) the heaps. records and what is swept'
    end_heap_wait    = 'at the end ([\d.]+) waiting for the heaps. records'
    heap_pass_slots  = 'the pass over the heap.s views per frame: ([\d.]+) ms looking at slots'
    heap_pass_spans  = 'looking at slots, ([\d.]+) their copies'
    heap_pass_tex    = 'and refreshes, ([\d.]+) textures'
    heap_pass_waited = 'VICTORY_HEAP_PASS_ASIDE\), ([\d.]+) ms waited for at the lists. close'
    records_ms       = 'whose record was new.*?, ([\d.]+) ms \([\d.]+ ms waited for\)'
    records_waited   = 'ms \(([\d.]+) ms waited for\)'
    lock_ranges      = 'ranges held \d+ ([\d.]+) ms'
    lock_journal_rd  = 'journal read \d+ ([\d.]+) ms'
    lock_journal_nt  = 'journal noted in \d+ ([\d.]+) ms'
    lock_guards      = 'guards \d+ ([\d.]+) ms$'
    behind_worked    = 'that thread worked ([\d.]+) ms'
    title_at_present = 'title waited [\d.]+ ms for room in the line, ([\d.]+) ms at presents'
    until_first_call = 'after a present ([\d.]+) ms until the first list call'
    lists_reset      = 'lists reset in ([\d.]+) ms'
    user_ms          = 'the process ran (\d+) ms user'
    kernel_ms        = 'ms user \+ (\d+) ms kernel'
    record_calls_ms  = 'list calls ([\d.]+) ms \(all recording'
    kernel_asks_ms   = 'kernel askings over \d+ MB \(([\d.]+) ms'
    guard_waits_ms   = 'askings refused\);.*?; ([\d.]+) ms waiting for the guards'
    protections_ms   = 'ms waiting for the guards, ([\d.]+) ms changing protections'
    relay_landing    = 'and was ([\d.]+) ms landing in all'
    land_merge       = 'merging what was fetched ([\d.]+) ms'
    ref_wall         = 'refreshes on several threads per frame: ([\d.]+) ms wall'
    ref_check        = 'threads. ([\d.]+) ms asking whether a span'
    ref_update       = 'nothing and ([\d.]+) ms updating the others'
    ref_spans        = 'the others; ([\d.]+) spans in'
    kernel_asks      = 'all threads: (\d+) kernel askings over'
    kernel_asks_mb   = 'kernel askings over (\d+) MB'
    pages_written    = 'ms, (\d+) pages of 4 KiB found'
    vq_queries       = '; (\d+) queries [\d.]+ ms, moves'
    vq_ms            = '; \d+ queries ([\d.]+) ms, moves'
    remap_moves_ms   = 'ms, moves ([\d.]+) ms'
    page_faults      = 'MB unmapped; (\d+) page faults'
    guard_faults     = 'fallbacks; (\d+) guard faults'
    blocks_readonly  = 'block by block; (\d+) blocks made read-only'
    host_lists_made  = 'ms; ([\d.]+) host lists made'
    host_lists_ms    = 'host lists made ([\d.]+) ms'
    record_calls     = 'recording per frame \(\d+ frames\): (\d+) list calls'
    thin_calls_ms    = 'VICTORY_THIN\): \d+ in ([\d.]+) ms'
    pipeline_wait    = 'threads\); ([\d.]+) ms waiting for pipelines'
    unmapped_mb      = 'ms, moves [\d.]+ ms, ([\d.]+) MB unmapped'
    card_idle_behind = 'ms \(the card about \d+% busy by this count\), ([\d.]+) ms of that while the submitting thread'
}
$all = @()
foreach ($file in $Log) {
    $block = @{}
    foreach ($line in [IO.File]::ReadLines((Resolve-Path $file))) {
        foreach ($k in $patterns.Keys) {
            $m = [regex]::Match($line, $patterns[$k])
            if ($m.Success -and -not $block.ContainsKey($k)) { $block[$k] = [double]::Parse($m.Groups[1].Value, [Globalization.CultureInfo]::InvariantCulture) }
        }
        if ($line -match 'victory_gpu::copystat: frame account per frame') {
            if ($block.ContainsKey('draws') -and $block['draws'] -ge $MinDraws) { $all += , $block }
            $block = @{}
        }
    }
}
"blocks: $($all.Count)"
foreach ($k in $patterns.Keys) {
    $xs = @($all | Where-Object { $_.ContainsKey($k) } | ForEach-Object { $_[$k] })
    if ($xs.Count) { '{0,-18} {1,9:N2}  (n {2})' -f $k, (($xs | Measure-Object -Average).Average), $xs.Count }
}



