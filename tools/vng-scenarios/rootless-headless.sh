# Sourced by tools/vng-shot.sh INSIDE the guest (DISPLAY=:7, cwd = artifacts),
# with yserver started `--headless --rootless --rootless-record <out>/rec` by
# rootless-headless-host.sh. Real clients (xterm with a border, xmessage,
# xeyes when installed) plus a popup and a SHAPEd window, then
# rootless-check.py compares every recorded frame with GetImage.
# shellcheck shell=sh
set -u
set +e
scen=${YSERVER_REPO:?}/tools/vng-scenarios
xterm -geometry 40x8+40+40 -bw 3 > xterm.log 2>&1 &
xmessage -geometry +40+260 "rootless headless" > xmessage.log 2>&1 &
if command -v xeyes > /dev/null; then xeyes -geometry 120x120+420+40 > xeyes.log 2>&1 & fi
python3 "$scen/rootless-clients.py" > clients.log 2>&1 &
timeout 30 xdotool search --sync --onlyvisible --class xterm > /dev/null 2>&1
sleep 3
xwininfo -root -tree > tree.log 2>&1
if [ ! -s rec/events.log ]; then
    echo "fail: no rootless recording (rec/events.log)" > RESULT
elif python3 "$scen/rootless-check.py" rec > check.log 2>&1; then
    echo pass > RESULT
else
    echo "fail: $(grep -m1 '^fail' check.log)" > RESULT
fi
cat check.log
