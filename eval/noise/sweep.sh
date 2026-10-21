#!/usr/bin/env bash
# Full robustness sweep in small capped jobs (one family × intensity × ≤ 200 seeds per job, one
# job at a time, 2 worker threads), then the report from all batches.
# Usage (from the repo root): eval/noise/sweep.sh [seeds per cell, default 60] [work dir]
set -euo pipefail
SEEDS=${1:-60}
WORK=${2:-/tmp/claude-1000/w1noise}
CAP=${CAP:-2500M}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
RUN=${RUN_CAPPED:-$ROOT/../scripts/run_capped.sh}
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2}
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/target-noise}
BIN=$CARGO_TARGET_DIR/release/noise_eval
LOG=$WORK/sweep.log
mkdir -p "$WORK"
"$RUN" "$CAP" "$LOG" cargo build --offline --release --manifest-path "$ROOT/eval/noise/Cargo.toml"
BATCH=$WORK/batches
rm -rf "$BATCH"
FAMILIES=$("$BIN" --list)
for f in $FAMILIES; do
  for t in 0 0.25 0.5 0.75 1; do
    from=0
    while [ "$from" -lt "$SEEDS" ]; do
      n=$(( SEEDS - from < 200 ? SEEDS - from : 200 ))
      "$RUN" "$CAP" "$LOG" "$BIN" --families "$f" --intensities "$t" --seed-from "$from" --seeds "$n" \
        --threads 2 --work "$WORK" --batch-dir "$BATCH"
      from=$(( from + n ))
    done
  done
done
"$RUN" "$CAP" "$LOG" "$BIN" --report-from "$BATCH" --out "$ROOT/eval/noise/REPORT.md"
echo "report: $ROOT/eval/noise/REPORT.md"
