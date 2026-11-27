"""victory-legs.py on made-up run folders.  python3 -I -m unittest discover -s scripts/tests"""

import importlib.util
import io
import json
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("legs", HERE.parent / "victory-legs.py")
legs = importlib.util.module_from_spec(spec)
spec.loader.exec_module(legs)

HEADER = "frame,t_ms,frame_ms,draws,draws_dropped,pipelines_queued,pipelines_made,vram_used_mb\n"


def make_run(root, name, *, route="intro", env=None, frame_ms=40.0, draws=5000, seconds=120,
             hitches=0, log="", label=None, later_draws=None):
    d = root / name
    d.mkdir(parents=True)
    (d / "run.json").write_text(json.dumps({
        "started_epoch": 1.0, "profile": "release", "git_rev": "abc1234", "git_dirty": False,
        "bench": route, "exit_code": 0, "seconds": seconds,
        "env": {"VICTORY_RELAY": "2", "VICTORY_FRAMES_CSV": f"Z:\\{name}", **(env or {})}}))
    rows, t, n = [HEADER, "0,0.0,0.00,0,0,0,0,0\n"], 0.0, 1
    while t < seconds * 1000:
        ms = 400.0 if n <= hitches else frame_ms
        t += ms
        dr = later_draws if later_draws is not None and t > 60000 else draws
        rows.append(f"{n},{t:.1f},{ms:.2f},{dr},0,0,0,0\n")
        n += 1
    (d / "frames.csv").write_text("".join(rows))
    (d / "victory.log").write_text(log)
    if label:
        (d / "leg.json").write_text(json.dumps({"label": label, "exe_sha256": "f" * 64}))
    return d


def run(args):
    out = io.StringIO()
    with redirect_stdout(out):
        code = legs.main([str(a) for a in args])
    return code, out.getvalue()


LOG = ("12:00:00 \x1b[33mWARN\x1b[0m victory_gpu::draw: a texture picked by index changed after its upload "
       "with nothing to tell of it: uploaded again\n"
       "12:00:01 INFO victory_gpu::draw: a draw of the title's decoration bake: textures picked by index are "
       "uploaded synchronously first frame=9 bakes=3 queued_at_the_last_pass=1 uploaded_so_far=70\n"
       "12:00:02 INFO victory_gpu::draw: a draw of the title's decoration bake: textures picked by index are "
       "uploaded synchronously first frame=99 bakes=512 queued_at_the_last_pass=1 uploaded_so_far=779592\n"
       "12:00:03 ERROR wine: unhandled exception 0xc0000005\n")


class LegsTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def test_show_switches_faults_and_unviewed(self):
        a = make_run(self.root, "a", log=LOG, hitches=3)
        b = make_run(self.root, "b", env={"VICTORY_TEXTURE_NO_WAIT": "0", "VICTORY_RELAY": "1"}, label="nowait0")
        _, out = run(["show", a, b])
        lines = out.splitlines()
        self.assertIn("changed_unannounced=1", lines[1])
        self.assertIn("exception=1", lines[1])
        self.assertIn("bakes=512 uploaded=779592", lines[1])
        self.assertIn("3 over 250 ms", lines[1])
        self.assertIn("(unviewed)", lines[1])
        b_line = next(l for l in lines if l.startswith("nowait0 |"))
        self.assertIn("+VICTORY_RELAY=1 +VICTORY_TEXTURE_NO_WAIT=0", b_line)
        self.assertIn("sha ffffffffffff", b_line)
        self.assertNotIn("FRAMES_CSV", out)

    def test_capped_line_is_a_lower_bound(self):
        line = ("WARN victory_gpu::draw: a texture picked by index changed after its upload with nothing "
                "to tell of it: uploaded again looks=4 changed={}\n")
        capped = "".join(line.format(n) for n in range(1, 201)) + line.format(3000)
        a = [make_run(self.root, f"a{i}", log=capped) for i in range(4)]
        b = [make_run(self.root, f"b{i}", log=line.format(1) * 5) for i in range(4)]
        _, out = run(["show", a[0], b[0]])
        self.assertIn("changed_unannounced=3000+", out)
        self.assertIn("changed_unannounced=5 ", out)
        _, out = run(["groups", "A=" + ",".join(map(str, a)), "B=" + ",".join(map(str, b))])
        self.assertIn("changed_unannounced: A 3000..3000 against B 5..5: unknown", out)

    def test_viewed_list_removes_the_mark(self):
        a = make_run(self.root, "a")
        viewed = self.root / "viewed.txt"
        viewed.write_text(f"# looked at\n{a}\n")
        _, out = run(["--viewed", viewed, "show", a])
        self.assertNotIn("(unviewed)", out.splitlines()[1])

    def test_compare_only_same_view(self):
        ref = make_run(self.root, "ref", draws=5000, frame_ms=40)
        same = make_run(self.root, "same", draws=5100, frame_ms=50)       # +2 % draws
        other = make_run(self.root, "other", draws=5000, later_draws=5400)  # +8 % after 60 s
        route = make_run(self.root, "route", route="resume")
        _, out = run(["compare", "--reference", ref, same, other, route])
        first = next(l for l in out.splitlines() if l.strip().startswith("0-60"))
        self.assertIn("same: +10.0 ms (+25 %)", first)
        self.assertIn("other: +0.0 ms", first)
        self.assertIn("route: not comparable (route resume against intro)", first)
        second = next(l for l in out.splitlines() if l.strip().startswith("60-120"))
        self.assertIn("other: not comparable (draws 5400 against 5000 (+8 %))", second)

    def test_compare_groups_give_spread(self):
        ref = make_run(self.root, "ref", frame_ms=40)
        runs = [make_run(self.root, f"b{i}", frame_ms=40 + 2 * i) for i in range(3)]
        _, out = run(["compare", "--reference", ref, "B=" + ",".join(map(str, runs))])
        first = next(l for l in out.splitlines() if l.strip().startswith("0-60"))
        self.assertIn("B: +2.0 ms (+5 %) [+0.0..+4.0, 3/3 comparable]", first)

    def test_groups_refuse_under_four_starts(self):
        a = [make_run(self.root, f"a{i}", hitches=i) for i in range(4)]
        b = [make_run(self.root, f"b{i}", hitches=10 + i) for i in range(3)]
        code, out = run(["groups", "A=" + ",".join(map(str, a)), "B=" + ",".join(map(str, b))])
        self.assertEqual(code, 1)
        self.assertIn("no verdict: B has 3 starts", out)
        self.assertNotIn("apart", out)

    def test_groups_ranges_and_counts(self):
        a = [make_run(self.root, f"a{i}", hitches=i) for i in range(4)]
        b = [make_run(self.root, f"b{i}", hitches=10 + i) for i in range(4)]
        counts = self.root / "hair.csv"
        counts.write_text("run,count\n" + "".join(f"{r.name},{n}\n" for r, n in zip(a + b, [1, 0, 2, 1, 1, 2, 0, 3])))
        code, out = run(["groups", "A=" + ",".join(map(str, a)), "B=" + ",".join(map(str, b)),
                         "--count", f"hair={counts}"])
        self.assertEqual(code, 0)
        self.assertIn("hitches: A 0..3 against B 10..13: apart: every start of A lower than every start of B", out)
        self.assertIn("hair: A 0..2 against B 0..3: overlap", out)
        self.assertIn("A: 1 0 2 1  median 1, range 0..2", out)
        self.assertNotIn("p-value", out)


if __name__ == "__main__":
    unittest.main()
