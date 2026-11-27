"""victory-blink.py on made-up captures.  python3 -I -m unittest discover scripts/tests"""

import importlib.util
import io
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

import numpy as np
from PIL import Image

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("blink", HERE.parent / "victory-blink.py")
blink = importlib.util.module_from_spec(spec)
spec.loader.exec_module(blink)


def scene(seed=1, w=480, h=270):
    """A textured picture: smooth gradients and some blocks, no flat areas."""
    rng = np.random.default_rng(seed)
    y, x = np.mgrid[0:h, 0:w]
    img = np.stack([(x * 255 / w), (y * 255 / h), ((x + y) * 128 / (w + h)) + 60], -1)
    for _ in range(12):
        x0, y0 = rng.integers(0, w - 60), rng.integers(0, h - 40)
        img[y0:y0 + 40, x0:x0 + 60] = rng.integers(0, 255, 3)
    return img.clip(0, 255).astype(np.uint8)


def write(folder, frames, start_ms=300000, gap_ms=140):
    folder.mkdir(parents=True, exist_ok=True)
    for i, f in enumerate(frames):
        Image.fromarray(f).save(folder / f"{start_ms + i * gap_ms:07d}.png")


def run(args):
    out = io.StringIO()
    with redirect_stdout(out):
        code = blink.main([str(a) for a in args])
    return code, out.getvalue()


def number(text):
    line = next(l for l in text.splitlines() if l.startswith("blink number:"))
    return float(line.split("median ")[1].split(",")[0])


class BlinkTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def test_still_picture_is_one(self):
        base = scene()
        write(self.dir / "still", [base] * 12)
        code, out = run([self.dir / "still"])
        self.assertEqual(code, 0)
        self.assertAlmostEqual(number(out), 1.0, places=2)

    def test_pan_is_not_blinking(self):
        big = scene(w=640)
        frames = [np.ascontiguousarray(big[:, 6 * i:6 * i + 480]) for i in range(12)]
        write(self.dir / "pan", frames)
        _, out = run([self.dir / "pan"])
        self.assertLess(number(out), 1.5, out)

    def test_cut_is_not_blinking(self):
        a, b = scene(1), scene(2)
        write(self.dir / "cut", [a] * 6 + [b] * 6)
        _, out = run([self.dir / "cut", "--window", "12"])
        self.assertLess(number(out), 1.5, out)

    def test_single_frame_shard_blinks(self):
        base = scene()
        frames = [base.copy() for _ in range(12)]
        shard = frames[5].copy()
        shard[60:180, 100:300] = (200, 30, 40)  # a large flat area, one capture only
        frames[5] = shard
        write(self.dir / "shard", frames)
        _, out = run([self.dir / "shard", "--out", self.dir / "crops"])
        self.assertGreater(number(out), 10, out)
        self.assertTrue(any((self.dir / "crops").glob("*-blink*.png")))

    def test_shard_over_two_captures_blinks(self):
        base = scene()
        frames = [base.copy() for _ in range(12)]
        for i in (5, 6):
            frames[i][60:180, 100:300] = (10, 200, 40)
        write(self.dir / "two", frames)
        _, out = run([self.dir / "two"])
        self.assertGreater(number(out), 10, out)

    def test_flat_captures_counted_and_left_out(self):
        base = scene()
        frames = [base] * 10
        frames.insert(4, np.zeros_like(base))
        frames.insert(8, np.full_like(base, 128))
        write(self.dir / "black", frames)
        _, out = run([self.dir / "black"])
        self.assertIn("2 all one colour", out)
        self.assertAlmostEqual(number(out), 1.0, places=2)

    def test_from_to_and_times(self):
        write(self.dir / "t", [scene()] * 20, start_ms=313000, gap_ms=200)
        _, out = run([self.dir / "t", "--from", "314", "--to", "315.5", "--window", "4"])
        self.assertIn("8 captures from 314.0 to 315.4 s", out)

    def test_is_flat(self):
        self.assertTrue(blink.is_flat(np.zeros((10, 10, 3), np.float32)))
        self.assertFalse(blink.is_flat(scene().astype(np.float32)))


if __name__ == "__main__":
    unittest.main()
