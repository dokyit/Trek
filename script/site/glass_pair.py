"""Take the isolated test app's frame twice, over black and over white, so its real alpha can be
recovered (see unglass.py). The app was started with TREK_SHOT_UNDER=<under.png>; its blurred
underlay is cached per size, so a resize in between makes it read the swapped file.

glass_pair.py SHOT_DIR UNDER_PNG OUT_PREFIX WIDTH HEIGHT
"""
import os
import shutil
import sys
import time
from pathlib import Path

from PIL import Image

shot_dir, under, prefix = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3]
W, H = int(sys.argv[4]), int(sys.argv[5])


def run(lines, cap=30.0):
    done = shot_dir / "done"
    if done.exists():
        done.unlink()
    tmp = shot_dir / "cmd.tmp"
    tmp.write_text("\n".join(lines) + "\n")
    os.replace(tmp, shot_dir / "cmd")
    t0 = time.time()
    while not done.exists():
        if time.time() - t0 > cap:
            raise SystemExit(f"timed out on {lines}")
        time.sleep(0.02)
    out = done.read_text().strip()
    if out != "ok":
        raise SystemExit(f"{lines}: {out}")


def solid(colour):
    tmp = under.with_suffix(".tmp.png")
    Image.new("RGB", (64, 64), colour).save(tmp)
    os.replace(tmp, under)


for name in ("gb", "gw"):
    p = shot_dir / f"{name}.png"
    if p.exists():
        p.unlink()

# The underlay is cached for the last size a frame was saved at, so a frame saved at another size
# in between is what makes the app read the swapped file.
solid((0, 0, 0))
run([f"resize {W + 2} {H}", "wait 150", "shot gx", f"resize {W} {H}", "wait 250", "shot gx"])
run(["shot gb"])
solid((255, 255, 255))
run([f"resize {W + 2} {H}", "wait 120", "shot gx", f"resize {W} {H}", "wait 120", "shot gw"])
shutil.copy(shot_dir / "gb.png", prefix + "-black.png")
shutil.copy(shot_dir / "gw.png", prefix + "-white.png")
print("pair", prefix)
