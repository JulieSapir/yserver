#!/bin/bash
# mpv testsrc at 60 fps, 15 s quiet, then 15 s of 250 Hz motion. MODE=composed (window) or MODE=direct (fullscreen).
. "$(dirname "$0")/../lib.sh"; BIN=$1; mkdir -p "$OUT"; X=$HERE/../xlat
start_server "$BIN"
if [ "${MODE:-composed}" = direct ]; then G=--fs; else G=--geometry=1280x720+100+100; fi
mpv --no-config --vo=gpu-next --gpu-api=vulkan $G --video-sync=display-resample --loop=inf \
  --term-status-msg='STAT drop=${frame-drop-count}\n' "av://lavfi:testsrc=size=1920x1080:rate=60" > "$OUT/mpv.log" 2>&1 & M=$!
sleep 20; cam 01-quiet
q=$(tr '\r' '\n' < "$OUT/mpv.log" | grep -o 'drop=[0-9]*' | tail -1)
step motion; $X 15 250; cam 02-motion
m=$(tr '\r' '\n' < "$OUT/mpv.log" | grep -o 'drop=[0-9]*' | tail -1)
kill $M; wait $M 2>/dev/null; sleep 1; stop_server
echo "mpv dropped frames: after quiet ${q:-?}, after motion ${m:-?}"
log_checks
