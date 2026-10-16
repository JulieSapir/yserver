#!/bin/bash
# MANUAL hardware run: immediate direct and composed tearing, plus mailbox/fifo controls.
. "$(dirname "$0")/../lib.sh"
source "$HERE/aggregate.sh"
BASE=${OUT:-out/tearing}
DURATION=${DURATION:-15}
M=
C=
SP=
XID_MON=
XH=

cleanup() {
  if [ -n "$XH" ]; then kill -TERM "$XH" 2>/dev/null || true; wait "$XH" 2>/dev/null || true; XH=; fi
  if [ -n "$XID_MON" ]; then kill -TERM "$XID_MON" 2>/dev/null || true; wait "$XID_MON" 2>/dev/null || true; XID_MON=; fi
  if [ -n "$M" ]; then kill -TERM "$M" 2>/dev/null || true; wait "$M" 2>/dev/null || true; M=; fi
  if [ -n "$C" ]; then kill -TERM "$C" 2>/dev/null || true; wait "$C" 2>/dev/null || true; C=; fi
  if [ -n "$SP" ] && kill -0 "$SP" 2>/dev/null; then stop_server; SP=; fi
}
trap cleanup EXIT INT TERM

xid_journal_cursor() {
  journalctl -k -b --no-pager -n 0 --show-cursor 2>/dev/null \
    | sed -n 's/^-- cursor: //p' | tail -n 1
}

xid_count_after_cursor() {
  local kernel_log
  kernel_log=$(journalctl -k -b --after-cursor="$1" --no-pager -o cat 2>/dev/null) || { echo unavailable; return; }
  printf '%s\n' "$kernel_log" | awk '/NVRM: Xid/ { count++ } END { print count + 0 }'
}

if ! journalctl -k -b --no-pager -n 0 >/dev/null 2>&1; then
  echo "cannot read the current kernel journal; Xid monitoring is required for this matrix" >&2
  exit 1
fi

watch_xid() {
  local cursor=$1 count
  while [ -n "$SP" ] && kill -0 "$SP" 2>/dev/null; do
    count=$(xid_count_after_cursor "$cursor")
    if [[ ! "$count" =~ ^[0-9]+$ ]]; then
      : > "$OUT/xid-monitor-error"
      echo "kernel journal became unreadable during $OUT; terminating the workload" >&2
      [ -n "$M" ] && kill -TERM "$M" 2>/dev/null || true
      [ -n "$C" ] && kill -TERM "$C" 2>/dev/null || true
      return
    fi
    if [ "$count" -gt 0 ]; then
      : > "$OUT/xid-detected"
      echo "NVIDIA Xid detected during $OUT; terminating the workload" >&2
      [ -n "$M" ] && kill -TERM "$M" 2>/dev/null || true
      [ -n "$C" ] && kill -TERM "$C" 2>/dev/null || true
      return
    fi
    sleep 0.5
  done
}

move_cursor_250hz() {
  local size width height x y step=0
  size=$(xrandr --current | awk '/\*/ { split($1, mode, "x"); print mode[1], mode[2]; exit }')
  read -r width height <<< "$size"
  width=${width:-800}; height=${height:-600}
  while kill -0 "$M" 2>/dev/null; do
    x=$((40 + step % (width > 80 ? width - 80 : 1)))
    y=$((40 + (step * 7) % (height > 80 ? height - 80 : 1)))
    xdotool mousemove "$x" "$y" >/dev/null 2>&1 || return
    step=$((step + 1))
    sleep 0.004
  done
}

run_case() {
  local name=$1 api=$2 swap=$3 cursor=$4
  OUT=$BASE/$name
  mkdir -p "$OUT"
  export OUT
  step "$name (${DURATION}s)"
  rm -f "$OUT/xid-detected"
  rm -f "$OUT/xid-monitor-error"
  local xid_delta xid_cursor elapsed duration_quarters
  xid_cursor=$(xid_journal_cursor)
  if [ -z "$xid_cursor" ]; then
    echo "could not get a kernel journal cursor for Xid monitoring" >&2
    return 1
  fi
  start_server "$BIN"
  local mpv_args=(--no-config --vo=gpu-next --fs --loop=inf --untimed --cursor-autohide=no)
  if [ "$api" = vulkan ]; then
    mpv_args+=(--gpu-api=vulkan "--vulkan-swap-mode=$swap")
  else
    mpv_args+=(--gpu-api=opengl --opengl-swapinterval=0)
  fi
  if [ "$cursor" = hidden ]; then
    # Hide before mpv starts so every hidden case exercises direct scanout.
    "$HERE/../xhide" "$((DURATION + 5))" & XH=$!
    sleep 2
  fi
  mpv "${mpv_args[@]}" --term-status-msg='STAT drop=${frame-drop-count}\n' \
    "av://lavfi:testsrc=size=1920x1080:rate=240" > "$OUT/mpv.log" 2>&1 &
  M=$!
  if [ "$cursor" = visible ]; then move_cursor_250hz & C=$!; fi
  watch_xid "$xid_cursor" & XID_MON=$!
  duration_quarters=$(awk -v duration="$DURATION" 'BEGIN { print int(duration * 4) }')
  elapsed=0
  while (( elapsed < duration_quarters )); do
    sleep 0.25
    elapsed=$((elapsed + 1))
    { [ -f "$OUT/xid-detected" ] || [ -f "$OUT/xid-monitor-error" ]; } && break
  done
  local xid_during
  xid_during=$(xid_count_after_cursor "$xid_cursor")
  if [[ ! "$xid_during" =~ ^[0-9]+$ ]]; then
    : > "$OUT/xid-monitor-error"
    [ -n "$M" ] && kill -TERM "$M" 2>/dev/null || true
    [ -n "$C" ] && kill -TERM "$C" 2>/dev/null || true
  elif [ "$xid_during" -gt 0 ]; then
    : > "$OUT/xid-detected"
    [ -n "$M" ] && kill -TERM "$M" 2>/dev/null || true
    [ -n "$C" ] && kill -TERM "$C" 2>/dev/null || true
  fi
  if [ -n "$XID_MON" ]; then kill -TERM "$XID_MON" 2>/dev/null || true; wait "$XID_MON" 2>/dev/null || true; XID_MON=; fi
  if [ -n "$C" ]; then kill -TERM "$C" 2>/dev/null || true; wait "$C" 2>/dev/null || true; C=; fi
  if [ -n "$XH" ]; then kill -TERM "$XH" 2>/dev/null || true; wait "$XH" 2>/dev/null || true; XH=; fi
  kill -TERM "$M" 2>/dev/null || true
  wait "$M" 2>/dev/null || true
  M=
  sleep 1
  stop_server
  SP=
  xid_delta=$(xid_count_after_cursor "$xid_cursor")
  if [[ ! "$xid_delta" =~ ^[0-9]+$ ]]; then
    xid_delta=unavailable
    : > "$OUT/xid-monitor-error"
  fi
  print_tearing_aggregate "$OUT/yserver.log" "$CARD_DEVICE_KEY"
  print_present_latency_aggregate "$OUT/yserver.log" "$CARD_DEVICE_KEY"
  log_checks
  printf '%-26s %s\n' "NVIDIA Xid during run" "$xid_delta"
  if [[ "$xid_delta" =~ ^[0-9]+$ ]] && [ "$xid_delta" -gt 0 ]; then
    echo "NVIDIA Xid detected during workload or teardown; stopping before the next case" >&2
    return 1
  fi
  if [ -f "$OUT/xid-detected" ]; then
    echo "NVIDIA Xid detected; stopping before the next case" >&2
    return 1
  fi
  if [ -f "$OUT/xid-monitor-error" ]; then
    echo "kernel journal monitoring failed; stopping before the next case" >&2
    return 1
  fi
}

main() {
  BIN=${1:?usage: CARD=/dev/dri/cardN OUT=out/tearing ./tests/tearing.sh /path/to/yserver}
  local card_device_hex card_device_major card_device_minor
  card_device_hex=$(stat -c '%t:%T' "$CARD") || return 1
  card_device_major=${card_device_hex%%:*}
  card_device_minor=${card_device_hex##*:}
  CARD_DEVICE_KEY=$(printf '%d:%d' "0x$card_device_major" "0x$card_device_minor")

  if ! journalctl -k -b --no-pager -n 0 >/dev/null 2>&1; then
    echo "cannot read the current kernel journal; Xid monitoring is required for this matrix" >&2
    return 1
  fi

  run_case vulkan-immediate-hidden vulkan immediate hidden || return 1
  run_case vulkan-immediate-moving vulkan immediate visible || return 1
  run_case opengl-swapinterval-0-hidden opengl immediate hidden || return 1
  run_case opengl-swapinterval-0-moving opengl immediate visible || return 1
  run_case vulkan-mailbox-control vulkan mailbox hidden || return 1
  run_case vulkan-fifo-control vulkan fifo hidden || return 1
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  main "$@"
fi
