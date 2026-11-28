#!/bin/bash
# Terminal-Bench (CPU-only set, 7 light tasks), same model for both harnesses: Trek's Direct loop
# (trek-bench via the earlier study's Harbor adapter) and OpenCode (Harbor's built-in agent).
# One trial per task, 25-minute agent limit (multiplier 0.125), 2 trials at a time.
# A guard polls OpenRouter's key and stops both jobs past CAP dollars spent since the start.
set -u
S="$1"; OUT="$2"; CAP="${CAP:-4.0}"
KEY="$(cat ~/.trek-bench/openrouter.key)"
usage() { curl -s https://openrouter.ai/api/v1/key -H "Authorization: Bearer $KEY" | python3 -c "import json,sys; print(json.load(sys.stdin)['data']['usage'])"; }
START=$(usage); echo "usage at start: $START"
TASKS="-i cargo-flight-dispatch -i production-planning -i foodstuff-beta-activity -i risk-scorer-replay -i photonic-waveguide-routing -i bun-sourcemap-leak -i roy-polymorph-cn"
COMMON="-p $S/tb/terminal-bench-cpu-only -k 1 -n 2 --agent-timeout-multiplier 0.125 -o $OUT -y"
guard() { while sleep 60; do u=$(usage) || continue; spent=$(python3 -c "print(round($u-$START,3))"); echo "spent $spent"; if python3 -c "import sys; sys.exit(0 if $u-$START>=$CAP else 1)"; then echo "CAP REACHED: stopping"; pkill -TERM -P $$ harbor; for c in $(docker ps -q); do docker stop -t 5 $c; done; exit 0; fi; done; }
guard & G=$!
cd ~/Developer/trek-bench-study
PYTHONPATH=bench harbor run $COMMON --job-name tb-trek-direct -a harbor_trek.agent:TrekAgent -m deepseek/deepseek-v4-flash \
  --ak script=false --ak model=deepseek/deepseek-v4-flash --ak effort=high --ak max_turns=200 --ak time_limit_s=1500 --ak trek_bench_path=$S/trek-bench $TASKS
echo "trek-direct done rc=$? spent $(python3 -c "print(round($(usage)-$START,3))")"
OPENROUTER_API_KEY="$KEY" harbor run $COMMON --job-name tb-opencode -a opencode -m openrouter/deepseek/deepseek-v4-flash --api-key-name OPENROUTER_API_KEY $TASKS
echo "opencode done rc=$? spent $(python3 -c "print(round($(usage)-$START,3))")"
kill $G
