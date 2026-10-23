#!/usr/bin/env bash
# Tier 0: instant, deterministic checks after every edit (no LLM, no network). `make quick`.
#
#   scripts/quick.sh                      run every part
#   scripts/quick.sh --update             accept the current output (snapshots, golden, noise)
#   scripts/quick.sh --only snapshots,golden
#
# Parts (each prints one result line; timings go to stderr, so stdout is the same on every run):
#   snapshots  feature snapshot tests, agent/tests/snapshots.rs (no data needed)
#   golden     mini golden on ~20 small real files, eval/quick/files.txt vs golden.tsv ($HEDGEHOG_DATA)
#   noise      noise smoke: 1 seed per family at t = 0.5, vs eval/quick/noise.tsv
#   info       quick mode of the info scoreboard, when eval/info/score.py exists
#   public     14 HiTab tables of the public benchmark (eval/public/quick.py): value-label linking counts
#
# Builds use the `quick` profile (Cargo.toml: dependencies optimized, hedgehog's own crates
# unoptimized and incremental, so a rebuild after an edit takes seconds) in target-quick/
# (QUICK_TARGET_DIR), separate from the release builds. Heavy commands go through ../scripts/run_capped.sh when it exists (memory cap; output to a
# log in target-quick/quick-work/), else they run directly (CI). HEDGEHOG_BIN overrides the hedgehog
# binary of the golden part (default: the quick build, rebuilt incrementally).
set -uo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
UPDATE=0
ONLY=snapshots,golden,noise,info,public
while [ $# -gt 0 ]; do
  case $1 in
    --update) UPDATE=1 ;;
    --only) ONLY=$2; shift ;;
    -h|--help) sed -n '2,/^set /p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument $1" >&2; exit 2 ;;
  esac
  shift
done

export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2}
export CARGO_TARGET_DIR=${QUICK_TARGET_DIR:-$ROOT/target-quick}
WORK=$CARGO_TARGET_DIR/quick-work
mkdir -p "$WORK"

# The data folder: $HEDGEHOG_DATA, else ../testdata next to the repository (or next to its main
# checkout when this is a git worktree).
find_up() { # find_up <relative path>: the first ancestor of ROOT that has it
  local d=$ROOT
  while [ "$d" != / ]; do
    if [ -e "$d/$1" ]; then echo "$d/$1"; return 0; fi
    d=$(dirname "$d")
  done
  return 1
}
if [ ! -d "${HEDGEHOG_DATA:-}" ]; then HEDGEHOG_DATA=$(find_up testdata || true); fi
export HEDGEHOG_DATA
RUN_CAPPED=${RUN_CAPPED:-$(find_up scripts/run_capped.sh || true)}
CAP=${QUICK_CAP:-2500M}

# run <log> <cmd...>: memory-capped when possible; output to the log
run() {
  local log=$1; shift
  : >"$log"
  if [ -n "$RUN_CAPPED" ]; then
    "$RUN_CAPPED" "$CAP" "$log" "$@"
    local rc=$?
    if grep -q '^run_capped: waiting' "$log"; then
      echo "  (waited for run_capped: other capped jobs or high swap; timing includes the wait)" >&2
    fi
    return $rc
  else
    "$@" >>"$log" 2>&1
  fi
}

now() { date +%s.%N; }
secs() { awk -v a="$1" -v b="$(now)" 'BEGIN { printf "%.1f", b - a }'; }
want() { case ",$ONLY," in *",$1,"*) return 0 ;; *) return 1 ;; esac; }

FAILED=()
TIMES=()
finish() { # finish <part> <rc> <t0>
  local t; t=$(secs "$3")
  TIMES+=("$1 $t s")
  echo "  [$1 $t s]" >&2
  [ "$2" -eq 0 ] || FAILED+=("$1")
}

# --- 1. feature snapshots ------------------------------------------------------------------------
if want snapshots; then
  t0=$(now)
  log=$WORK/snapshots.log
  run "$log" env UPDATE_SNAPSHOTS=$UPDATE cargo test --offline --profile quick -p hh_agent --test snapshots -q -- --test-threads=1
  rc=$?
  if [ $rc -eq 0 ]; then
    res=$(grep -E '^test result:' "$log" | tail -1 | sed -E 's/^test result: ok\. ([0-9]+) passed.*/\1 passed/')
    if [ $UPDATE = 1 ]; then
      echo "snapshots: $res; updated: $(grep -c '^updated ' "$log") (review: git diff agent/tests/snapshots/)"
    else
      echo "snapshots OK: $res"
    fi
  else
    echo "snapshots FAILED (log: ${log#"$ROOT"/})"
    if grep -q '^failures:' "$log"; then
      sed -n '/^failures:/,/^test result/p' "$log" | grep -v -E "^(note: run with|failures:\$|test result|thread '.*'.* panicked at )" | sed 's/^/  /'
    else
      tail -40 "$log" | sed 's/^/  /'
    fi
  fi
  finish snapshots $rc "$t0"
fi

# --- 2. mini golden on real files --------------------------------------------------------------
if want golden; then
  t0=$(now)
  if [ -z "$HEDGEHOG_DATA" ] || [ ! -d "$HEDGEHOG_DATA" ]; then
    echo "golden: skipped (no test data; set HEDGEHOG_DATA)"
  else
    log=$WORK/golden.log
    BIN=${HEDGEHOG_BIN:-}
    rc=0
    if [ -z "$BIN" ]; then
      run "$log" cargo build --offline --profile quick -p hh_cli -q
      rc=$?
      BIN=$CARGO_TARGET_DIR/quick/hedgehog
    fi
    if [ $rc -ne 0 ]; then
      echo "golden: build FAILED"; tail -30 "$log" | sed 's/^/  /'
    else
      args=(--bin "$BIN" --data "$HEDGEHOG_DATA" --tmp "$WORK")
      [ $UPDATE = 1 ] && args+=(--update)
      run "$log" python3 -I "$ROOT/eval/quick/golden.py" "${args[@]}"
      rc=$?
      cat "$log"
    fi
  fi
  finish golden "${rc:-0}" "$t0"
fi

# --- 3. noise smoke ------------------------------------------------------------------------------
if want noise; then
  t0=$(now)
  rc=0
  if [ -z "$HEDGEHOG_DATA" ] || [ ! -d "$HEDGEHOG_DATA" ]; then
    echo "noise: skipped (no test data; set HEDGEHOG_DATA)"
  else
    log=$WORK/noise.log
    run "$log" cargo build --offline --profile quick --manifest-path "$ROOT/eval/noise/Cargo.toml" -q
    rc=$?
    if [ $rc -ne 0 ]; then
      echo "noise: build FAILED"; tail -30 "$log" | sed 's/^/  /'
    else
      rm -rf "$WORK/noise"
      run "$log" "$CARGO_TARGET_DIR/quick/noise_eval" --seeds 1 --intensities 0.5 --threads 2 \
        --work "$WORK/noise" --batch-dir "$WORK/noise/batch" --data "$HEDGEHOG_DATA"
      rc=$?
      if [ $rc -ne 0 ]; then
        echo "noise: noise_eval FAILED"; tail -30 "$log" | sed 's/^/  /'
      else
        args=("$WORK/noise/batch")
        [ $UPDATE = 1 ] && args+=(--update)
        python3 -I "$ROOT/eval/quick/noise.py" "${args[@]}"
        rc=$?
      fi
    fi
  fi
  finish noise $rc "$t0"
fi

# --- 4. info scoreboard, quick mode -------------------------------------------------------------
# HOOK(info): eval/info/ (the info scoreboard) is built on a parallel branch. When
# eval/info/score.py exists, run its quick mode on the small cases here, e.g.
#   run "$WORK/info.log" python3 -I eval/info/score.py --quick ...; cat "$WORK/info.log"
# and compare with a stored result like the parts above.
if want info; then
  t0=$(now)
  if [ -z "$HEDGEHOG_DATA" ] || [ ! -d "$HEDGEHOG_DATA" ]; then
    echo "info: skipped (no test data; set HEDGEHOG_DATA)"
  else
    log=$WORK/info.log
    : >"$log"
    BIN=${HEDGEHOG_BIN:-}
    rc=0
    if [ -z "$BIN" ]; then
      run "$log" cargo build --offline --profile quick -p hh_cli -q
      rc=$?
      BIN=$CARGO_TARGET_DIR/quick/hedgehog
    fi
    if [ $rc -ne 0 ]; then
      echo "info: build FAILED"; tail -30 "$log" | sed 's/^/  /'
    else
      : >"$log"
      args=(--bin "$BIN" --data "$HEDGEHOG_DATA" --tmp "$WORK")
      [ $UPDATE = 1 ] && args+=(--update)
      run "$log" python3 -I "$ROOT/eval/quick/info.py" "${args[@]}"
      rc=$?
      grep -E "^info|^  " "$log"
    fi
  fi
  finish info "${rc:-0}" "$t0"
fi

# --- 5. public benchmark slice (HiTab, metric B) ------------------------------------------------
if want public; then
  t0=$(now)
  rc=0
  if [ -z "$HEDGEHOG_DATA" ] || [ ! -d "$HEDGEHOG_DATA/public/hitab" ]; then
    echo "public: skipped (no HiTab data; eval/public/README.md)"
  else
    log=$WORK/public.log
    BIN=${HEDGEHOG_BIN:-}
    if [ -z "$BIN" ]; then
      run "$log" cargo build --offline --profile quick -p hh_cli -q
      rc=$?
      BIN=$CARGO_TARGET_DIR/quick/hedgehog
    fi
    if [ $rc -ne 0 ]; then
      echo "public: build FAILED"; tail -30 "$log" | sed 's/^/  /'
    else
      args=(--bin "$BIN" --data "$HEDGEHOG_DATA" --tmp "$WORK")
      [ $UPDATE = 1 ] && args+=(--update)
      run "$log" python3 -I "$ROOT/eval/public/quick.py" "${args[@]}"
      rc=$?
      grep -E "^public|^  " "$log"
    fi
  fi
  finish public "${rc:-0}" "$t0"
fi

# --- summary ---------------------------------------------------------------------------------------
echo "timings: ${TIMES[*]}" | sed 's/ s \([a-z]\)/ s · \1/g' >&2
if [ ${#FAILED[@]} -gt 0 ]; then
  echo "quick: FAILED (${FAILED[*]})"
  exit 1
fi
echo "quick: OK"
