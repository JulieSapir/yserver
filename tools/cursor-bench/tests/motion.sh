#!/bin/bash
# Pure motion at 250 Hz, then a RANDR mode change and a gamma change while moving, then shutdown while moving.
. "$(dirname "$0")/../lib.sh"; BIN=$1; mkdir -p "$OUT"; X=$HERE/../xlat
start_server "$BIN"; cam 00-start
step idle; $X 5 1
step motion-250hz-20s; $X 20 250; cam 01-moving
step randr-while-moving; $X 8 250 & XP=$!; sleep 2
alt=$(xrandr | awk -v c="$CONNECTOR" '$1==c {f=1; next} f && /^ / {print $1} f && !/^ / {exit}' | sed -n 2p)
xrandr --output "$CONNECTOR" --mode "$alt"; sleep 2; cam 02-alt-mode; xrandr --output "$CONNECTOR" --auto; wait $XP
step gamma-while-moving; $X 4 250 & XP=$!; sleep 1
xrandr --output "$CONNECTOR" --gamma 1.3:1.0:0.7; sleep 1; xrandr --output "$CONNECTOR" --gamma 1:1:1; wait $XP
step shutdown-while-moving; $X 6 250 > "$OUT/xlat-shutdown.txt" 2>&1 & XP=$!; sleep 2
stop_server; kill $XP 2>/dev/null; wait $XP 2>/dev/null
log_checks
