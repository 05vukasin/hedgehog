#!/usr/bin/env bash
# Runs one hh_fuzz target for a bounded time, restarting the fuzzer after a hang (exit 70) or an
# abort (heap limit, stack overflow, a non-unwinding panic), and keeping the input that caused it.
#
#   fuzz/run.sh <target> <seconds> <corpus_dir> <out_dir> [hh_fuzz binary]
#
# Run it under a memory cap (scripts/run_capped.sh); the fuzzer's own heap limit is --rss-mb
# (default 2048 MB). Findings: <out>/crashes (panics, aborts), <out>/hangs; log: <out>/log.
set -uo pipefail
target=$1 secs=$2 corpus=$3 out=$4
bin=${5:-${CARGO_TARGET_DIR:-target}/fuzz/hh_fuzz}
mkdir -p "$out/crashes"
end=$((SECONDS + secs))
while ((SECONDS < end)); do
  left=$((end - SECONDS))
  "$bin" fuzz "$target" --corpus "$corpus" --out "$out" --seconds "$left" --timeout "${HH_FUZZ_TIMEOUT:-10}" \
    --rss-mb "${HH_FUZZ_RSS_MB:-2048}" 2>>"$out/log"
  rc=$?
  if ((rc == 0)); then
    break
  fi
  if ((rc != 70)); then
    cur=$(ls "$out"/cur.* 2>/dev/null | head -1)
    if [[ -n $cur ]]; then
      sum=$(sha256sum "$cur" | cut -c1-16)
      cp "$cur" "$out/crashes/exit$rc-$sum.${cur##*.}"
      tail -5 "$out/log" >"$out/crashes/exit$rc-$sum.${cur##*.}.txt"
      echo "== exit $rc: saved $out/crashes/exit$rc-$sum.${cur##*.}" >>"$out/log"
    fi
  fi
done
