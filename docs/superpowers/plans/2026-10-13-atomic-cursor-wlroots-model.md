# Hardware cursor in the wlroots model — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** drive the hardware cursor through atomic commits in the wlroots model on every driver that exposes atomic
cursor planes. This lets the NVIDIA software-cursor policy go, and lets direct scanout engage when the cursor is
hidden. No KMS thread.

**Architecture:** one commit stream per CRTC with at most one commit in flight.
- The cursor plane's `CRTC_X`/`CRTC_Y` ride every scene commit on that CRTC: shared flip, copied flip, direct
  scanout and composed unflip. Grouped transactions carry every participating CRTC's position.
- When nothing is in flight on the CRTC, motion sends a cursor-only `NONBLOCK | PAGE_FLIP_EVENT` commit. Further
  motion coalesces (latest wins) until it retires.
- While a cursor-only commit is in flight, the scene and direct scanout wait for it exactly as they wait for their own
  flip, and they are resumed by its retirement.
- Devices whose cursor planes lack atomic position properties, or that expose no cursor planes, keep master's legacy
  cursor ioctls.

**Tech Stack:** Rust, `drm` crate atomic requests, nvidia-drm (open kernel modules), amdgpu, i915, the 3b
`OutputSink` / `CursorPlaneSink` seam (`crates/yserver-render/src/kms/render/scene/sink.rs`).

**Evidence (spike, measured 2026-10-13 on card1, master 7b5fff83, branch `spike/nvidia-atomic-cursor`):**
- **Tests compared:** software cursor, legacy hardware cursor and this model, on pure motion, window drag, windowed
  mpv plus motion, and fullscreen mpv plus motion.
- **Result:** this model matched the software cursor on every test: 0 round trips over 8 ms, drag p99 12 px, 60 fps,
  0 mpv drops. It got 2068 direct submits where the software cursor gets 0. EBUSY was 0, and NONBLOCK ioctl time had
  a p99 of 60–150 µs. The legacy cursor had 1200 stalls.
- **Root cause of the legacy stall:** nvidia-drm has no `atomic_async_check`/`atomic_async_update`, and its own
  `nv_drm_atomic_commit` ignores `legacy_cursor_update`. A legacy cursor ioctl is therefore a blocking commit that
  waits for the pending flip and its own vblank (`nvidia-drm-modeset.c:746`, `:897`).
- **AMD and i915:** amdgpu (`atomic_async_*`) and i915 (`intel_legacy_cursor_update`) apply legacy moves immediately,
  so on them this model trades an immediate update for added cursor latency: up to two frames in the worst case,
  when motion arrives just after a commit started and waits for its retirement. The user chose one model for every
  driver (2026-10-13); Task 7 measures that latency on amdgpu.
- **Spike diff:** `~/Projects/claude-scratch/yserver-pr1/bin-spike/spike-final.diff`. Reference only; its scene parts
  predate 3b and it was NVIDIA-only.

## Global Constraints

- **Path choice per device:** the atomic path applies to a device when every active CRTC has a cursor plane with
  `CRTC_X` and `CRTC_Y`. Otherwise the device keeps master's legacy `set_cursor2` / `move_cursor` calls exactly (a
  test pins this). The choice is by capability, never by driver name. The NVIDIA software-cursor policy is deleted.
- No environment variable switches behaviour (project rule). `YSERVER_LOOP_TELEMETRY` stays a pure diagnostic.
- **No test-only behaviour in production:**
  - no `cfg(test)` forks or test-only behaviour in production bodies;
  - test seams are `#[cfg(test)]` items production does not call, or constructor parameters;
  - no side effects inside `debug_assert!`;
  - no sleeps to order events.
- **Test rules:**
  - **A**, end-state checks: no cursor commit left in flight, no unsent position, the recorded commit sequence exact.
  - **B**, advance only through `Backend` trait entries or `yserver_core::core_loop::run_core`.
  - **C**, hardware validation per real-path task (Task 7).
  - **F**, each new test 5 times under 16 `nproc` busy loops started outside the tests and killed afterwards.
- Every test names the mutation that must make it fail; run it once.
- Show/hide and image upload keep master's legacy ioctls (`set_cursor2`); only positions go atomic.
- **Only accepted commits are in flight.** A commit counts as in flight only after its ioctl returned success. A
  rejected cursor-only commit leaves no in-flight state. EBUSY is counted and warned (it is a bookkeeping bug), never
  retried in a loop.
- **Rejections.**
  - **EBUSY on a cursor-only commit (M-6):**
    - The position stays unsent, and a scene or direct commit may carry it.
    - The CRTC is marked `cursor_retry_waits_for_flip`: no further cursor-only commit is attempted on it until the
      next flip event on that CRTC. An EBUSY means something is pending there, so that event will come.
    - New motion does not clear the mark.
  - **Any other rejection (EINVAL, ENOSPC, …) of a request carrying cursor fields** follows master's legacy rule
    (`platform/cursor.rs` ~L619): hide the plane through the legacy ioctl, and only after a successful hide mark the
    CRTC for transient software fallback (`note_einval`).
    - If the hide fails, hardware ownership is retained and the CRTC enters **hide recovery**:
      - it carries no cursor fields;
      - the outcome wakes the scene and requests a direct unflip when needed, through the existing
        `handle_cursor_move_outcome` handoff (`backend/kms/scanout.rs` ~L267);
      - the hide is retried on later **pointer motion** on that CRTC (M-3, round 4), at most 3 times, at most once per
        scheduling pass. Motion is what makes a frozen sprite visible, so recovery needs no unrelated flip; without
        motion the frozen sprite is where the pointer is;
      - a successful retry completes the software fallback;
      - after the third failure the device's hardware cursor is marked `permanently_disabled` with an error log, and
        the sprite stays where the kernel has it, as master's legacy rollback already accepts.
    - In every case the unsent position is dropped, and the CRTC carries **no cursor fields** until the next
      successful show, so a scene commit is never poisoned by cursor state.
    - A rejected scene commit that carried cursor fields gets the same treatment before the scene's retry.
- The intermediate-disable RANDR stall (~3.1 s in `disable_output`, present with every cursor mode) is out of scope.

## Review Focus

1. **A cursor-only commit in flight when the topology changes or the VT is released.** Its flip event must be
   drained by the existing old-event discard, never attributed to the next scene flip on a reused CRTC. Add cursor
   in-flight CRTCs to `pending_pageflip_crtcs` (`backend/kms/crtc_config.rs:42`) and to the VT path's equivalent
   (Task 5 test).
2. **A RANDR mode change passing through zero active CRTCs.** The plane-to-CRTC position properties are discovery
   data and must survive `CursorPlane::retain_crtcs`; the spike fell back to the software cursor here (Task 1 test).
3. **Motion while a direct-scanout successor or unflip is pending.** Nothing may be submitted to those CRTCs, and
   every CRTC's position must ride the next direct or composed transaction (Tasks 2–3 tests).
4. **The cursor shown again during direct scanout after a game hid it.** The existing path must bind the plane or
   unflip; the cursor must never be invisible while shown (Task 6 test).
5. **Cursor latency on amdgpu and i915**, where legacy moves were immediate. It is measured motion-to-visible, not
   only motion-to-accept: motion arriving just after a commit started waits for that commit's retirement and is
   visible a frame later (Task 1 instrumentation, Task 7 acceptance).

---

### Task 1: Atomic position properties, the path choice and a recordable commit sink

**Files:** `crates/yserver/src/kms/cursor_plane.rs`, new `crates/yserver/src/drm/atomic.rs` (+ `drm/mod.rs`),
`crates/yserver/src/kms/render/platform.rs` (where `KmsCursorState` and `KmsDevice` are defined),
`crates/yserver/src/kms/render/platform/cursor.rs`, `platform/init.rs`.

**Interfaces — produces:**
- `CursorPlane::atomic_position_properties(crtc) -> Option<(plane::Handle, property::Handle, property::Handle)>`
  and `supports_atomic_positions(&[crtc]) -> bool`. They are filled at discovery from every cursor plane with
  `CRTC_X` and `CRTC_Y`; **`retain_crtcs` never prunes them.**
- `crate::drm::atomic::CursorPlanePosition { device_key, crtc, plane, crtc_x, crtc_y, x, y }` (plus the motion
  timestamp), with
  `add_to(&self, req: &mut AtomicModeReq)`.
- **`AtomicCommitSink`,** held per KMS device:
  - Construction: a constructor parameter. In production it is the device's `atomic_commit`. A `#[cfg(test)]`
    recorder stores each request's object/property/value triples and flags. The recorder's result can be scripted
    per call (`Ok`, `EBUSY`, `EINVAL`) for Task 5's rejection tests.
  - It records each commit's kind (`cursor-only`, `scene-with-cursor`, `scene`), ioctl time and result.
  - Debug log: `atomic commit kind=… crtc=… ioctl_us=… result=…`.
  - Per-second info line under `YSERVER_LOOP_TELEMETRY`: per-kind counts, EBUSY, max/p99 ioctl µs, and two cursor
    latencies, both measured from the pointer event that produced a position (Review Focus 5):
    - **motion-to-accept:** to the accepted commit carrying it;
    - **motion-to-visible:** to the flip event retiring that commit. The pointer event's timestamp is carried with
      the position into the commit's bookkeeping.
  - On the legacy path, both latencies are measured to the `move_cursor` return, so the two paths are compared on the
    same scale.
- **`KmsCursorState` path choice:**
  - `atomic_cursor: bool` is decided per device from `supports_atomic_positions` over its active CRTCs. The decision
    is recomputed at every topology refresh.
  - `new_with_nvidia_policy` and the policy are removed.
  - A device whose CRTCs lack the properties keeps the legacy path, not the software cursor.

- [ ] Test `cursor_keeps_atomic_positions_across_a_zero_crtc_topology_step`: install, `retain_crtcs(∅)`,
  `retain_crtcs({crtc})`. Expect `supports_atomic_positions` true and `is_visible_on` false. Mutation: prune
  `atomic_positions` in `retain_crtcs`.
- [ ] Test: a device whose cursor plane lacks `CRTC_X` chooses the legacy path (not software, not atomic). Mutation:
  choose atomic unconditionally.
- [ ] Gate (below); commit `feat(kms): atomic cursor position properties and a recordable commit sink`.

### Task 2: The cursor rides every scene commit

**Files:** `crates/yserver/src/kms/render/platform/scene_sink.rs` (shared flip, ~L440),
`platform/scanout_bos.rs` (copied flip, ~L123), `backend/kms/scanout.rs` (direct ~L534, composed unflip ~L854),
`crates/yserver/src/drm/{page_flip.rs,modeset.rs,atomic.rs}`, `platform.rs`, `platform/{cursor.rs,init.rs}`.

**Interfaces:**
- `PlatformBackend::cursor_plane_atomic_positions_for_outputs(&[output_idx]) -> Vec<CursorPlanePosition>` returns one
  entry per listed output whose device uses the atomic path and whose plane is visible on that CRTC, at the latest
  position.
- The single-output submitters (shared and copied flip) take that list for their one output.
- The grouped submitters (`submit_direct_scanout`, `submit_composed_scanout`) take the list for every participating
  output.
- After a successful request, `cursor_plane_scene_commit_submitted(&[CursorPlanePosition])` clears the unsent
  position of exactly the (device, CRTC) pairs it carried. CRTC and plane handles are only unique within a DRM
  device, so every position names its `device_key`. A failed request clears nothing.

- [ ] Test (shared flip, through `tick_maybe_composite_for_tests` or the scripted scene path): the recorded request
  carries the cursor plane's `CRTC_X`/`CRTC_Y` = the latest position. Mutation: omit the append.
- [ ] Test (B-1): a **two-CRTC** direct transaction and a two-CRTC composed unflip with different output origins.
  Both planes' property triples are present with each CRTC's local coordinates. Mutation: append only the first
  output's position.
- [ ] Test: the copied flip carries the sink device's cursor plane position. Mutation: use the renderer device's
  plane.
- [ ] Test: a legacy-path device's scene commits carry no cursor properties. Mutation: append unconditionally.
- [ ] Gate; commit `feat(kms): the cursor position rides the scene's atomic commits`.

### Task 3: Cursor-only commits, retirement and the scene gate

**Files:** `platform.rs` (`KmsCursorState`), `platform/cursor.rs`, `platform/flip_events.rs`,
`backend/portable/pointer.rs`, `backend/kms/scanout.rs` (`scene_wants_compose`, `backend_scanout_next_wakeup`,
`backend_scanout_maybe_composite`), `platform/scene_sink.rs` (the `OutputSink` impl),
`crates/yserver-render/src/kms/render/scene/{sink.rs,tick.rs}`.

**Interfaces / invariants** (Tasks 4 and 5 build on these):
- **Per-CRTC state:** `atomic_pending_moves` (latest wins) and `atomic_cursor_commits_in_flight`.
- **Motion on an atomic-path device never submits directly (M-5).**
  - It stores the position (with its event timestamp) and marks the backend's scheduling pass as wanted.
  - The compose-wanted predicate that runs `maybe_composite` (`scene_wants_compose` in `backend/kms/scanout.rs`; `trait_impl.rs:166` only delegates) includes "an unsent cursor
    position eligible for a cursor-only commit exists". Eligible means not under `cursor_retry_waits_for_flip` and
    not in hide recovery.
  - **Cursor-only commits are decided only in that pass**, so motion cannot bypass the arbitration below.
  - **Direct scanout keeps master's own callbacks** (round 4 M-5). First-entry admission inside Present handling
    (`present.rs` ~L834–869) and `submit_queued_direct_successor` stay where they are and submit immediately as today.
    They check the same blockers: no cursor-only commit in flight on their CRTCs. No direct work is deferred to the
    pass, so no deferred first-entry state exists.
  - A cursor-only commit is marked in flight **only if its ioctl succeeded**.
- **Retirement:** a flip event on a CRTC with a cursor-only commit in flight retires that commit. With at most one
  commit in flight per CRTC, the CRTC identifies it.
- **Arbitration (the scheduling pass, the only place cursor-only commits are decided),** in this order, for each
  CRTC with nothing in flight:
  1. submits a waiting direct candidate whose CRTCs are all clear;
  2. otherwise lets the scene submit if it has a frame ready for that output;
  3. only then sends the unsent position as a cursor-only commit.
- **Scene gate:** new `OutputSink::output_commit_in_flight(output_idx) -> bool` (`false` in `TestSink` unless set).
  The scene's per-output submit gate (`tick_one_output`) and `has_output_ready_for_submit` treat it like a non-empty
  `pending_acks`. The composed unflip also waits while a cursor-only commit is in flight on its outputs.

- [ ] Test `idle_motion_coalesces_until_cursor_flip_retires` (via `Backend::warp_pointer_root` and
  `on_page_flip_ready`): one cursor-only commit, nothing until retirement, then one commit with the latest position.
  Mutation: first-wins insertion.
- [ ] Test: motion during a pending scene flip sends nothing; the next scene request carries the latest position.
  Mutation: ignore the scene blocker.
- [ ] Test: the scene does not submit while a cursor-only commit is in flight, and submits after its retirement.
  Mutation: `output_commit_in_flight` always false.
- [ ] Test (M-5): scene damage is pending behind a cursor-only commit. Then come the retirement, then more motion,
  then the scheduling pass. The next commit is the scene's, carrying the latest position, and no cursor-only commit
  goes out in between. Mutation: let motion submit a cursor-only commit immediately.
- [ ] Test `legacy_path_pointer_motion_keeps_one_legacy_move_and_no_atomic_commit`. Mutation: route the legacy device
  through the atomic branch.
- [ ] Gate; commit `feat(kms): cursor-only atomic commits, one commit in flight per CRTC`.

### Task 4: Direct scanout and the cursor commit stream

**Files:** `backend/kms/{present.rs,scanout.rs}`, `platform/cursor.rs`.

**Interfaces / invariants:**
- **Direct Present behind a cursor commit (M-1, B-2).** It depends on whether a direct frame already owns scanout.
  - **First entry** (no direct frame `current` or `pending`): a cursor-only commit in flight on any of its outputs is
    treated exactly like master's "scene flip pending" case (`present.rs` ~L839). The Present is not taken direct
    this time (`Ok(false)`, pins released, composed path as today), and the next Present retries. No queued-only
    direct state is ever created, so no cancellation owner is needed.
  - **Successor** (a direct frame is `current`, fully retired): the Present goes into master's successor slot
    (latest-wins, the same pin/Skip rules). The cursor-only retirement that clears its last blocking CRTC calls the
    `submit_queued_direct_successor` path. If an unflip was requested meanwhile, master's composed replacement — which
    exists because `current` is set — Skips it as today.
- Direct first-entry admission, `submit_queued_direct_successor` and the composed unflip check the Task 3 blocker
  (no cursor-only commit in flight on their CRTCs). Step 1 of Task 3's arbitration resumes a retained successor.

- [ ] Test (Review Focus 3): motion while a direct successor is pending sends nothing; the successor carries it.
  Mutation: drop the direct blocker.
- [ ] Test (M-1, successor behind a cursor commit):
  - a direct frame is current, a cursor-only commit is in flight, then an eligible Present arrives;
  - it is retained, not submitted;
  - the cursor retirement submits it, its completion arrives, and its pins retire;
  - no further cursor-only commit goes out before it.
  Mutation: retirement does not resume the waiting successor.
- [ ] Test (B-2, first entry behind a cursor commit):
  - no direct frame exists and a cursor-only commit is in flight;
  - an eligible Present returns `Ok(false)`: no queued successor, pins released, composed completion delivered;
  - after the retirement, the next eligible Present enters direct;
  - a root overlay appearing in between leaves nothing retained.
  Mutation: queue the first-entry Present in the successor slot.
- [ ] Gate; commit `feat(kms): direct scanout waits for and resumes after cursor-only commits`.

### Task 5: Rejections, topology changes and VT release

**Files:** `platform.rs` (`KmsCursorState`), `platform/cursor.rs`, `platform/flip_events.rs`,
`backend/kms/{scanout.rs,crtc_config.rs,session.rs}`.

**Interfaces / invariants:** the **Rejections** rules in Global Constraints (EBUSY → `cursor_retry_waits_for_flip`;
other errors → legacy hide, then software fallback or motion-driven hide recovery), and:
- **Old events:** `pending_pageflip_crtcs` and the VT-release path's set include CRTCs with a cursor-only commit in
  flight, so the old-event discard drains them (Review Focus 1). `cursor_plane_hide_all` and topology changes clear
  the per-CRTC state only after that discard.

- [ ] Test (M-2): the recorder rejects a cursor-only commit with EBUSY. Expect:
  - no in-flight state and the position still unsent;
  - one EBUSY counted;
  - the next scene frame submits without any synthetic flip event and carries the position;
  - no second cursor-only attempt in the same pass.
  Mutation: mark in flight before the ioctl result.
- [ ] Test (M-6): after a cursor-only EBUSY on an idle desktop, three successive scheduling passes without new input
  attempt no cursor-only commit, and the compose-wanted predicate is false. New motion does not change that. A flip
  event on the CRTC re-enables exactly one cursor-only commit. Mutation: do not mark `cursor_retry_waits_for_flip`.
- [ ] Test (M-3): the recorder rejects a cursor-only commit with EINVAL.
  - The plane is hidden through the legacy ioctl and the CRTC goes to software fallback.
  - The next scene request carries no cursor fields and is accepted.
  - With the hide scripted to fail, hardware ownership is retained and scene requests still carry no cursor fields.
  - **Hide recovery (idle desktop, no other damage):**
    - Motion retries the hide; a failed first retry is followed by another on the next motion.
    - When the hide is scripted to succeed, the CRTC moves to software fallback and the motion is visible through the
      scene.
    - When it fails three times, the device is `permanently_disabled` and no fourth hide is attempted.
    - No retry depends on a flip event.
  Mutation: keep the unsent position after EINVAL; separately, never retry the hide.
- [ ] Test (Review Focus 1): a cursor-only commit in flight at a RANDR change. Its event is discarded, and the first
  scene flip after the change retires the scene's own frame. Mutation: leave cursor CRTCs out of
  `pending_pageflip_crtcs`.
- [ ] Gate; commit `feat(kms): cursor commit rejections and topology changes keep the stream consistent`.

### Task 6: A hidden cursor does not block direct scanout

**Files:**
- `crates/yserver-render/src/kms/render/scene/cursor.rs`: `classify_cursor_mode_from_per_output` (L28) and
  `cursor_mode` (L114);
- the `CursorPlaneMode` enum (`yserver-render/src/kms/render/scene.rs:275`);
- `crates/yserver/src/kms/render/backend/kms/scanout.rs`: the `scanout_m1_probe_eligible` calls (~L1013, and the
  diagnostic one ~L1465) and the composite-tick unflip (~L2034);
- `backend/portable/pointer.rs`: the mode match.

**Cause:** XFIXES `HideCursor` makes every output `OutputCursorMode::Hidden`. The classifier maps "no Hw and no Sw"
to `CursorPlaneMode::Sw`, and both direct gates require `Hw`. So a hidden cursor, which needs no composition, blocks
direct scanout on every GPU; the unflip reason is literally `composite_tick_software_or_hidden_cursor`.

**Invariant:** a cursor that is hidden on every output is direct-compatible.
- Add `CursorPlaneMode::Hidden`: every output `Hidden`, no pending transition.
- Both direct gates accept `Hw | Hidden`.
- The pointer fast path treats `Hidden` as "nothing to move".
- `Mixed` stays as today.

- [ ] Test (render crate, `TestSink`): all outputs hidden → `cursor_mode() == Hidden`; Hidden + Sw → `Sw`;
  Hidden + Hw → `Hw`. Mutation: map all-Hidden to `Sw`.
- [ ] Test (backend): after `HideCursor`, an eligible fullscreen Present enters direct scanout and the composite tick
  does not request an unflip. Mutation: gate on `Hw` only.
- [ ] Test (Review Focus 4): `ShowCursor` during direct scanout binds the plane (Hw) or unflips (Sw), and never leaves
  the cursor invisible. Mutation: skip the bind on show.
- [ ] Gate; commit `fix(kms): a hidden cursor does not keep direct scanout off`.

### Task 7: Hardware validation (with the user's approval)

Run on **card1 (NVIDIA)**, then on **card0 (AMD)** after the user moves the monitor cable. Use
`~/Projects/claude-scratch/yserver-pr1/hw_spike_matrix.sh` with master (on NVIDIA: the software cursor; on AMD:
master's legacy hardware cursor), master with the NVIDIA policy lifted (NVIDIA only), and this branch. On each card
also run:
- fullscreen mpv with the cursor hidden (an XFIXES `HideCursor` client);
- a RANDR mode change while moving;
- a VT switch, done by a person from a tty3 login.

Stop at the first Xid or `ERR!`. **i915:** ask the maintainer to run the matrix on an Intel machine. A run without it
is reported as "i915 not validated".

**Acceptance:**
- **NVIDIA:** each matrix row equals the software cursor (0 round trips > 8 ms, drag p99 within 1 px, 60 fps,
  0 drops).
- **AMD:** each row equals or beats master's legacy cursor.
  - Report motion-to-accept and motion-to-visible (p50/p99/max) for master's legacy cursor and this branch, with
    sustained motion and with single motions after idle.
  - Acceptance: motion-to-visible p99 ≤ two frames (33.4 ms at 60 Hz).
  - The table is reported to the user and the maintainer as the price of one model for every driver.
- **Both:**
  - direct submits > 0 in fullscreen, with the cursor visible and with it hidden;
  - EBUSY 0;
  - NONBLOCK ioctl p99 < 1 ms;
  - the cursor stays hardware after the RANDR change;
  - the VT switch restores the cursor.

- **Shared and copied flip callers** (no hardware-free fixture reaches them; Task 2 tests them at helper level):
  shared flips with a visible cursor are recorded as `scene-with-cursor` in the per-second line. On a copied output
  this is checked when one is available.

Record the tables in this plan and in `docs/status.md`.

### Task 7 results — NVIDIA (card1, 2026-10-13, branch d0d61759, master 7e325a61; Xid 0 throughout)

| test | master, software cursor | master, legacy HW cursor (policy lifted) | branch |
|---|---|---|---|
| pure motion 250 Hz 20 s: round trips > 8 ms / max | 0 / 1.9 ms | 1200 / 17.1 ms | **0 / 1.9 ms** |
| window drag: lag p99 px / query p99 ms | 12 / 0.39–0.40 | 13–19 / 17.2–31.1 | **12 / 0.36–0.41** |
| windowed mpv + motion: > 8 ms; frames/s; mpv drops | 0; 60; 0 | 900; 60; 0 | **0; 60; 0** |
| fullscreen mpv + motion: > 8 ms; live direct submits; unflips | 0; 0 (software cursor blocks direct); 0 | 896; 2053; 2 | **0; 2066; 2** |
| cursor hidden (XFIXES) **before** fullscreen mpv: direct submits in the first 14 s | – | **0** (the Task 6 bug) | **863** |
| cursor hidden after direct entry: direct submits while hidden | 0 | 515 | 515 |

- EBUSY 0 on every branch run. NONBLOCK ioctl p99: scene-with-cursor 60–360 µs, cursor-only 25–80 µs.
- RANDR mode change while moving: the cursor stays hardware (`topology_blocked` never set). The ~3.1 s stall in
  `disable_output` appears with **every** binary, the software cursor included (pre-existing, out of scope).
- MATE session (`hw_cursor_mate_dpms.sh`): 5 DPMS off/on cycles while moving, then a screensaver lock. No old-event
  drain timeout, no EBUSY, no panic, clean exit, and the desktop is back after every cycle. 2 of 5 DPMS-offs show the
  same pre-existing ~3.1 s `disable_output` stall.
- **Open:**
  - The latency fields of the per-second line were non-zero in only a few seconds of each motion phase
    (motion-to-visible ~20 ms when present). The instrumentation must be checked before the AMD run, whose acceptance
    depends on it.
  - VT switch: not run yet. It needs the one-time root setup (`claude-scratch/yserver-pr1/vt-auto/`).
  - AMD (card0) and i915: not run yet.

### Task 7 results — AMD (card0 = Ryzen integrated GPU, amdgpu, HDMI-A-1 1920x1080@60; 2026-10-13, branch 15b02cc3 vs master 7e325a61)

Run with `tools/cursor-bench`. Rendering is on the same device (`relationship: Same`). No new amdgpu kernel errors
(19 before, 19 after).

| test | master (legacy cursor ioctls) | branch |
|---|---|---|
| pure motion 250 Hz 20 s: round trips > 8 ms / max | 0 / 0.41 ms | **0 / 0.37 ms** |
| window drag: lag p99 px / query p99 ms | 12 / 0.24–0.32 | **12 / 0.26–0.31** |
| windowed mpv + motion: > 8 ms; mpv drops | 0; 0 | **0; 0** |
| fullscreen mpv + motion: > 8 ms; mpv drops | 0; 0 | **0; 0** |
| branch cursor latency (motion seconds), p99 per second: median / max | – | accept 3.8–4.0 / 24.6 ms; **visible 20.3–20.6 / 30.0 ms** (RANDR excluded) |
| EBUSY | 0 | 0 |

- **Acceptance met:** every row equals master, and motion-to-visible p99 stays under two frames (33.4 ms).
- **Direct scanout does not engage on this iGPU with either binary**, so the hidden-cursor row cannot be shown here.
  The cursor gate is open (`m1_gates[... cursor=Hw|Hidden ...]`, `m1_gate_reject[cursor=0]`). Direct entry is blocked
  by the source buffer instead:
  - `reject_regions` for every Present;
  - `scanout M1 add_fb2 rejected linear buffer with explicit modifier (EINVAL) and legacy metadata (EINVAL)` for
    mpv's buffers.

  This is pre-existing and out of scope.
- i915: not run (no Intel machine). VT switch: not run.

## Gate (every task)

1. `cargo +nightly fmt`, then `cargo +nightly fmt -- --check`.
2. Clippy `--workspace --all-targets -D warnings`: default, `--features yserver/tcp-transport`,
   `--features yserver/xdmcp`.
3. `cargo test --workspace --locked`, and each file under `crates/yserver/tests/` by name.
4. The lavapipe ignored suite in the form master's CI uses (`.github/workflows`), with
   `VK_DRIVER_FILES=~/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1`.
   `two_gpu`/`_drm` tests are skipped without the user's approval.
5. Rule F for the task's new tests.
6. Cargo.lock unchanged. FreeBSD: "not checked" (CI runs it).
7. The source layout rules master's CI checks since #243 (`tools/check-layout.sh`): file-size ceilings, the
   `backend/portable` seam, `yserver-render`'s dependencies. Until the branch is rebased past `a343264b`, run the
   script's body from the worktree root.

## Review log

- **Task 3 stop (2026-10-13):** the compose-wanted predicate and the scene gate live in `backend/kms/scanout.rs`,
  and the `OutputSink` impl in `platform/scene_sink.rs`; both were added to Task 3. File lists name the main files.
  A task may also change any file that owns or calls the interfaces it names, and the report lists them.

- **Task 2 (2026-10-13):** the direct and unflip caller tests go through `Backend` entries with the recording
  sink. The shared and copied flip callers have no hardware-free scene fixture (`for_tests_with_vk_live_scene`
  needs a KMS device), so they are tested at helper level, and Task 7 checks them on hardware.

- **Task 2 stop (2026-10-13):** a position without a device identity cannot be cleared unambiguously when two
  devices reuse CRTC handles; `CursorPlanePosition` gains `device_key`, and Task 2 may touch `drm/atomic.rs`,
  `platform.rs`, `platform/{cursor.rs,init.rs}`.

- **Task 1 stop (2026-10-13):** `KmsCursorState` and `KmsDevice` live in `kms/render/platform.rs`, which no
  file list named; added to Tasks 1, 3 and 5.

Task numbers in rounds 1–4 refer to the plan as it was then. After round 4, Task 3 was split into Tasks 3–5
(cursor stream, direct scanout, rejections/topology); the hidden cursor became Task 6 and hardware Task 7.


- **Round 1 (codex `gpt-6.1-sol` xhigh, instrument `111490f`, 2026-10-13), on the NVIDIA-only revision:** 1 blocking,
  2 major, 0 minor. All three were verified against the code and applied:
  - **B-1:** grouped direct and unflip transactions carry every participating CRTC's position (Task 2).
  - **M-1:** a direct Present waiting behind a cursor-only commit is retained in the successor slot and resumed by the
    cursor retirement (Task 3).
  - **M-2:** in flight means accepted; the rejection test (Task 3).
- **Round 2 (same instrument, 2026-10-13):** 1 blocking, 2 major, 0 minor; B-1 and M-2 confirmed applied, M-1
  traded. All three were verified against the code and applied:
  - **B-2:** the first direct entry behind a cursor commit follows master's "scene flip pending" branch instead of
    queueing, so no queued-only direct state exists (Task 3).
  - **M-3:** non-EBUSY rejections hide the plane through the legacy rollback and stop carrying cursor fields (Global
    Constraints, Task 3 test).
  - **M-4:** cursor latency is measured motion-to-visible as well as motion-to-accept, and the claim is corrected to
    up to two frames (Task 1, Task 5).
- **Round 3 (same instrument, 2026-10-13):** 0 blocking, 2 major, 0 minor; B-1, M-1, M-2, B-2 and M-4 confirmed
  applied, M-3 partial. Both findings were verified against the code and applied:
  - **M-3:** hide recovery has an owner (the outcome handoff wakes the scene and unflips), a bounded retry (3 flip
    retirements) and a final state (`permanently_disabled`) (Global Constraints, Task 3 test).
  - **M-5:** motion only records a position; every commit on a CRTC is decided in the scheduling pass, so a ready
    scene frame cannot be starved (Task 3 test).
- **Round 4 (same instrument, 2026-10-13):** 0 blocking, 3 major, 0 minor; B-1, M-1, M-2, B-2 and M-4 confirmed
  applied, M-3 and M-5 partial. All three were verified against the code and applied:
  - **M-3:** hide recovery is driven by pointer motion, not by flips that may never come on an idle desktop.
  - **M-5:** the single-pass rule is narrowed to cursor-only commits. Direct admission and successor submission keep
    master's callbacks and check the cursor blocker, so no deferred direct state exists.
  - **M-6:** after EBUSY a CRTC waits for its next flip event before another cursor-only attempt, and the
    compose-wanted predicate ignores ineligible positions.
- **Scope change (user, 2026-10-13):** Tasks 1–3 apply to every driver with atomic cursor position properties (AMD
  and i915 too), chosen by capability. Hardware validation on AMD was added.

## Non-goals

The KMS owner thread; the RANDR intermediate-disable stall; tearing / async page flips; atomic show/hide or cursor
image changes; the software clock.
