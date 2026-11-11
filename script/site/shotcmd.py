"""Send raw shots commands to the isolated test app (one per argument), and wait for them.

shotcmd.py SHOT_DIR "resize 1180 760" "wait 300"
"""
import os
import sys
import time
from pathlib import Path

shot_dir = Path(sys.argv[1])
lines = sys.argv[2:]
done = shot_dir / "done"
if done.exists():
    done.unlink()
tmp = shot_dir / "cmd.tmp"
tmp.write_text("\n".join(lines) + "\n")
os.replace(tmp, shot_dir / "cmd")
t0 = time.time()
while not done.exists():
    if time.time() - t0 > 60:
        sys.exit(f"timed out on {lines}")
    time.sleep(0.02)
out = done.read_text().strip()
if out != "ok":
    sys.exit(f"{lines}: {out}")
