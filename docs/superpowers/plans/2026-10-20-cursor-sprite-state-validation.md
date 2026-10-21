# Cursor sprite pool and atomic cursor state — validation record

Spec: [`../specs/2026-10-19-cursor-sprite-state-design.md`](../specs/2026-10-19-cursor-sprite-state-design.md).
Plans: [`2026-10-19-cursor-sprite-state-a.md`](2026-10-19-cursor-sprite-state-a.md) (P2a),
[`2026-10-19-cursor-sprite-state-b.md`](2026-10-19-cursor-sprite-state-b.md) (P2b).
Branch `feat/cursor-sprite-state` on master `38c36935`; code revision validated: `ee34fd2d`.

## Software

| Check | Result |
| --- | --- |
| `cargo test --workspace --locked --features xdmcp` | 4227 passed, 0 failed |
| Whole `-- --ignored` suite on lavapipe | 565 passed, 0 failed |
| `cargo clippy --workspace --all-targets --locked --features xdmcp -- -D warnings` | clean |
| FreeBSD 15 cross-check (`--target x86_64-unknown-freebsd`, local sysroot) | clean |
| `tools/check-layout.sh` | ok |
| CI on `ee34fd2d` (builds, freebsd, macos, vng) | all six jobs pass |
| New lifecycle and pressure tests | 5x under 16 busy loops; GPU-waiting tests 20x; each named mutation fails at its assertion (task reports) |

Copied scenarios still log the pre-existing `TRANSFER_SRC`-only image-view VUID from master (`cde08e9e`).

## Hardware (2026-10-20)

Host: Linux 7.2.8-gentoo-dist; AMD Raphael iGPU (amdgpu, RADV Mesa 26.1.8) `card0` HDMI-A-1 1920x1080@60;
NVIDIA RTX 5060 Ti (open module 615.71.09) `card1` HDMI-A-2 1920x1080@60. One output per card. Release builds of
master `38c36935` and the branch, started by `tools/cursor-bench` on display `:5`.

Both devices log `cursor_state_path=atomic reason=qualified`.

`bench.sh` (motion, drag, composed animation, direct animation, hidden cursor), master vs branch:

| Card | Result |
| --- | --- |
| AMD `card0` | Branch equals master: 0 timeouts, 0 panics, 0 EBUSY, 0 dropped mpv frames, drag lag p99 12 px on both, one ~190 ms motion peak on both. Direct scanout is not reached on either build (`add_fb2` rejects mpv's linear buffer). |
| NVIDIA `card1` | Branch equals master: direct animation 2070 direct submits (master 2068); hidden 791 while hidden, 661 after show+motion (master 793/663); 0 timeouts, 0 panics, 0 EBUSY, 0 dropped frames, 0 Xid. Master's motion run had one 3.2 s stall (the known RANDR disable stall); the branch's worst was 191 ms. |

Lifecycle script (2-frame animated root cursor, 300 ms per frame), both builds on both cards:

| Case | AMD | NVIDIA |
| --- | --- | --- |
| Animated cursor, composed | animates, 0 errors | animates, 0 errors |
| Animated cursor over fullscreen mpv | composed on both builds (no direct on this driver); 0 dropped frames | direct; branch 0 dropped frames (master 75); see the known cost below |
| DPMS off/on | cursor returns | cursor returns |
| `xrandr --off`/`--auto`, 3 cycles | cursor returns each time | cursor returns each time |
| Shutdown | clean, no pool-drop warnings | clean, no pool-drop warnings |

Observed with the user:

- VT switch (NVIDIA, `tests/vt.sh`, 5 switches, one while the pointer moved): every resume logs
  `rearm_cursor: done — shown=1 failed=0`; the picture kept updating and the cursor was visible and moving after
  each return; 0 errors.
- Physical hot-unplug (HDMI cable pulled and replugged 3 times on each card): each disconnect drops the active
  scanout through the physical rescan path, each reconnect re-enables 1920x1080@60, cursor commits resume at the
  pre-unplug rate immediately, the cursor was seen moving and animating after each reconnect; shutdown clean with
  no pool-drop warnings; 0 errors, 0 Xid.

## Known cost

An animated cursor over held direct scanout: each image frame is a cursor-only commit, which meets the in-flight
direct flip with EBUSY; the show defers and the direct hook unflips. On NVIDIA this produced 76 EBUSY/unflip pairs in
about 26 s (one per animation frame), with no dropped client frames and no ownership leak. This is the cost the spec
records for P2; P3 (cursor changes ride the frame) removes it.

## Not executed

- Dual-head topology (one output per card here).
- VT switch on AMD.
- A paravirtualized driver (virtio-gpu, kernel >= 6.8): needs a VM.
- Intel, Asahi, Qualcomm.
