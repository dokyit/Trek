#!/bin/bash
# The Trek half of the sequence: Trek's own perf, then the stress runs. Waits for a pid first.
set -u
S="$1"; R="$2"; WAIT_PID="${3:-}"; HERE="$(cd "$(dirname "$0")" && pwd)"; ROOT="$HERE/../.."
[ -n "$WAIT_PID" ] && while kill -0 "$WAIT_PID" 2>/dev/null; do sleep 20; done
echo "apps done $(date) load $(sysctl -n vm.loadavg)"
mkdir -p "$S/perf-tools"; cp "$S/perf_seed" "$S/perf_checkpoint" "$S/perf-tools/"
(cd "$ROOT" && ./bin/agent-cli perf launch idle stream long --bin "$S/trek-release" --tools "$S/perf-tools" --out "$R/trek-perf.json" --json) > "$S/logs/trek-perf.log" 2>&1
echo "trek perf done rc=$? $(date) load $(sysctl -n vm.loadavg)"
THREADS=100,1000,5000,10000 SWARM=10,20,50 TABS=50,100,200 RUNS=3 SETTLE=25 \
  python3 -I "$HERE/trek_stress.py" "$S/trek-release" "$S/perf_seed" "$R/trek-stress.json"
echo "stress done $(date) load $(sysctl -n vm.loadavg)"
