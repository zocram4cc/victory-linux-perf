"""victory-leg.sh's pure parts, sourced into bash.  python3 -I -m unittest discover -s scripts/tests

Never starts the launcher: only argument parsing, the data folder guard, the
wait for a run's start and the black test.
"""

import json
import os
import subprocess
import tempfile
import threading
import time
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "victory-leg.sh"


def bash(code, home=None, timeout=30):
    env = dict(os.environ, HOME=str(home)) if home else None
    return subprocess.run(["bash", "-c", f'source "{SCRIPT}"; {code}'], capture_output=True, text=True,
                          timeout=timeout, env=env)


class LegTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def test_syntax(self):
        self.assertEqual(subprocess.run(["bash", "-n", str(SCRIPT)]).returncode, 0)

    def test_parse_shots(self):
        r = bash("parse_shots 300-330,400-410")
        self.assertEqual(r.returncode, 0)
        self.assertEqual(r.stdout.split("\n")[:2], ["300 330", "400 410"])
        self.assertEqual(bash("parse_shots 090-100").stdout.strip(), "90 100")
        for bad in ["330-300", "300", "a-b", "300-330,", "", "300-300"]:
            self.assertNotEqual(bash(f"parse_shots '{bad}'").returncode, 0, bad)

    def test_data_dir_refuses_the_players(self):
        share = self.dir / ".local/share/victory"
        (share / "data").mkdir(parents=True)
        self.assertNotEqual(bash(f"data_dir '{share}/data'", home=self.dir).returncode, 0)
        self.assertNotEqual(bash(f"data_dir '{share}/data/../data/'", home=self.dir).returncode, 0)
        (self.dir / "link").symlink_to(share / "data")
        self.assertNotEqual(bash(f"data_dir '{self.dir}/link'", home=self.dir).returncode, 0)
        r = bash("data_dir ''", home=self.dir)
        self.assertEqual(r.stdout.strip(), str(share / "data-auto"))

    def test_main_refuses_players_data_before_starting(self):
        share = self.dir / ".local/share/victory"
        (share / "data").mkdir(parents=True)
        r = bash(f"VICTORY_DATA_DIR='{share}/data' main lbl intro", home=self.dir)
        self.assertEqual(r.returncode, 2)
        self.assertIn("player's data folder", r.stderr)
        r = bash("main 'bad label' intro", home=self.dir)
        self.assertIn("label:", r.stderr)
        r = bash("main lbl intro NOTASWITCH", home=self.dir)
        self.assertIn("not a NAME=value switch", r.stderr)

    def test_wait_for_start_waits_for_started_epoch(self):
        run = self.dir / "run"
        run.mkdir()
        (run / "run.json").write_text("{")  # being written

        def later():
            time.sleep(1.5)
            (run / "run.json").write_text(json.dumps({"bench": "intro"}))
            time.sleep(1.0)
            (run / "run.json").write_text(json.dumps({"bench": "intro", "started_epoch": 1795795487.79}))
        threading.Thread(target=later).start()
        t = time.time()
        r = bash(f"wait_for_start '{run}' 10")
        self.assertEqual(r.returncode, 0)
        self.assertEqual(r.stdout.strip(), "1795795487.79")
        self.assertGreater(time.time() - t, 2.0)

    def test_wait_for_start_times_out(self):
        r = bash(f"wait_for_start '{self.dir}/missing' 2")
        self.assertNotEqual(r.returncode, 0)

    def test_wait_for_run_folder(self):
        out = self.dir / "leg.out"
        out.write_text("note: something\nrun folder: /x/runs/20261127-160447-bench-intro\nmore\n")
        self.assertEqual(bash(f"wait_for_run_folder '{out}' 3").stdout.strip(), "/x/runs/20261127-160447-bench-intro")
        out.write_text("nothing\n")
        self.assertNotEqual(bash(f"wait_for_run_folder '{out}' 2").returncode, 0)

    def test_black_test(self):
        try:
            from PIL import Image
        except ImportError:
            self.skipTest("no PIL")
        shots = self.dir / "shots"
        shots.mkdir()
        Image.new("RGB", (64, 36), (0, 0, 0)).save(shots / "0300000.png")
        Image.new("RGB", (64, 36), (90, 20, 200)).save(shots / "0300140.png")
        im = Image.new("RGB", (64, 36), (0, 0, 0))
        for x in range(64):
            for y in range(36):
                im.putpixel((x, y), (x * 4, y * 7, 100))
        im.save(shots / "0300280.png")
        r = bash(f"flat_shots '{shots}'")
        lines = r.stdout.split("\n")
        self.assertEqual(lines[0], "2 3")
        self.assertEqual(lines[1:3], ["0300000.png", "0300140.png"])


if __name__ == "__main__":
    unittest.main()
