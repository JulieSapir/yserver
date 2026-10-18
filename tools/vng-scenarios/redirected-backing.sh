# Sourced by tools/vng-shot.sh INSIDE the guest (DISPLAY=:7, cwd = artifacts).
# What a Manually redirected window's backing holds after client draws, resizes
# and tree changes (plan 2026-10-18-redirected-backing-geometry). Diffed live
# against Xorg; raw damage rectangles go to damage-raw.log, not the golden.
# shellcheck shell=sh
# golden: probe.log
set -u
set +e
src=${YSERVER_REPO:?}/tools/vng-scenarios/redirected-backing-probe.c
cc -O1 -o probe "$src" -lX11 -lXext -lXcomposite -lXdamage > cc.log 2>&1 || cat cc.log >&2
timeout 120 ./probe > probe.log 2>&1
cat probe.log
if [ ! -x probe ]; then echo "fail: probe did not build (cc.log)" > RESULT
elif ! grep -qx done probe.log; then echo "fail: the probe stopped early (probe.log)" > RESULT
else echo pass > RESULT; fi
