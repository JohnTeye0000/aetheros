#!/bin/bash
# Run every experiment plan against one kernel image.
#
#   experiments/run_all.sh <kernel-image.bin> <output-dir> [parallelism]
#
# e.g. the optimized kernel:
#   cargo bootimage --release
#   experiments/run_all.sh target/x86_64-unknown-none/release/bootimage-aetheros_kernel.bin \
#                          experiments/results/release 6
#
# Each plan runs in its own QEMU (they are independent and deterministic under
# -icount, so running several at once changes wall-clock time only, never the
# results). Launches are staggered because starting many QEMUs in the same
# second occasionally fails on Windows.

BIN="$1"
OUT="$2"
PAR="${3:-6}"
if [ -z "$BIN" ] || [ -z "$OUT" ]; then
  echo "usage: $0 <kernel-image.bin> <output-dir> [parallelism]"
  exit 2
fi
HERE="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$OUT"

PLANS="main_cpu4 main_mixed main_interactive main_all scale quantum plainlang \
fair_main fair_sweeps fair_plainlang hfair_main hfair_sweeps hfair_mult hfair_plainlang plainlang_v2 aging_boost"

running=0
for p in $PLANS; do
  [ -f "$HERE/plans/$p.txt" ] || { echo "missing plan $p"; continue; }
  rm -rf "$OUT/$p"
  python "$HERE/aetherbench.py" --script "$HERE/plans/$p.txt" --out "$OUT/$p" --bin "$BIN" \
      --timeout 7200 --stall 600 --files "$HERE"/plainlang/*.axe > "$OUT/$p.out" 2>&1 &
  sleep 4
  running=$((running + 1))
  if [ "$running" -ge "$PAR" ]; then
    wait -n
    running=$((running - 1))
  fi
done
wait
echo "== summary"
for p in $PLANS; do
  printf "%-18s %s\n" "$p" "$(tail -1 "$OUT/$p.out" 2>/dev/null)"
done
