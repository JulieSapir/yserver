# Common setup for the cursor bench. Environment:
#   CARD=/dev/dri/cardN   KMS device yserver drives (required)
#   CAM=/dev/video0       optional webcam for photos (unset = no photos)
#   DISP=:5               X display to use
#   YLOG                  RUST_LOG (the default keeps the per-commit debug lines)
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
: "${CARD:?set CARD=/dev/dri/cardN}"
DISP=${DISP:-:5}; export DISPLAY=$DISP
YLOG=${YLOG:-info,yserver::kms::render::backend::kms=debug}
cam() { [ -n "${CAM:-}" ] && ffmpeg -loglevel error -y -f v4l2 -input_format mjpeg -video_size 1280x720 -i "$CAM" -frames:v 1 "$OUT/$1.jpg"; return 0; }
step() { echo "== $(date +%T) $*"; }
start_server() { # $1 = yserver binary
  YSERVER_LOOP_TELEMETRY=1 RUST_LOG=$YLOG YSERVER_DRM_DEVICE=$CARD "$1" "$DISP" -nolisten tcp > "$OUT/yserver.log" 2>&1 &
  SP=$!
  sock=/tmp/.X11-unix/X${DISP#:}
  for i in $(seq 1 120); do [ -S "$sock" ] && break; sleep 0.25; done
  sleep 2
  CONNECTOR=$(xrandr | awk '/ connected/ {print $1; exit}'); echo "connector: $CONNECTOR"
}
stop_server() {
  kill -TERM $SP; for i in $(seq 1 80); do kill -0 $SP 2>/dev/null || break; sleep 0.25; done
  kill -0 $SP 2>/dev/null && { echo "SERVER STILL RUNNING"; kill -KILL $SP; } || echo "server exited"
  wait $SP; echo "exit=$?"
}
log_checks() {
  L=$OUT/yserver.log
  for p in "timed out waiting for" "panicked" " ERROR " "EBUSY"; do printf '%-26s %s\n' "$p" "$(grep -c -- "$p" "$L")"; done
  printf '%-26s %s\n' "live direct submits" "$(grep -c 'live direct submit' "$L")"
  printf '%-26s %s\n' "cursor path" "$(grep -o 'atomic_cursor=[a-z]*' "$L" | sort | uniq -c | tr '\n' ' ')"
}
