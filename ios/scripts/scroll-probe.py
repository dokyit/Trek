"""Reads a scroll probe (ScrollProbe.swift) and reports jumps and blank screens.

A frame's line: time, offset, content height, top row id, its index, its top's screen y, rows
visible, touching, rows in all, and (UIKit list) motion: how far the previous frame's top row
moved on screen.

With motion (one line a frame): a jump is a move of more than 300 pt in one frame, or a change
of speed of more than 150 pt/frame from one frame to the next (a swipe or a fling speeds up and
slows down smoothly; a jump doesn't).
Without it (the old SwiftUI list, logged per scroll change): a jump is the top row moving by
more rows than the offset moved (|d index| > |d offset| / 20 + 3), the offset moving more than
400 pt in one change, or the top row's index rising by more than 3 while the offset falls.
Blank: no rows on screen while the transcript has rows.
"""
import sys

frames, notes = [], []
for line in open(sys.argv[1]):
    p = line.rstrip("\n").split("\t")
    if len(p) > 1 and p[1] == "#":
        notes.append((float(p[0]), p[2]))
        continue
    if len(p) < 9:
        continue
    motion = None
    if len(p) >= 10 and p[9] != "-":
        motion = float(p[9])
    frames.append(dict(t=float(p[0]), y=float(p[1]), h=float(p[2]), id=p[3], i=int(p[4]), ty=float(p[5]),
                       vis=int(p[6]), touch=int(p[7]), rows=int(p[8]), m=motion))

has_motion = any(f["m"] is not None for f in frames)
jumps, blanks = [], []
prev_m = 0.0
max_m = 0.0
prev_gap = 1
for a, b in zip(frames, frames[1:]):
    if has_motion:
        if b["m"] is None:
            prev_m = 0.0
            continue
        # Per frame: a hitch (frames dropped) shows a frame's worth of motion several times over,
        # in the frame after it or (a scroll view catching up its deceleration) the one after that.
        frames_between = max(1, round((b["t"] - a["t"]) / 0.01667), prev_gap)
        prev_gap = max(1, round((b["t"] - a["t"]) / 0.01667))
        m = b["m"] / frames_between
        max_m = max(max_m, abs(m))
        why = None
        if abs(m) > 300:
            why = f"content moved {m:+.0f} pt in one frame"
        elif abs(m - prev_m) > 150:
            why = f"speed changed {prev_m:+.0f} -> {m:+.0f} pt/frame"
        if why:
            jumps.append((b["t"], a["i"], b["i"], b["y"] - a["y"], b["touch"], why))
        prev_m = m
        continue
    dy = b["y"] - a["y"]
    if a["i"] < 0 or b["i"] < 0:
        continue
    shift = b["rows"] - a["rows"]
    di = b["i"] - a["i"] - shift
    why = None
    if abs(di) > abs(dy) / 20 + 3:
        why = f"top row moved {di:+d} rows for {dy:+.0f} pt"
    elif abs(dy) > 400 and shift == 0:
        why = f"offset moved {dy:+.0f} pt in one frame"
    elif dy < -2 and di > 3:
        why = f"scrolling up but thrown {di:+d} rows down"
    if why:
        jumps.append((b["t"], a["i"], b["i"], dy, b["touch"], why))
for f in frames:
    if f["vis"] == 0 and f["rows"] > 3:
        blanks.append(f["t"])

t0 = frames[0]["t"] if frames else 0
print(f"{len(frames)} frames over {frames[-1]['t'] - t0 if frames else 0:.1f} s; "
      f"top row {frames[0]['i'] if frames else '-'} -> {frames[-1]['i'] if frames else '-'} "
      f"of {frames[-1]['rows'] if frames else '-'} rows; "
      f"offset {min(f['y'] for f in frames):.0f}..{max(f['y'] for f in frames):.0f}"
      + (f"; fastest frame {max_m:.0f} pt" if has_motion else ""))
gaps = [b["t"] - a["t"] for a, b in zip(frames, frames[1:]) if b["t"] - a["t"] > 0.05]
print(f"hitches (a frame over 50 ms): {len(gaps)}" + (f", longest {max(gaps) * 1000:.0f} ms" if gaps else ""))
print(f"jumps: {len(jumps)}")
for t, ia, ib, dy, touch, why in jumps[:40]:
    print(f"  {t - t0:7.2f}s  row {ia} -> {ib}  ({why}){' finger down' if touch else ''}")
print(f"blank frames: {len(blanks)}" + (f" (first at {blanks[0] - t0:.2f}s)" if blanks else ""))
kept = [n for _, n in notes if n.startswith("measured") and "above the screen" in n]
if kept:
    print(f"rows measured above the screen, the offset moved to match: {len(kept)}")
for t, n in [(t, n) for t, n in notes if not n.startswith("measured")][:40]:
    print(f"  note {t - t0:7.2f}s  {n}")
