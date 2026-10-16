# Cursor bench — hardware cursor in the wlroots model

Compares a yserver build of master with a build of the atomic-cursor branch on one GPU. It measures core stalls
while the pointer moves, window-drag lag, frame drops under animation, direct scanout (also with the cursor hidden
before a fullscreen client starts), and cursor latency on the branch. The bench changes nothing on the machine.
Each test starts its own yserver on a free display (`:5` by default), driving the KMS device you name.

## Needs

- A free KMS device with a connected display, and no other DRM master on it: run from a text console, or on a card
  your session is not using.
- `cc`, xcb + xcb-xtest, libX11 + libXfixes headers, `mpv` (Vulkan), `xrandr`, `xdotool`, `xset`, and a window
  manager with title bars for the drag test (`WM=...`; default `marco --no-composite --replace`).
- Optional: a webcam for photos (`CAM=/dev/video0`).

## Run

```sh
make                                   # builds xlat, xdrag, xhide
CARD=/dev/dri/card0 ./bench.sh out master=/path/to/yserver-master branch=/path/to/yserver-branch
```

About 3 minutes per binary. Results land in `out/<name>-<test>/` (`yserver.log`, the probe outputs, photos), plus
one summary per test on stdout and the branch's latency summary at the end.

**VT switch (manual, optional, about 2 minutes).** From a text console, run
`CARD=/dev/dri/cardN OUT=out/vt ./tests/vt.sh /path/to/yserver-branch` and follow the prompts.

## Tearing run (manual hardware test)

From a text console or another session that does not own the selected card, run:

```sh
CARD=/dev/dri/cardN OUT=out/tearing ./tests/tearing.sh /path/to/yserver
```

The script starts and stops its own yserver for each 15-second case (set `DURATION=...` to change that). It runs
Vulkan immediate with the cursor hidden, then with the visible cursor moving through `xdotool` at a 250 Hz target;
OpenGL with swap interval 0 in both cursor states; and Vulkan mailbox and fifo controls. The fifo case uses the same
Vulkan test client as the immediate cases for the latency comparison. Each case writes `yserver.log` and `mpv.log`
under its own `OUT` directory. The output includes the selected card's per-second summaries and rate/latency
aggregates, plus `log_checks` EBUSY/panic/error counts. `present_latency_summary` provides the Vsync baseline while
the fifo case remains Off. The script monitors `journalctl -k -b`; an NVIDIA Xid terminates the workload and stops
the matrix before another case starts.

In `tearing_summary`, `async_flip_turnaround_us_*` measures from the accepted
async ioctl timestamp in `CLOCK_MONOTONIC` to yserver receiving its page-flip
event in `CLOCK_MONOTONIC`. It includes kernel/userspace event-delivery delay,
because that delay determines when yserver can submit the next flip.
`present_to_flip_event_us_*` measures from Present receipt in
`CLOCK_MONOTONIC` to the kernel's flip-completion timestamp, normalized to
`CLOCK_MONOTONIC`. Linux event timestamps are checked against both
`CLOCK_MONOTONIC` and `CLOCK_BOOTTIME` at receipt; implausible timestamps are
reported as `n/a`. Present UST values sent to clients remain in the kernel's
original clock domain.

| Case | Expected result |
|---|---|
| Vulkan immediate, cursor hidden | `async_direct_flips/s` follows `client_present_direct/s` above refresh; async flip turnaround, `present_to_submit_us_direct`, and Present-to-flip-event latency are reported. |
| Vulkan immediate, visible cursor moving | composed async flips; cursor follows the pointer; no persistent Vsync submits after handoff; composed GPU cost, `present_to_submit_us_composed`, and Present-to-flip-event latency are reported. |
| OpenGL, swap interval 0, hidden and visible | Same direct and composed behavior as Vulkan immediate. |
| Vulkan mailbox and fifo controls | No tearing state and no async commits; fifo provides the same-client Vsync latency baseline. |

Expect Xid 0, no `panicked` or `timed out waiting for` lines, and EBUSY at or below the #244 baseline. The aggregate
latency p50/p99 values are means of the per-second summary percentiles; the printed max is the greatest per-second
maximum. Keep the raw per-second lines when comparing runs, especially when async flip turnaround limits the direct
rate below the client's Present rate.

## What to expect (AMD / Intel)

| test | master (legacy cursor ioctls) | branch |
|---|---|---|
| motion: `rtt ... >8ms=` | 0 expected; amdgpu and i915 apply legacy moves without waiting | 0 |
| drag: `window lag px p99` | baseline | within ~1 px of master |
| anim-composed / anim-direct: `>8ms=`, `mpv dropped frames` | 0 / 0 | 0 / 0 |
| anim-direct: `live direct submits` | > 0 | > 0 |
| hidden: `direct submits: while hidden=` | **0** (a hidden cursor counts as a software cursor) | **> 0** |
| `EBUSY`, `timed out waiting for`, `panicked` | 0 | 0 |
| branch latency, `visible p99 median` | – | ≤ 33.4 ms (two frames at 60 Hz) |

The branch moves the cursor at most once per vblank, inside the scene's commit or alone when the CRTC is idle. On
amdgpu and i915 a legacy move applied immediately, so the branch adds up to a frame of cursor latency there. The
latency line is the number to look at. On NVIDIA (measured): motion-to-visible p99 ≈ 20 ms, 0 stalls, where the
legacy cursor stalled the core about 1200 times in 20 s.

Known and unrelated: a RANDR mode change can block for ~3 s on nvidia-drm with every build (the CRTC disable waits
for a flip event); it shows as one `rtt max≈3200 ms` in the motion test.
