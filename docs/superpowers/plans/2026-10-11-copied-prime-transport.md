# Copied PRIME transport: system memory and a padded width, so the sink can import and read it — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The copied scanout route (render on GPU A, scan out on GPU B) works across two GPUs that cannot reach each other's VRAM (Task 1), and at output widths where A's linear pitch is not a multiple of B's import alignment. The known widths are 1366, 1440 and 1680 on an NVIDIA renderer with a radv sink.

**Architecture (Task 1, added 2026-10-11 after the memory probe; the user approved Tasks 2–3 before it was added, so Task 1 needs its own approval):** the copied transport's memory moves from `DEVICE_LOCAL` (renderer VRAM, which another GPU cannot reach without P2P) to a compatible non-`DEVICE_LOCAL` type on the renderer, i.e. system memory. Same-device exports (TFP) keep `DEVICE_LOCAL`.

**Architecture (Task 2):** Only the DMA-BUF **transport** of `CopiedRenderSource` gets a padded width: the logical width rounded up to a multiple of 64 pixels (256 bytes for BGRA8). Everything else keeps the logical extent:
- the optimal render target;
- `width()` / `height()`;
- every copy region (render target → transport on A, transport → destination on B);
- the readbacks and digests;
- the destination BOs.

The padded columns are never read into a destination. If an import still fails, the existing exact-plan qualification reports it as today.

**Tech Stack:** Rust, Vulkan (ash), `VK_EXT_image_drm_format_modifier`, DMA-BUF.

**Finding (Task 1):** `docs/superpowers/findings/2026-10-11-copied-prime-transport-device-local-memory.md` on the reference branch (`e292cf8d`). It measured, at 1920×1080 linear, NVIDIA renderer → radv sink:
- all five NVIDIA memory types allocate, export and import;
- the two `DEVICE_LOCAL` types (1: VRAM; 4: `DEVICE_LOCAL | HOST_VISIBLE`, the BAR) make the radv copy fail (`amdgpu: Not enough memory for command submission` → `VK_ERROR_DEVICE_LOST`, no GPU reset);
- the three system-memory types (0: no flags; 2: `HOST_VISIBLE | HOST_COHERENT`; 3: `+ HOST_CACHED`) copy with every pixel matching;
- today the start-up qualification therefore rejects the copied route on such hosts; with no other route, yserver does not start on that output.

**Finding (Task 2):** `docs/superpowers/findings/2026-10-11-copied-prime-nvidia-radv-linear-pitch.md` on the fork's reference branch `feat/phase-c0-atomic-kms-migration` (`b4e83809`). It measured:
- NVIDIA RTX 5060 Ti (driver 615.71.09) gives a linear pitch of 5504 bytes at width 1366 (128-byte aligned), and radv rejects it with `ERROR_INVALID_DRM_FORMAT_MODIFIER_PLANE_LAYOUT_EXT`;
- Mesa 26.1.8 radv requires a linear pitch that is a multiple of 256 bytes on GFX9–GFX11 (`ac_surface.c` `ac_surface_get_pitch_align`, `ac_surface_override_offset_stride`);
- 1920, 2560, 3840, 1280 and 1000 import on this host; the extra widths 1440 and 1680 are predicted from the 128-byte rule and are confirmed by this plan's test.

**Direction (user, 2026-10-11).** Reverse PRIME (#95, AprilGrimoire) was built and validated in the other direction: the AMD Radeon 780M iGPU renders and the NVIDIA RTX 4070 shows, at 1600×1200. There the renderer's `DEVICE_LOCAL` memory is system RAM (UMA), and 1600×4 bytes is 256-aligned, so neither defect appears.
- **Who renders:** the primary card is the first card, in enumeration order, with a connected connector (`platform/drm.rs` `primary_kms_card_candidate_index`). Its render node renders everything.
- **When these defects appear:** when a discrete GPU is primary and a monitor is also on an integrated GPU's output, e.g. a desktop whose dGPU enumerates as `card0`, or a render-node override.
- **Effect on April's direction:**
  - On an APU renderer, the system-memory preference moves the transport from the `DEVICE_LOCAL` carve-out to a GTT type. Both are system RAM, so the dGPU sink still reads them.
  - The padding only changes widths that are not multiples of 64.
  - Task 1 pins this direction with its own test.
- The PR text (Task 3) says this.

**Base:** `joske/master` `0e5c09ea` (layout refactor phase 2 merged), branch `fix/copied-prime-transport` (renamed from `fix/copied-transport-pitch`), worktree `~/Projects/yserver-fix-copied-pitch`. It is independent of the KMS-owner PR series and goes upstream as its own small PR.

## Global Constraints

- Behaviour changes only for the copied transport's allocation (memory type, width) and its sink import. No change to destination BOs, KMS, framebuffers or the render target.
- No environment variable or flag (joske, PR 129). No `cfg(test)` forks in production bodies. No side effects inside `debug_assert!`.
- The padding is a pure function of the logical width and the format's bytes per pixel, named and documented at its definition (why 256 bytes: radv's GFX9+ linear pitch alignment, the strictest known sink; GFX12 needs 128, Intel 64).
- Upstream file-size ceilings: no non-test `.rs` file over 5,000 lines, no test `.rs` file over 6,000.
- Test rules: **A** end-state checks; **C** the real path on hardware (here: the two GPUs of this host, no display needed); **F** new tests 5× under `nproc` busy loops.
- **Hardware safety for the `two_gpu` tests:**
  - only render nodes and Vulkan device files are opened, never a `/dev/dri/card*` node;
  - `nvidia-smi` shows no `ERR!` before the run;
  - `journalctl -k -b | grep -c Xid` does not grow.
- Commits: user's git identity, trailers `Implemented-By: codex (…)` and `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`, never a session URL.

## Review Focus

1. **A copy region or readback that uses the transport's allocated extent instead of the logical one.** It would read or write the padded columns into the destination or the digest. Expected: every region uses `width()` / `height()` (logical). **The bounds evidence is Task 2's copy-region audit** (every `vk::ImageCopy`, `BufferImageCopy` and digest listed in the report with its extent). The pixel test only shows that the logical content is preserved: a copy widened to the transport width could still produce the expected first columns *(plan review r1, m-1)*.
2. **Memory accounting and the qualification probe** (`allocate_exact_for_disposable_probe`) see the padded transport. Expected: the probe uses the same padded allocation as the live pool, so it qualifies what will run.
3. **A width already a multiple of 64.** Expected: no padding; the allocation is byte-identical to today's.

## Gate

1. `cargo +nightly fmt`, then `cargo +nightly fmt -- --check`.
2. `cargo clippy --all-targets $F -- -D warnings` for `F` in `""`, `--features tcp-transport`, `--features xdmcp`.
3. `cargo test -p yserver-core --locked`; `cargo test -p yserver --lib --locked`; each integration test file under `crates/yserver/tests/`.
4. Lavapipe (the existing `--ignored` suite with the lavapipe ICD, `--skip two_gpu`).
5. `cargo test -p yserver --lib --locked two_gpu -- --ignored` with the host ICDs (no `VK_DRIVER_FILES`), with the hardware safety checks above.

---

### Task 1: The copied transport lives in system memory

**Files:**
- Modify: `crates/yserver/src/kms/vk/target.rs` (`bind_exportable_memory`, `allocate_exportable_modifier`, `allocate_copied_source_exact`)
- Test: the tests module of `target.rs` (unit), and the `two_gpu` test of `kms/vk/scanout/copied.rs` shared with Task 2

**Interfaces:**
- **Produces:** `enum ExportMemoryPlacement { DeviceLocal, CrossDevice }` (names free), passed by every caller of `bind_exportable_memory`.
  - `DeviceLocal`: today's choice (`DEVICE_LOCAL` first, then any compatible type). Used by TFP and every same-device export; their behaviour is unchanged.
  - `CrossDevice`: among the image-compatible types, the first **non-`DEVICE_LOCAL`** type. `HOST_VISIBLE` is not required (the probe's type 0 has no flags and works). If none exists, fall back to today's choice and let the existing copied qualification decide; log once at `info` that no system-memory type was found.
- `allocate_copied_source_exact` uses `CrossDevice`. The copied path is the only `CrossDevice` caller.
- **Pure helper for the unit test:** `fn pick_export_memory_type(props: &vk::PhysicalDeviceMemoryProperties, type_bits: u32, placement: ExportMemoryPlacement) -> Option<u32>`.

**Invariants:**
- Every caller of `bind_exportable_memory` is listed in the report with its placement. Only copied sources use `CrossDevice`.
- `mem_accounting` categorises the copied transport as today (it already re-categorises it as `Scanout`).

- [ ] **Step 1: Tests first.**
  - `cross_device_export_prefers_system_memory` (unit, synthetic `PhysicalDeviceMemoryProperties` copying the probe's five NVIDIA types and heaps):
    - `CrossDevice` picks type 0;
    - with type 0 masked out of `type_bits`, it picks type 2;
    - with only `DEVICE_LOCAL` types compatible, it falls back to type 1;
    - `DeviceLocal` picks type 1 in every case.

    **Mutation:** treat `DEVICE_LOCAL | HOST_VISIBLE` (type 4) as acceptable for `CrossDevice`; it must not be chosen.
  - `copied_source_transfers_from_igpu_to_dgpu_two_gpu` (April's direction): the renderer is the AMD radv context and the sink is the NVIDIA context; same transfer and pixel check at 1920×1080 and 1600×1200. It passes before and after the change; the report names the memory type chosen in each run. **Mutation:** a `CrossDevice` choice that returns no type (`None`), which must fail allocation. Only Vulkan devices and render nodes are used, never `card1`.
  - `copied_source_transfers_across_gpus_two_gpu`, `#[ignore = "needs the NVIDIA and AMD Vulkan devices"]`, at 1920×1080:
    - contexts as in Task 2's `two_gpu` test;
    - `CopiedRenderSource::allocate_exact` with the linear plan;
    - a pattern rendered on the renderer, copied through the transport, and copied on the sink into a sink-local image, matches on readback.

    **Mutation:** `CrossDevice` → today's choice. The sink submit then fails with device lost, and the test must fail, not hang.

    **Disposal contract** *(plan review r2, M-1)*:
    - The headless harness owns both contexts, the source, the sink-local image, the pattern and digest resources, and every submission resource as **one disposable attempt**, as the pool probe's aggregate owner does (`scanout.rs` ~L669, `probe.rs` ~L99).
    - Fence waits are bounded.
    - Normal destruction happens only after completion is proved on both devices.
    - On device loss or an unproved completion, the attempt keeps (leaks) the referenced graph instead of destroying it or waiting without bound. This mirrors the production probe's quarantine (`copied.rs` ~L1302).
    - The contract covers the initial failing run, the device-local mutation, and Task 2's `two_gpu` test, which reuses the harness.
- [ ] **Step 2:** Run them; they fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Gate (all five steps) and the load gate for the new tests. Record the change in `amdgpu ... Not enough memory for command submission` lines; only the mutation run may add them.
- [ ] **Step 5:** Commit `fix(kms): keep the copied PRIME transport in system memory so the sink GPU can read it`.

### Task 2: Pad the copied transport width

**Files:**
- Modify: `crates/yserver/src/kms/vk/scanout/copied.rs` (`CopiedRenderSource::allocate_exact`)
- Modify, if the transport allocation needs the padded width passed through: `crates/yserver/src/kms/vk/target.rs` (`allocate_copied_source_exact`)
- Test: the tests module of `kms/vk/scanout/copied.rs`, or its existing test file

**Interfaces:**
- **Produces:** `fn copied_transport_width(logical_width: u32) -> u32`, which rounds up to a multiple of 64 pixels for BGRA8 (a `const` alignment named for its reason).
- **Allocation:**
  - The transport is allocated and imported at `(copied_transport_width(width), height)`.
  - `allocate_transfer_resources` and the render target stay at `(width, height)`.
  - `CopiedRenderSource` keeps `width()` / `height()` = logical (they already read the render target).

**Invariants:**
- The renderer-side copy (render target → transport) and the sink-side copy (transport import → destination) both use the logical extent. Verify every `vk::ImageCopy`, `BufferImageCopy` and digest in `copied.rs`, and list them in the report.
- The sink import passes the renderer's actual offset and stride for the padded image, unchanged, as today. The fix is in what is allocated, not in what the import descriptor claims.
- A logical width that is already a multiple of 64 allocates exactly as today.

- [ ] **Step 1: Tests first.**
  - `copied_transport_width_rounds_to_64_pixels` (unit): 1366 → 1408, 1440 → 1472, 1680 → 1728, 1920 → 1920, 1 → 64, 64 → 64. **Mutation:** round to 32.
  - `copied_source_imports_on_the_sink_at_unaligned_widths_two_gpu`, `#[ignore = "needs the NVIDIA and AMD Vulkan devices"]`.
    - **Setup:** renderer and sink contexts built as production builds them for copied scanout (`VkContext::new_for_render_device` for the renderer; the sink found through the renderer context's DRM-device inventory, then `VkContext::new_transfer_for_device`).
    - **For** 1366×768, 1440×900, 1680×1050, 1920×1080 and 2560×1440: `CopiedRenderSource::allocate_exact` with the linear plan succeeds.
    - **Then** a pattern rendered into the render target, copied through the transport and read back on the sink matches at the logical extent. Use the existing probe-pattern and digest machinery the copied qualification already uses (`CopiedProbePatternPipeline`, the render/sink digests); say in the report which entry point.
    - **Mutation:** allocate the transport at the logical width. 1366, 1440 and 1680 then fail at import.
  - **If the host lacks either device, the test fails with a clear message, never skips.**
- [ ] **Step 2:** Run them; the unit test fails (missing function), and the `two_gpu` test fails at 1366 (Task 1 is already in, so its pixel check reaches the sink copy).
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Gate (all five steps), and the load gate for the new tests.
- [ ] **Step 5:** Commit `fix(kms): pad the copied PRIME transport so linear-pitch sinks can import it`.

### Task 3: Upstream PR text (coordinator)

- [ ] Prepare the PR description: the memory-type failure and the probe table, the failing widths, the radv rule with its Mesa source lines, the measured NVIDIA pitches, and the test. Push and open the PR only when the user says so.
