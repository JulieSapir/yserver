#!/bin/bash
# XFIXES HideCursor BEFORE a fullscreen client starts (how games hide it); count direct submits while hidden.
. "$(dirname "$0")/../lib.sh"; BIN=$1; mkdir -p "$OUT"
start_server "$BIN"; L=$OUT/yserver.log
"$HERE/../xhide" 22 & H=$!; sleep 2
mpv --no-config --vo=gpu-next --gpu-api=vulkan --fs --cursor-autohide=no --loop=inf \
  "av://lavfi:testsrc=size=1920x1080:rate=60" > "$OUT/mpv.log" 2>&1 & M=$!
sleep 14; cam 01-hidden; c1=$(grep -c "live direct submit" "$L"); wait $H
sleep 1; "$HERE/../xlat" 4 250 > /dev/null; cam 02-shown; c2=$(grep -c "live direct submit" "$L")
kill $M; wait $M 2>/dev/null; sleep 1; stop_server
echo "direct submits: while hidden=$c1 after show+motion=$((c2-c1))"
log_checks
