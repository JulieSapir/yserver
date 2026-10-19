# Sourced by tools/vng-shot.sh INSIDE the guest (DISPLAY=:7, cwd = artifacts).
# Property data byte order: a raw big-endian ('B') and a raw little-endian
# ('l') client write and read the same window properties (format 8/16/32,
# Append/Prepend, long_offset windows, delete, type mismatch, ListProperties,
# PropertyNotify, RotateProperties and its errors) and a RandR output
# property. Each value is printed as the reader decodes it, so the log is
# byte-order independent: on Xorg every client reads the same numbers
# whoever wrote them.
# Not in the golden (PROBE_FULL=1 prints them): XI2 device properties, where
# Xorg leaves a BE writer's data unswapped on input but swaps it on output
# (Xi/xiproperty.c SProcXIChangeProperty vs ProcXIGetProperty) and yserver
# swaps both ways.
# shellcheck shell=sh
# golden: probe.log
set -u
set +e
python3 -I "${YSERVER_REPO:?}/tools/vng-scenarios/prop-order-probe.py" > probe.log 2>&1
cat probe.log
if [ ! -e PROBE-DONE ]; then echo "fail: the probe stopped early (probe.log)" > RESULT
else echo pass > RESULT; fi
