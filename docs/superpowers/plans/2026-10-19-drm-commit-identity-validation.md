# DRM commit identity — validation record

Plan: [`2026-10-19-drm-commit-identity.md`](2026-10-19-drm-commit-identity.md). Spec:
[`../specs/2026-10-19-drm-commit-identity-design.md`](../specs/2026-10-19-drm-commit-identity-design.md).
Branch `feat/drm-commit-identity` from master `cb8297d0`; code revision validated: `d72f9e81`.

## Software

| Check | Result |
| --- | --- |
| `cargo test --workspace --locked --features xdmcp` + whole `-- --ignored` suite (lavapipe ICD) | 4470 passed, 0 failed, 0 skipped |
| `cargo clippy --workspace --all-targets --locked --features xdmcp -- -D warnings` | clean |
| `tools/check-layout.sh` | ok |
| Scene / copied / zero-cookie lifecycle tests (7), 20 runs each under 16 busy loops | 140/140; `kms_retirement_requires_the_exact_paired_bo` 5/5 |
| Direct / degraded-unflip lifecycle tests (7), 20 runs each under 16 busy loops | 140/140 (Task 4 report) |
| `commit_identity` module, 5 consecutive runs | 23/23 each |
| Mutation per new test | each named mutation applied once and observed failing (task reports) |
| Fixture skips | none on the local lavapipe runtime; the only cursor-fixture skip (missing SYNC_FD export) prints its reason |

Copied scenarios log three VUIDs each for an image view on a `TRANSFER_SRC`-only copied-sink image. That is the
pre-existing master finding (`cde08e9e`), outside this change; no leaked-object or destroyed-in-use messages.

Cross-platform: local musl and FreeBSD cross-checks stop in native `-sys` build scripts (no target sysroot) before
reaching yserver code; the FreeBSD VM job in CI is the evidence for the portable ioctl types.

## Hardware (coordinator-run, unattended, 2026-10-19)

Host: Linux 7.2.8-gentoo-dist; AMD Raphael iGPU (amdgpu, RADV Mesa 26.1.8) `card0` HDMI-A-1 1920x1080@60 (TV);
NVIDIA RTX 5060 Ti (open module 615.71.09) `card1` HDMI-A-2. One output per card. Binaries: release builds of master
`cb8297d0` and the branch, started by `tools/cursor-bench` on display `:5`.

`bench.sh` (motion, drag, composed animation, direct animation, hidden cursor), master vs branch:

| Card | Result |
| --- | --- |
| AMD `card0` | Server starts (CRTC-in-event capability accepted). Branch equals master: 0 timeouts, 0 panics, 0 EBUSY, 0 dropped mpv frames; drag lag p99 12 px on both; one motion run peaks at ~174 ms on both. 0 unmatched identity, 0 unroutable events. Direct scanout not reached on either build: `add_fb2` rejects mpv's linear buffer (EINVAL). |
| NVIDIA `card1` | Server starts. Branch equals master: 0 timeouts, 0 panics, 0 EBUSY, 0 dropped frames; direct animation 2068 direct submits on both; hidden cursor 791 direct while hidden, 662 after show+motion (master 663). 0 unmatched identity, 0 unroutable events, 0 Xid. Master's motion run had one 3.2 s stall (the known RANDR disable stall); the branch's worst was ~174 ms. |

`tests/tearing.sh` on NVIDIA, branch:

| Case | Result |
| --- | --- |
| Vulkan immediate, cursor hidden | 527 async direct flips/s, 0.995 direct flips per Present |
| Vulkan immediate, visible moving cursor | 697 async composed flips/s |
| OpenGL swap interval 0, hidden / moving | 170 async direct/s / 663 async composed/s |
| Vulkan mailbox, fifo controls | Vsync, 57 flips/s |
| All cases | 0 async EINVAL, 0 EBUSY, 0 panics, 0 Xid, 0 unmatched identity |

Copied PRIME on real ICDs (NVIDIA + RADV, no lavapipe): the three `two_gpu_tests` and the three copied
`commit_identity` scenarios pass.

## Not executed

- Dual-head topology (one output per card here): grouped direct across CRTCs is covered by the software tests only.
- Physical copied display or degraded unflip: neither route is reached naturally on this host.
- Natural async fallback: not observed; the scripted fallback tests are its evidence.
- VT switch away/back (`tests/vt.sh` is manual).
- Intel, Asahi, Qualcomm.
