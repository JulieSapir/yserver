#!/bin/bash
# Window drag at 250 Hz under a window manager (WM, default marco; any WM with title bars). The dragged window is a
# windowed mpv playing a test pattern (mpv is needed by the other tests anyway).
. "$(dirname "$0")/../lib.sh"; BIN=$1; mkdir -p "$OUT"; D=$HERE/../xdrag
start_server "$BIN"
${WM:-marco --no-composite --replace} > "$OUT/wm.log" 2>&1 & WMP=$!; sleep 2
mpv --no-config --vo=gpu-next --gpu-api=vulkan --geometry=700x450+400+250 --loop=inf \
  "av://lavfi:testsrc=size=640x360:rate=30" > "$OUT/mpv.log" 2>&1 & T=$!
W=""; for i in $(seq 1 40); do W=$(xdotool search --onlyvisible --pid $T 2>/dev/null | head -1); [ -n "$W" ] && break; sleep 0.25; done
echo "window: ${W:-none}"; [ -z "$W" ] && { kill $T $WMP; stop_server; exit 1; }
sleep 2; cam 01-window
for r in 1 2 3; do step "drag $r"; $D "$W" 10 250; sleep 1; done
kill $T $WMP 2>/dev/null; sleep 1; stop_server; log_checks
