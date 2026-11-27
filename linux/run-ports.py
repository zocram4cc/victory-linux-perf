"""Run fps20-ports MSVC build (Renderer Ports 1,2,5,6,7 + Page-Mask + keep_ref) and profile threads."""
import os
from pathlib import Path
import subprocess
import sys
import threading
import time

coord = Path("@SCRATCH@")
sys.path.insert(0, str(coord))
from room_run_headroom import run as headroom_run

profile = Path("@PROFILE@")
work = coord / "fps20-ports"
tag = sys.argv[1] if len(sys.argv) > 1 else "harbor-x-ports-smoke"
seed_name = sys.argv[2] if len(sys.argv) > 2 else "warm-console-x-compact-landing-union"
console = sys.argv[3] if len(sys.argv) > 3 else "series-x"
out = coord / "fps20-profile" / tag
out.mkdir(parents=True, exist_ok=True)
runner = coord / "fulladaptive-player"
seed = profile / seed_name
exe = work / "bin/victory.exe"
dest = profile / tag


def find_pids():
    found = {}
    for p in Path("/proc").iterdir():
        if not p.name.isdigit():
            continue
        try:
            env = (p / "environ").read_bytes()
            if tag.encode() not in env:
                continue
            comm = (p / "comm").read_text().strip()
            cmd = (p / "cmdline").read_bytes().replace(b"\x00", b" ").decode("utf-8", "ignore")
            if "victory.exe" in cmd.lower() and "run" in cmd.lower():
                found["game"] = int(p.name)
            elif comm == "wineserver":
                found["server"] = int(p.name)
        except Exception:
            continue
    return found


def snap_tasks(pid):
    rows = {}
    try:
        for t in (Path(f"/proc/{pid}/task")).iterdir():
            if not t.name.isdigit():
                continue
            try:
                comm = (t / "comm").read_text().strip()
                stat = (t / "stat").read_text()
                rp = stat.rfind(")")
                fields = stat[rp + 2:].split()
                rows[int(t.name)] = (comm, int(fields[11]), int(fields[12]))
            except Exception:
                continue
    except Exception:
        pass
    return rows


def watcher():
    log = dest / "logs/victory.log"
    deadline = time.monotonic() + 210
    while time.monotonic() < deadline:
        if log.exists() and "common present observer room view" in log.read_text(errors="replace"):
            break
        time.sleep(0.5)
    else:
        return
    time.sleep(0.5)
    pids = find_pids()
    if not pids:
        return
    t0 = {k: snap_tasks(pid) for k, pid in pids.items()}
    procs = []
    for k, pid in pids.items():
        try:
            (out / f"{k}.maps").write_text(Path(f"/proc/{pid}/maps").read_text())
            sf = (out / f"{k}.samples").open("w")
            ef = (out / f"{k}.stderr").open("w")
            procs.append((k, pid, sf, ef, subprocess.Popen([str(coord / "fps20-profile/sample-all-threads"), str(pid), "5"], stdout=sf, stderr=ef)))
        except Exception:
            pass
    for k, pid, sf, ef, pr in procs:
        pr.wait()
        sf.close()
        ef.close()
    t1 = {k: snap_tasks(pid) for k, pid in pids.items()}
    with (out / "thread-cpu.tsv").open("w") as f:
        f.write("kind\ttid\tcomm\tdelta_utime_ticks\tdelta_stime_ticks\tdelta_total_ticks\n")
        for k in pids:
            for tid, (comm, u1, s1) in t1[k].items():
                u0, s0 = t0[k].get(tid, (comm, u1, s1))[1:3]
                if (u1 - u0) + (s1 - s0) > 0:
                    f.write(f"{k}\t{tid}\t{comm}\t{u1-u0}\t{s1-s0}\t{(u1-u0)+(s1-s0)}\n")


env = dict(
    os.environ,
    HARBOR_PLAYER=str(work),
    HARBOR_SEED=str(seed),
    HARBOR_CONSOLE=console,
    VICTORY_PRESENT_OBSERVER="1",
    VICTORY_CARD_PAGE_MASK=os.environ.get("VICTORY_CARD_PAGE_MASK", "1"),
)
wt = threading.Thread(target=watcher, daemon=True)
wt.start()
command = [
    sys.executable,
    str(runner / "isolated-supervisor.py"),
    tag,
    str(exe),
    "--launcher",
    str(work / "launch-ports.sh"),
    "--seconds",
    str(int(os.environ.get("PORTS_WORLD_SECONDS", "90")) + 150),
    "--world-seconds",
    os.environ.get("PORTS_WORLD_SECONDS", "90"),
    "--memory-gib",
    "14",
    "--reserve-wait",
    "--capture-match",
    "common present observer room view",
]
# PORTS_LONG=1: the whole world time (steady state), not only the observer's window.
if os.environ.get("PORTS_LONG") != "1":
    command += ["--until", "common present observer completed"]
code = headroom_run(command, env, dest)
wt.join(timeout=15)
raise SystemExit(code)
