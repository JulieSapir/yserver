#!/usr/bin/env python3
"""Summarise the branch's per-second 'atomic commits/s' lines: EBUSY and cursor latency, over seconds with motion."""
import re
import sys

PAT = re.compile(
    r"atomic commits/s cursor_only=(\d+) scene_with_cursor=(\d+) scene=(\d+) ebusy=(\d+) .*"
    r"motion_to_accept_us_max/p99=(\d+)/(\d+) motion_to_visible_us_max/p99=(\d+)/(\d+)"
)

for path in sys.argv[1:]:
    rows = [tuple(map(int, m.groups())) for m in map(PAT.search, open(path, errors="replace")) if m]
    if not rows:
        continue  # master binaries print no such line
    ebusy = sum(r[3] for r in rows)
    moving = [r for r in rows if r[7] > 0]
    if not moving:
        print(f"{path}: ebusy={ebusy}, no motion samples")
        continue
    acc = sorted(r[5] for r in moving)
    vis = sorted(r[7] for r in moving)
    mid = lambda v: v[len(v) // 2] / 1000
    print(
        f"{path}: ebusy={ebusy} motion-seconds={len(moving)} "
        f"accept p99 median {mid(acc):.1f} ms max {acc[-1] / 1000:.1f} ms | "
        f"visible p99 median {mid(vis):.1f} ms max {vis[-1] / 1000:.1f} ms"
    )
