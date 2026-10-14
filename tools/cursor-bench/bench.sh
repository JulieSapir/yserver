#!/bin/bash
# Usage: CARD=/dev/dri/cardN [CAM=/dev/video0] ./bench.sh <outdir> <name=yserver-binary>...
# Per binary: motion (incl. RANDR + gamma while moving), drag, windowed and fullscreen animation with motion,
# cursor hidden before a fullscreen client. Compare master against the branch.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$1; shift; mkdir -p "$ROOT"
for nb in "$@"; do
  n=${nb%%=*}; b=${nb#*=}
  for t in motion drag anim-composed anim-direct hidden; do
    echo "=== $n $t"
    case $t in anim-*) script=anim; mode=${t#anim-} ;; *) script=$t; mode= ;; esac
    MODE=$mode OUT=$ROOT/$n-$t bash "$HERE/tests/$script.sh" "$b" > "$ROOT/$n-$t.out" 2>&1
    grep -E "rtt|lag|^mpv |direct submits|exit=|STILL|timed out|panicked|EBUSY" "$ROOT/$n-$t.out" | sed 's/^/    /'
  done
done
python3 "$HERE/latency.py" "$ROOT"/*/yserver.log
