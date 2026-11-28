#!/bin/bash
# Second pass: apps installed later, the Codex app again on a quieter Mac, the `open -g` baselines,
# then the accessibility re-probe, then Synara's post (#12) with real agents in Trek.
set -u
S="$1"; R="$2"; HERE="$(cd "$(dirname "$0")" && pwd)"
APPPROBE="$S/appprobe" AXPROBE="$S/axprobe2" AX_WAIT=8 RUNS=3 SETTLE=25 CPU_SECS=30 \
  python3 -I "$HERE/measure_apps.py" "$R/apps.json" "$S/app-shots" Paseo super.engineering "Codex app" "Calculator (baseline)" "TextEdit (baseline)"
echo "pass2 apps done $(date) load $(sysctl -n vm.loadavg)"
APPPROBE="$S/appprobe" AXPROBE="$S/axprobe2" AX_WAIT=8 python3 -I "$HERE/ax_pass.py" "$R/ax-pass.json" \
  Cursor Synara "T3 Code" "OpenCode desktop" Zed Antigravity "VS Code" Orca Capy Conductor Claude Warp
echo "ax pass done $(date) load $(sysctl -n vm.loadavg)"
while [ "$(sysctl -n vm.loadavg | awk '{print int($2)}')" -ge 5 ]; do sleep 30; done
python3 -I "$HERE/trek_real10.py" "$S/trek-release" "$R/trek-real10.json"
echo "real10 done $(date) load $(sysctl -n vm.loadavg)"
