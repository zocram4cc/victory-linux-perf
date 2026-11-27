#!/usr/bin/env python3
"""leg-index.py <captures-root>: writes <root>/index.html from the legs that
have a label.txt (made by leg.sh).

Per leg: its label, binary, seed and switches; tiles of what it measured
(read from its victory.log once and kept in stats.json); the blink
measurement; the four captures. On top: a ranking, fastest first, each leg
against the reference (the newest leg whose label starts with REFERENCE).
verdict.txt in a leg's folder, if there, is shown as its verdict; a first
word of BETTER, WORSE or SAME colours it.
"""
import html
import json
import re
import statistics as st
import datetime as dt
import sys
from pathlib import Path

root = Path(sys.argv[1])
legs = sorted((p.parent for p in root.glob('*/label.txt')), key=lambda d: (d / 'label.txt').stat().st_mtime, reverse=True)
FRAME = re.compile(
    r'host: ([0-9.]+) fps, ([0-9.]+) ms/frame \(([0-9.]+) preparing: copies ([0-9.]+) ms.*?([0-9.]+) copying back.*?'
    r', (\d+) draws/frame, dropped (\d+) without a host pipeline \((\d+) being made\)(?: and (\d+) indirect)?')
BUSY = re.compile(r"the card about (\d+)% busy")


def read(p):
    return p.read_text(errors='replace').strip() if p.exists() else ''


def stats_of(d):
    """What a leg measured, from its log; kept beside it once the leg is over."""
    kept = d / 'stats.json'
    log = d / 'victory.log'
    if kept.exists() and (not log.exists() or kept.stat().st_mtime >= log.stat().st_mtime):
        s = json.loads(kept.read_text())
        if 'timing' in s or not log.exists():
            return s
    if not log.exists():
        return {}
    rows, room_busy = [], []
    last_draws = 0
    text = log.read_text(errors='replace')
    for line in text.splitlines():
        m = FRAME.search(line)
        if m:
            rows.append([float(x or 0) for x in m.groups()])
            last_draws = rows[-1][5]
            continue
        b = BUSY.search(line)
        if b and last_draws > 3000:
            room_busy.append(int(b.group(1)))
    room = [r for r in rows if r[5] > 3000]
    room = room[2:] or room
    phone = [r for r in rows if 1000 < r[5] <= 3000]
    if not room:
        s = {'world': False}
    else:
        med = lambda rs, i: st.median(r[i] for r in rs)
        ms = med(room, 1)
        s = {
            'world': True,
            'ms': ms, 'fps': 1000 / ms,
            'fps_low': 1000 / max(r[1] for r in room), 'fps_high': 1000 / min(r[1] for r in room),
            'reports': len(room),
            'copies': med(room, 3), 'back': med(room, 4), 'preparing': med(room, 2),
            'draws': med(room, 5), 'dropped': med(room, 6), 'making': med(room, 7), 'refused': med(room, 8),
            'phone_ms': med(phone, 1) if phone else None,
            'busy': st.median(room_busy) if room_busy else None,
        }
    s['lost'] = len(re.findall(r'device removed|panicked', text))
    s['timing'] = timing_of(d, text)
    kept.write_text(json.dumps(s))
    return s


STAMP = re.compile(r'^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d+)Z')


def timing_of(d, text):
    """Seconds from the leg's start to: the layer's first log line (Wine and
    the prefix are up), the first frame presented, the first frame of the
    world (500 draws or more) and the first five-second report within 15 %
    of the leg's median frame time."""
    def at(line):
        m = STAMP.match(line)
        return dt.datetime.fromisoformat(m.group(1)).replace(tzinfo=dt.timezone.utc) if m else None
    first = presented = world = steady = None
    reports = []
    for line in text.splitlines():
        when = at(line)
        if when is None:
            continue
        first = first or when
        if presented is None and 'frame presented' in line and 'frame=1 ' in line:
            presented = when
        if world is None and 'common present observer origin' in line:
            world = when
        m = FRAME.search(line)
        if m and world is not None and float(m.group(6)) > 3000:
            reports.append((when, float(m.group(2))))
    if reports:
        median = st.median(ms for _, ms in reports)
        steady = next((when for when, ms in reports if ms <= median * 1.15), None)
    label = dict(l.split(': ', 1) for l in read(d / 'label.txt').splitlines() if ': ' in l)
    try:
        start = dt.datetime.strptime(label['when'], '%Y-%m-%d %H:%M:%S').astimezone()
    except (KeyError, ValueError):
        start = None
    since = lambda when: round((when - start).total_seconds(), 1) if (when and start) else None
    return {'log': since(first), 'frame': since(presented), 'world': since(world), 'steady': since(steady)}


def blink_of(d):
    lines = read(d / 'flicker.txt').splitlines()
    f = re.search(r'mean difference median ([0-9.]+) max ([0-9.]+); pixels over 24: median ([0-9.]+) % max ([0-9.]+)', lines[0] if lines else '')
    return tuple(float(x) for x in f.groups()) if f else None


def tile(name, value, unit='', note='', tone=''):
    return (f'<div class="tile {tone}"><div class="k">{html.escape(name)}</div>'
            f'<div class="v">{html.escape(str(value))}<span>{html.escape(unit)}</span></div>'
            f'<div class="note">{html.escape(note)}</div></div>')


def timing_tile(tm):
    if tm.get('world') is None:
        return tile('Start to the world', '–', '', 'not timed')
    parts = []
    if tm.get('frame') is not None:
        parts.append(f"first frame at {tm['frame']:.0f} s")
        parts.append(f"loading {tm['world'] - tm['frame']:.0f} s")
    if tm.get('steady') is not None:
        parts.append(f"steady at {tm['steady']:.0f} s")
    return tile('Start to the world', f"{tm['world']:.0f}", 's', ', '.join(parts))


def num(x, digits=1):
    return '–' if x is None else f'{x:.{digits}f}'


data = []
for d in legs:
    label = dict(l.split(': ', 1) for l in read(d / 'label.txt').splitlines() if ': ' in l)
    data.append({'dir': d, 'label': label, 'stats': stats_of(d), 'blink': blink_of(d), 'verdict': read(d / 'verdict.txt')})
reference = next((x for x in data if x['label'].get('label', '').startswith('REFERENCE') and x['stats'].get('world')), None)


def view(s):
    return ('A' if s['draws'] < 7800 else 'B') if s.get('world') else '–'


def delta(s):
    """Against the reference, when both are in one view."""
    if not reference or not s.get('world') or view(s) != view(reference['stats']):
        return None
    return (s['fps'] / reference['stats']['fps'] - 1) * 100


def captions(label):
    """The phone legs' four names, or the seconds in the world of other legs."""
    shots = label.get('shots', '28,40,58,68')
    if shots == '28,40,58,68' and label.get('keys', '92:Up') == '92:Up':
        return ['Room, early', 'Room, later', 'Phone up', 'Phone up, later']
    return [f'{s.strip()} s in the world' for s in shots.split(',')]


def tone_of(verdict):
    word = verdict.split(' ', 1)[0].strip(':,.').upper() if verdict else ''
    return {'BETTER': 'good', 'WORSE': 'bad', 'SAME': 'flat'}.get(word, '')


cards, rank = [], []
for x in data:
    d, label, s, blink, verdict = x['dir'], x['label'], x['stats'], x['blink'], x['verdict']
    name = html.escape(label.get('tag', d.name))
    is_ref = reference is not None and x is reference
    dl = delta(s)
    if is_ref:
        badge = '<span class="chip ref">reference</span>'
    elif dl is not None:
        badge = f'<span class="chip {"good" if dl >= 3 else "bad" if dl <= -3 else "flat"}">{dl:+.0f} % fps</span>'
    else:
        badge = ''
    if s.get('world'):
        phone_note = f"{num(1000 / s['phone_ms'])} fps" if s.get('phone_ms') else 'not reached'
        blink_tone = ('bad' if blink[0] > 4 else 'good' if blink[0] < 1.5 else '') if blink else ''
        blink_note = f"{num(blink[2])} % of pixels (still: under 1.5 / 2 %)" if blink else 'not measured'
        tiles = ''.join([
            tile('Frame rate, room', num(s['fps']), 'fps', f"{num(s['fps_low'])}–{num(s['fps_high'])} over {s['reports']} reports"),
            tile('Frame time', num(s['ms']), 'ms', f"view {view(s)}, {s['draws']:.0f} draws"),
            tile('Phone up', num(s['phone_ms']), 'ms', phone_note),
            tile('Copies up / back', f"{num(s['copies'])} / {num(s['back'])}", 'ms', f"preparing {num(s['preparing'])} ms"),
            tile('Card busy', num(s['busy'], 0), '%', 'by our queue'),
            tile('Draws dropped', f"{s['dropped']:.0f}", '', f"{s['making']:.0f} pipelines being made", 'bad' if s['dropped'] > 400 else ''),
            tile('Indirect refused', f"{s['refused']:.0f}", '', 'records a frame'),
            tile('Blinking', num(blink[0], 2) if blink else '–', '', blink_note, blink_tone),
            tile('Device lost', s.get('lost', 0), '', 'lines in the log', 'bad' if s.get('lost') else ''),
            timing_tile(s.get('timing') or {}),
        ])
    elif s:
        tiles = tile('World', 'not reached', '', f"device-lost or panic lines: {s.get('lost', 0)}", 'bad')
    else:
        tiles = tile('Leg', 'running', '', 'no log yet')
    shots = ''.join(
        f'<figure><a href="{d.name}/f{n}.jpg" target="_blank"><img loading="lazy" src="{d.name}/f{n}.jpg" alt="{cap}"></a>'
        f'<figcaption>{cap}</figcaption></figure>'
        for n, cap in enumerate(captions(label))
        if (d / f'f{n}.jpg').exists())
    compare = (f'<figure class="compare"><a href="{d.name}/compare.jpg" target="_blank"><img loading="lazy" src="{d.name}/compare.jpg" alt="comparison"></a>'
               f'<figcaption>Comparison. Top: the floor before and in this leg. Bottom: the phone before, and twice in this leg</figcaption></figure>') if (d / 'compare.jpg').exists() else ''
    clip = ''
    if (d / 'video.mp4').exists():
        clip = (f'<figure class="clip"><video controls preload="none" poster="{d.name}/f0.jpg" src="{d.name}/video.mp4"></video>'
                f'<figcaption>Recording, from {html.escape(read(d / "video.txt") or "the room")}</figcaption></figure>')
    if verdict:
        verdict_html = f'<p class="verdict {tone_of(verdict)}">{html.escape(verdict)}</p>'
    else:
        verdict_html = '<p class="verdict none">No verdict written yet.</p>'
    cards.append(
        f'<article id="{html.escape(d.name)}" class="leg {"is-ref" if is_ref else ""}">'
        f'<header><h2>{name}</h2>{badge}<time>{html.escape(label.get("when", ""))}</time></header>'
        f'<p class="what">{html.escape(label.get("label", ""))}</p>'
        f'{verdict_html}'
        f'<div class="tiles">{tiles}</div>'
        f'<div class="shots">{shots or "<p class=empty>No captures.</p>"}</div>'
        f'{compare}{clip}'
        f'<dl class="meta"><dt>Binary</dt><dd>{html.escape(label.get("binary", ""))}</dd>'
        f'<dt>Seed</dt><dd>{html.escape(label.get("seed", ""))}</dd>'
        f'<dt>Switches</dt><dd><code>{html.escape(label.get("switches", ""))}</code></dd></dl></article>')
    rank.append((s.get('fps', -1), x, dl))

best = max((r[0] for r in rank), default=1)
best = best if best > 0 else 1
rows = []
for n, (fps, x, dl) in enumerate(sorted(rank, key=lambda r: -r[0]), 1):
    s, blink = x['stats'], x['blink']
    name = html.escape(x['dir'].name)
    bar = f'<span class="bar"><i style="width:{max(2, 100 * fps / best):.0f}%"></i></span>' if fps > 0 else ''
    ref_chip = ' <span class="chip ref">ref</span>' if x is reference else ''
    sub = html.escape((x['verdict'] or x['label'].get('label', ''))[:150])
    world = s.get('world')
    rows.append(
        f'<tr><td class="n">{n}</td><td><a href="#{name}">{name}</a>{ref_chip}<div class="sub">{sub}</div></td>'
        f'<td class="n big">{num(s.get("fps")) if world else "–"}{bar}</td>'
        f'<td class="n">{f"{dl:+.0f} %" if dl is not None else "–"}</td><td class="n">{view(s)}</td>'
        f'<td class="n">{num(s.get("phone_ms")) if world else "–"}</td>'
        f'<td class="n">{int(s["dropped"]) if world else "–"}</td>'
        f'<td class="n">{num(blink[0], 2) if blink else "–"}</td><td class="n">{s.get("lost", "–")}</td></tr>')

page = f'''<!doctype html><html lang="en"><meta charset="utf-8"><meta http-equiv="refresh" content="60">
<meta name="viewport" content="width=device-width,initial-scale=1"><title>Legs of the day</title>
<style>
:root{{--bg:#101216;--panel:#181b21;--line:#272b33;--ink:#e8eaee;--dim:#98a0ad;--accent:#7aa2ff;--good:#4cc38a;--bad:#ff6b6b;--flat:#c9a227}}
@media(prefers-color-scheme:light){{:root{{--bg:#f4f5f7;--panel:#fff;--line:#dfe2e8;--ink:#16181d;--dim:#5d6675;--accent:#2f5fd0;--good:#16865a;--bad:#c93838;--flat:#8a6d00}}}}
*{{box-sizing:border-box}} body{{margin:0;background:var(--bg);color:var(--ink);font:15px/1.5 system-ui,-apple-system,"Segoe UI",sans-serif}}
main{{max-width:1500px;margin:0 auto;padding:20px 16px 60px}}
.top{{display:flex;flex-wrap:wrap;align-items:baseline;gap:8px 18px;margin-bottom:6px}} h1{{font-size:22px;margin:0;letter-spacing:-.01em}}
.top p{{margin:0;color:var(--dim)}} .legend{{color:var(--dim);font-size:13px;margin:0 0 14px;max-width:110ch}}
.panel,.leg{{background:var(--panel);border:1px solid var(--line);border-radius:12px}}
.panel{{padding:6px 14px 10px;margin-bottom:22px;overflow-x:auto}}
table{{border-collapse:collapse;width:100%;min-width:760px}} th,td{{text-align:left;padding:8px 10px;border-bottom:1px solid var(--line);vertical-align:top}}
tr:last-child td{{border-bottom:0}} th{{color:var(--dim);font-weight:500;font-size:12px;text-transform:uppercase;letter-spacing:.04em}}
td.n{{font-variant-numeric:tabular-nums;white-space:nowrap}} td.big{{font-weight:650;font-size:17px;min-width:150px}}
.bar{{display:block;height:4px;background:var(--line);border-radius:2px;margin-top:5px}} .bar i{{display:block;height:100%;background:var(--accent);border-radius:2px}}
.sub{{color:var(--dim);font-size:13px}} a{{color:var(--accent);text-decoration:none}} a:hover{{text-decoration:underline}}
.leg{{padding:16px 18px 14px;margin-bottom:18px}} .leg.is-ref{{border-color:var(--accent)}}
.leg header{{display:flex;flex-wrap:wrap;align-items:center;gap:6px 10px}} h2{{font-size:18px;margin:0}} time{{color:var(--dim);font-size:13px;margin-left:auto}}
.chip{{font-size:12px;padding:2px 9px;border-radius:99px;border:1px solid var(--line);color:var(--dim);white-space:nowrap}}
.chip.ref{{color:var(--accent);border-color:var(--accent)}} .chip.good{{color:var(--good);border-color:var(--good)}}
.chip.bad{{color:var(--bad);border-color:var(--bad)}} .chip.flat{{color:var(--flat);border-color:var(--flat)}}
.what{{margin:6px 0 8px;font-size:16px}}
.verdict{{margin:0 0 12px;padding:8px 12px;border-left:3px solid var(--accent);background:color-mix(in srgb,var(--accent) 10%,transparent);border-radius:0 8px 8px 0}}
.verdict.good{{border-color:var(--good);background:color-mix(in srgb,var(--good) 12%,transparent)}}
.verdict.bad{{border-color:var(--bad);background:color-mix(in srgb,var(--bad) 12%,transparent)}}
.verdict.flat{{border-color:var(--flat);background:color-mix(in srgb,var(--flat) 12%,transparent)}}
.verdict.none{{border-color:var(--line);background:none;color:var(--dim)}}
.tiles{{display:grid;grid-template-columns:repeat(auto-fill,minmax(150px,1fr));gap:8px;margin-bottom:12px}}
.tile{{border:1px solid var(--line);border-radius:10px;padding:9px 11px;min-width:0}}
.tile .k{{color:var(--dim);font-size:12px}} .tile .v{{font-size:22px;font-weight:650;font-variant-numeric:tabular-nums;line-height:1.25}}
.tile .v span{{font-size:13px;font-weight:500;color:var(--dim);margin-left:4px}} .tile .note{{color:var(--dim);font-size:12px}}
.tile.good .v{{color:var(--good)}} .tile.bad .v{{color:var(--bad)}}
.shots{{display:grid;grid-template-columns:repeat(auto-fill,minmax(300px,1fr));gap:8px;margin-bottom:10px}} figure{{margin:0;min-width:0}}
img{{width:100%;display:block;border-radius:8px;border:1px solid var(--line);aspect-ratio:16/9;object-fit:cover;background:var(--bg)}}
figcaption{{color:var(--dim);font-size:12px;margin-top:3px}} .empty{{color:var(--dim);margin:0}}
.clip{{margin:0 0 10px;max-width:760px}} video{{width:100%;display:block;border-radius:8px;border:1px solid var(--line);background:#000}}
.compare{{margin:0 0 10px}} .compare img{{aspect-ratio:auto;object-fit:contain}}
.meta{{display:grid;grid-template-columns:80px 1fr;gap:2px 12px;margin:0;font-size:13px;color:var(--dim)}} .meta dd{{margin:0;overflow-wrap:anywhere}}
code{{font:12px/1.4 ui-monospace,SFMono-Regular,Menlo,monospace}}
@media(max-width:900px){{.shots{{grid-template-columns:repeat(2,1fr)}} time{{margin-left:0;width:100%}}}}
</style>
<main>
<div class="top"><h1>Legs of the day</h1><p>{len(cards)} legs · reloads every minute</p></div>
<p class="legend">Every leg is the same scripted start into the room, Series X profile at 3840x2160 on the RX 7900 XTX. The start lands in view A (about 6,500 draws) or the heavier view B (about 9,000); the change against the reference is given only within one view. Blinking is the mean change between quick captures while nothing moves.</p>
<div class="panel"><table><tr><th>#</th><th>Leg</th><th>fps, room</th><th>vs reference</th><th>View</th><th>Phone up, ms</th><th>Draws dropped</th><th>Blinking</th><th>Device lost</th></tr>{''.join(rows)}</table></div>
{''.join(cards)}
</main></html>
'''
(root / 'index.html').write_text(page)
print(f'index: {root / "index.html"} ({len(cards)} legs)')
