#!/bin/bash
# The measurement sequence, one thing at a time on a quiet Mac.
# usage: run_all.sh <scratch> <results_dir>   (scratch holds trek-release, perf_seed, perf_checkpoint, appprobe, axprobe)
set -u
S="$1"; R="$2"; HERE="$(cd "$(dirname "$0")" && pwd)"; ROOT="$HERE/../.."
echo "start $(date) load $(sysctl -n vm.loadavg)"
APPPROBE="$S/appprobe" AXPROBE="$S/axprobe" RUNS=3 SETTLE=25 CPU_SECS=30 \
  python3 -I "$HERE/measure_apps.py" "$R/apps.json" "$S/app-shots"
echo "apps done $(date) load $(sysctl -n vm.loadavg)"
mkdir -p "$S/perf-tools"; cp "$S/perf_seed" "$S/perf_checkpoint" "$S/perf-tools/"
(cd "$ROOT" && ./bin/agent-cli perf launch idle stream long --bin "$S/trek-release" --tools "$S/perf-tools" --out "$R/trek-perf.json" --json) > "$S/logs/trek-perf.log" 2>&1
echo "trek perf done rc=$? $(date) load $(sysctl -n vm.loadavg)"
THREADS=100,1000,5000,10000 SWARM=10,20,50 TABS=50,100,200 RUNS=3 SETTLE=25 \
  python3 -I "$HERE/trek_stress.py" "$S/trek-release" "$S/perf_seed" "$R/trek-stress.json"
echo "stress done $(date) load $(sysctl -n vm.loadavg)"
