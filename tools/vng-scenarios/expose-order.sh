# Sourced by tools/vng-shot.sh INSIDE the guest (DISPLAY=:7, cwd = artifacts).
# Expose, MapNotify and DamageNotify order and extent against Xorg: ClearArea
# clipping, ParentRelative under a Manual parent, unredirect restore damage,
# MapSubwindows order and damage-list order. Pure protocol, no WM.
#
#   tools/vng-shot.sh --server xorg --dump none --name eo-xorg \
#       --scenario tools/vng-scenarios/expose-order.sh
#   tools/vng-shot.sh --dump none --name eo-ys \
#       --scenario tools/vng-scenarios/expose-order.sh
# shellcheck shell=sh
# golden: probe-nocomp.log probe-comp.log
set -u
set +e
src=${YSERVER_REPO:?}/tools/expose-order-probe.c
cc -O1 -Wall -o probe "$src" -lxcb -lxcb-damage -lxcb-composite -lxcb-xfixes > cc.log 2>&1 || cat cc.log >&2
timeout 400 ./probe nocomp > probe-nocomp.log 2>&1
timeout 400 ./probe comp > probe-comp.log 2>&1
cat probe-nocomp.log probe-comp.log
if [ ! -x probe ]; then echo "fail: probe did not build (cc.log)" > RESULT
elif ! grep -qx 'done' probe-nocomp.log || ! grep -qx 'done' probe-comp.log; then
    echo "fail: the probe stopped early (probe-*.log)" > RESULT
else echo pass > RESULT; fi
