#!/bin/bash
# diff-shards.sh CASES SHARDS [SEED] — the differential run against the TypeScript reference at scale: the release
# build of crates/layout/tests/differential.rs, SHARDS runs in parallel, shard k taking cases k·n … (k+1)·n−1 of ONE
# run of CASES cases from SEED (LAYOUT_DIFF_FIRST; diff-cases.mjs seeds case c with SEED + c·7919 and starts it from
# preset c mod 5), so the shards together are exactly `LAYOUT_DIFF_CASES=CASES LAYOUT_DIFF_SEED=SEED`.
#
# From the desktop repo (node on PATH):
#   CARGO_TARGET_DIR=target/layout-diff LAYOUT_TS_REF=<reference tree> \
#     tools/layout/rust-bridge/diff-shards.sh 30000 3
# Env: LAYOUT_TS_REF (default: the test's own default), J (cargo jobs, default 6), OUT (shard logs, default
# $CARGO_TARGET_DIR/diff-shards). Exit 0 only when every shard passed: 0 records different, 0 check violations.
set -u
CASES=${1:?usage: diff-shards.sh CASES SHARDS [SEED]}
SHARDS=${2:?usage: diff-shards.sh CASES SHARDS [SEED]}
SEED=${3:-20260927}
REPO="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$REPO" || exit 2
OUT=${OUT:-${CARGO_TARGET_DIR:-$REPO/target}/diff-shards}
mkdir -p "$OUT"
rm -f "$OUT"/shard-*.log
export LAYOUT_REQUIRE_NODE=1

cargo test --release -p tinhat-layout -j "${J:-6}" --test differential --no-run >"$OUT/build.log" 2>&1 || {
  tail -30 "$OUT/build.log"
  exit 2
}
per=$(( (CASES + SHARDS - 1) / SHARDS ))
t0=$(date +%s)
pids=()
for ((k = 0; k < SHARDS; k++)); do
  first=$(( k * per ))
  n=$(( CASES - first < per ? CASES - first : per ))
  [ "$n" -gt 0 ] || break
  LAYOUT_DIFF_CASES=$n LAYOUT_DIFF_SEED=$SEED LAYOUT_DIFF_FIRST=$first \
    cargo test --release -p tinhat-layout -j 1 --test differential -- --nocapture >"$OUT/shard-$k.log" 2>&1 &
  pids+=($!)
done
rc=0
for p in "${pids[@]}"; do wait "$p" || rc=1; done

records=0 different=0 violations=0 cases=0
for f in "$OUT"/shard-*.log; do
  line=$(grep -m1 '^differential vs the TypeScript reference' "$f") || { echo "$(basename "$f"): no summary"; rc=1; continue; }
  echo "$(basename "$f" .log): ${line#differential vs the TypeScript reference }"
  c=$(sed -E 's/^[^)]*\): ([0-9]+) cases \(#.*/\1/' <<<"$line")
  r=$(sed -E 's/.* → ([0-9]+) records, ([0-9]+) different.*/\1/' <<<"$line")
  d=$(sed -E 's/.* → ([0-9]+) records, ([0-9]+) different.*/\2/' <<<"$line")
  v=$(sed -E 's/.* — ([0-9]+) violations;.*/\1/' <<<"$line")
  cases=$((cases + c)) records=$((records + r)) different=$((different + d)) violations=$((violations + v))
  grep -E '^(DIFF|VIOLATION)' "$f" | head -5
done
echo "TOTAL: $cases cases from seed $SEED in ${#pids[@]} shards → $records records, $different different, $violations violations; $(( $(date +%s) - t0 )) s (logs $OUT)"
[ "$rc" -eq 0 ] && [ "$different" -eq 0 ] && [ "$violations" -eq 0 ] && [ "$cases" -eq "$CASES" ]
