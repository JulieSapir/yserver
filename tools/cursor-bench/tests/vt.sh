#!/bin/bash
# MANUAL: run from a text console (tty) on the machine. VT switch while the pointer moves.
# Switch away with Ctrl+Alt+F2 and back with your VT's key, twice, once while the pointer is moving.
. "$(dirname "$0")/../lib.sh"; BIN=$1; mkdir -p "$OUT"
case "$(tty)" in /dev/tty[0-9]*) ;; *) echo "run me from a text console (tty), not a terminal emulator"; exit 1;; esac
start_server "$BIN"
mpv --no-config --vo=gpu-next --gpu-api=vulkan --geometry=1280x720+100+100 --loop=inf \
  "av://lavfi:testsrc=size=1920x1080:rate=60" > "$OUT/mpv.log" 2>&1 & M=$!
"$HERE/../xlat" 90 250 > "$OUT/xlat.txt" 2>&1 & XP=$!
echo "Now: Ctrl+Alt+F2, wait 5 s, come back; repeat once. The run ends by itself after ~90 s."
wait $XP; kill $M; wait $M 2>/dev/null; stop_server
grep -iE "kms: VT|VT_ACTIVATE|vt release|vt acquire" "$OUT/yserver.log" | head -20
log_checks
echo "Check: after each return the picture kept updating and the cursor was visible and moving."
