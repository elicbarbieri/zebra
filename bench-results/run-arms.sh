#!/usr/bin/env bash
# Usage: FILTER='fixture_blocks/' run-arms.sh <arm:threads>...   e.g. run-arms.sh a:4 b:4 c:4
# - P-cores only (cpu 2 = one P-core; 2,4,6,8 = four physical P-cores)
# - arm c builds with SINSEMILLA_TABLE_LIMIT covering the 52-word MerkleCRH table
set -euo pipefail
ROOT=/home/elicb/DevProjects/ZingoLabs
OUT=$ROOT/bench-results
FILTER=${FILTER:-}
mkdir -p "$OUT"
for spec in "$@"; do
  arm=${spec%%:*}
  threads=${spec##*:}
  case $threads in
    1) cpus=2 ;;
    4) cpus=2,4,6,8 ;;
    *) echo "unsupported thread count $threads" >&2; exit 1 ;;
  esac
  name="arm-${arm}-t${threads}"
  echo "=== $name  $(date -Is)"
  (cd "$ROOT/zebra-bench-$arm" \
    && if [ "$arm" = c ]; then export SINSEMILLA_TABLE_LIMIT=3473408; fi \
    && ORCHARD_REPLAY=$OUT/replay RAYON_NUM_THREADS=$threads taskset -c "$cpus" \
       cargo bench -p zebra-chain --bench orchard_tree -- --save-baseline "$name" --noplot $FILTER \
    && critcmp --export "$name" > "$OUT/$name.json")
done
echo "=== done $(date -Is)"
