from pathlib import Path
import argparse
import csv
import datetime
import fcntl
import os
import re
import subprocess
import time

RESULTS = Path('@PROFILE@')

def live_titles():
    for proc in Path('/proc').iterdir():
        if not proc.name.isdecimal():
            continue
        try:
            args = (proc / 'cmdline').read_bytes().split(b'\0')
            if args and b'run' in args and any(a.lower().endswith(b'game_scarlett_master.exe') for a in args):
                yield proc.name
        except OSError:
            continue

def read_integer(path):
    try:
        return int(path.read_text())
    except (OSError, ValueError):
        return -1

def pressure_total(path):
    try:
        line = path.read_text().splitlines()[0]
        return int(line.rsplit('total=', 1)[1])
    except (OSError, ValueError, IndexError):
        return -1

def cpu_total(path):
    try:
        return int(path.read_text().splitlines()[0].split()[1])
    except (OSError, ValueError, IndexError):
        return -1

def host_memory():
    return {s[0]: int(s[1]) for s in re.findall(r'^(MemAvailable|SwapFree):\s+(\d+)', Path('/proc/meminfo').read_text(), re.M)}

def run(args):
    path = RESULTS / args.tag
    scope = 'cgbj-' + args.tag + '.scope'
    cg = Path('/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice') / scope
    log = path / 'logs/victory.log'
    gpu = next(p for p in Path('/sys/class/drm').glob('card*/device') if (p/'gpu_busy_percent').is_file() and (p/'vendor').read_text().strip()=='0x1002')
    active = list(live_titles())
    if path.exists() or cg.exists() or active:
        raise SystemExit(f'Existing path, scope or title. Active title PIDs: {active}')
    if host_memory()['MemAvailable'] < (args.memory_gib+2) * 1024**2:
        raise SystemExit('Host has insufficient available RAM for the selected scope limit')
    from capture import save
    player=args.exe.parent.parent
    os.environ['VICTORY_RUNTIME']=str(path)
    cmd=['systemd-run','--user','--scope','--unit='+scope,'-p',f'MemoryMax={args.memory_gib}G','-p','MemorySwapMax=256M','--',str(args.launcher or player/'play.sh'),'--seconds',str(args.seconds)]
    captured=False
    capture_marked=False
    second_saved = None
    position = 0
    rest = ''
    until = re.compile(args.until) if args.until else None
    capture_match = re.compile(args.capture_match) if args.capture_match else None
    reason = None
    world = None
    started = time.monotonic()
    with (RESULTS / f'{args.tag}-system.tsv').open('x', buffering=1) as f:
        output = csv.writer(f,delimiter='\t')
        output.writerow(['utc','host_avail_kib','swap_free_kib','scope_bytes','scope_swap_bytes','gpu_busy_pct','vram_bytes','scope_oom_kills',
                        'host_cpu_stall_us','host_memory_stall_us','scope_cpu_stall_us','scope_memory_stall_us','scope_cpu_total_us'])
        process = subprocess.Popen(cmd)
        try:
            while process.poll() is None:
                t = time.monotonic()
                memory = host_memory()
                events = cg / 'memory.events'
                try:
                    oom = re.search(r'^oom_kill (\d+)', events.read_text(), re.M)[1]
                except FileNotFoundError:
                    oom = ''
                output.writerow([datetime.datetime.now(datetime.timezone.utc).isoformat(),
                                 memory['MemAvailable'],memory['SwapFree'],read_integer(cg/'memory.current'),
                                 read_integer(cg/'memory.swap.current'),read_integer(gpu/'gpu_busy_percent'),
                                 read_integer(gpu/'mem_info_vram_used'),oom,
                                 pressure_total(Path('/proc/pressure/cpu')), pressure_total(Path('/proc/pressure/memory')),
                                 pressure_total(cg/'cpu.pressure'), pressure_total(cg/'memory.pressure'),
                                 cpu_total(cg/'cpu.stat')])
                if memory['MemAvailable'] < float(os.environ.get('SUPERVISOR_RESERVE_GIB', '3'))*1024**2:
                    reason = 'host RAM reserve reached'
                if log.exists():
                    with log.open(errors='replace') as handle:
                        handle.seek(position)
                        new = handle.read()
                        position = handle.tell()
                    lines = (rest+new).split('\n')
                    rest = lines.pop()
                    for line in lines:
                        draws = re.search(r', (\d+) draws/frame, dropped', line) if world is None else None
                        active_world = os.getenv('FORK_VISUAL_RUN') != '1' and draws and int(draws[1]) >= 500
                        if world is None and ('the first frame of 500 draws or more' in line or active_world):
                            world = t
                            print(line,flush=True)
                        if until and until.search(line):
                            reason = reason or 'requested diagnostic marker'
                        if capture_match and capture_match.search(line):
                            capture_marked = True
                capture_due = capture_marked if capture_match else world is not None and t-world >= args.capture_at
                if world is not None and not captured and capture_due:
                    try:
                        save(path)
                        captured=True
                    except Exception as exc:
                        print(f"Window capture unavailable: {exc}",flush=True)
                        captured=True
                if world is not None and args.world_seconds and t-world >= args.world_seconds:
                    reason = reason or 'requested world duration'
                if len(list((path / 'frames').glob('frame-*.png'))) >= 3:
                    second_saved = second_saved or t
                    if t - second_saved >= 5:
                        reason = reason or 'completed room captures'
                if t-started > max(270, args.seconds + 8):
                    reason = reason or 'overall deadline'
                if reason:
                    print(reason,flush=True)
                    break
                time.sleep(0.25)
        finally:
            if process.poll() is None or cg.exists():
                subprocess.run(['systemctl','--user','stop','--no-block',scope],check=False,timeout=6)
                time.sleep(0.6)
                if cg.exists():
                    subprocess.run(['systemctl','--user','kill','--signal=KILL',scope],check=False,timeout=6)
            code = process.wait(timeout=15)
        print(f'exit={code} world={world is not None} reason={reason}',flush=True)
    if reason is None and code:
        raise SystemExit(code)
    if world is None:
        raise SystemExit(4)

if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('tag')
    parser.add_argument('exe', type=lambda x: Path(x).resolve(strict=True))
    parser.add_argument('--launcher', type=Path)
    parser.add_argument('--reserve-wait', action='store_true')
    parser.add_argument('--seed', default='fxport-after-visual')
    parser.add_argument('--mode', choices=('warm','reuse'), default='warm')
    parser.add_argument('--seconds', type=int, default=110)
    parser.add_argument('--world-seconds', type=int, default=0)
    parser.add_argument('--capture-at', type=int, default=36)
    parser.add_argument('--capture-match')
    parser.add_argument('--until')
    parser.add_argument('--memory-gib', type=int, choices=range(12, 17), default=14)
    args = parser.parse_args()
    assert re.fullmatch(r'[a-z][a-z0-9-]{1,55}',args.tag)
    assert re.fullmatch(r'[a-z][a-z0-9-]{1,55}',args.seed)
    # The production launcher's own shell holds the single-instance lock.
    with open('@SCRATCH@/game.lock', 'ab', buffering=0) as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        os.environ['SMOKE_SUPERVISOR_LOCK'] = '1'
        # Hold the reservation through teardown and preflight. A previous
        # scope may still be releasing its memory as the lock becomes free.
        reserve = (args.memory_gib + 2) * 1024**2
        next_progress = 0
        settled = time.monotonic() + 9
        while (args.reserve_wait or time.monotonic() < settled) and (list(live_titles()) or host_memory()['MemAvailable'] < reserve):
            if time.monotonic() >= next_progress:
                available = host_memory()['MemAvailable'] / 1024**2
                print(f'Preflight for {args.tag}: {available:.2f} GiB available, {args.memory_gib + 2} GiB needed', flush=True)
                next_progress = time.monotonic() + 30
            time.sleep(3)
        run(args)
