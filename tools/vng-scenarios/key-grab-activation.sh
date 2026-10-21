# Sourced by tools/vng-shot.sh INSIDE the guest (DISPLAY=:7, cwd = artifacts).
# GH #277: a passive grab's activating key/button press is reported on the
# grab window (Xorg ActivatePassiveGrab); only later events take the
# owner_events walk (key-grab-activation-probe.c).
# shellcheck shell=sh
# golden: probe.log
# mask: ^keycodes: .* => keycodes: <masked> -- keycodes come from the guest keymap
set -u
set +e
cc -O1 -o probe "${YSERVER_REPO:?}/tools/vng-scenarios/key-grab-activation-probe.c" -lxcb -lxcb-xtest -lxcb-xinput > cc.log 2>&1 || cat cc.log >&2
timeout 120 ./probe > probe.log 2>&1
cat probe.log
if [ ! -x probe ]; then echo "fail: probe did not build (cc.log)" > RESULT
elif ! grep -qx done probe.log; then echo "fail: the probe stopped early (probe.log)" > RESULT
else echo pass > RESULT; fi
