# Tearing on Demand (Async Page Flips) — Design

Date: 2026-10-14. Status: **Approved by the user 2026-10-14** (reviews r1 1B/2M/1m incorporated, r2 0 findings). Plan waits for the maintainer's answer on the HLD change.

Builds on: `2026-08-05-present-direct-scanout-design.md` (direct scanout, M2),
`2026-08-11-async-present-defer-supersession.md` (async Present target identity),
the wlroots-model atomic cursor (#244, `e118bc3e`) and its follow-ups (#246, `a1fa1df6`).

## Goal

Code name **C1**. Uncapped direct scanout: a fullscreen client that asks for tearing gets its frames flipped with
`DRM_MODE_PAGE_FLIP_ASYNC`, so the direct-scanout flip rate is no longer capped by vblank. Nothing else changes.

- With async flips the rate is bounded by the flip's completion turnaround (one commit in flight per CRTC), not by
  the refresh. Success is the async flip rate following the client's frame rate well above the refresh rate; the
  turnaround is measured per driver in the hardware validation.
- The single-successor latest-wins slot and `Skip` completions stay as they are. Frames are skipped only when the
  client outpaces the flip turnaround.

## Decisions (approved)

1. **Protocol semantics = Xorg modesetting ("model B").** yserver advertises
   `PresentCapabilityAsync` and **not** `PresentCapabilityAsyncMayTear`. Per
   `presentproto.txt:337-339`, without `AsyncMayTear`, `PresentOptionAsync` means
   "may tear". Every Present carrying `PresentOptionAsync` may tear. Rationale (user):
   do not deny tearing to clients that asked for it.
   - What Mesa sends today (main, `wsi_common_x11.c:1785-1792`,
     `loader_dri3_helper.c:1154`): Vulkan `IMMEDIATE` → `Async` (plus `AsyncMayTear`
     only if advertised); `FIFO_RELAXED` → `Async`; `MAILBOX` → nothing on a non-Xwayland
     server (Mesa detects Xwayland by a RANDR output named `XWAYLAND*`,
     `wsi_common_x11.c:230`); GL swap interval ≤ 0 → `Async`. With model B all of these
     tear, which is what each mode asks for on a non-Xwayland server.
   - NVIDIA's closed client libraries, **measured 2026-10-14** (driver 615.71.09,
     RTX 5060 Ti; throwaway yserver build logging raw `PresentPixmap` options under
     three advertised capability sets; mpv gpu-next as client). Every Present in a
     run carried the same value:

     | Client / mode | no Async cap | cap `Async` | cap `Async`+`AsyncMayTear` |
     |---|---|---|---|
     | NVIDIA Vulkan `IMMEDIATE` | `0x8` | `0x9` | `0x9` |
     | NVIDIA Vulkan `MAILBOX`, `FIFO`, `FIFO_RELAXED` | `0x8` | `0x8` | `0x8` |
     | NVIDIA GL (EGL) swap interval 0 | `0x8` | `0x9` | `0x9` |
     | NVIDIA GL swap interval 1 | `0x8` | `0x8` | `0x8` |
     | control: Mesa radv `IMMEDIATE` | — | — | `0x19` |
     | control: Mesa radv `MAILBOX` | — | — | `0x8` |

     (`0x1` Async, `0x8` Suboptimal, `0x10` AsyncMayTear.)
   - NVIDIA never sends `AsyncMayTear`, even when it is advertised. It sends `Async`
     only for unsynchronized modes and only once `PresentCapabilityAsync` is
     advertised. The Xwayland semantics (tear only on `AsyncMayTear`) would therefore
     give **no tearing at all on NVIDIA**. That is why model B was kept over model A
     after the user first favoured A.
   - Neither NVIDIA nor Mesa (outside Xwayland) uses `Async` for `MAILBOX`, so model B
     does not turn a mailbox request into tearing on these stacks.
2. **Eligibility = direct-scanout eligible ignoring the cursor ("A").** Tearing
   applies only when the client is fullscreen and would be scanned out directly
   except for the cursor. The normal desktop and non-fullscreen windows never tear.
3. **Cursor = KWin model.** While tearing, the hardware cursor plane is not used on
   any driver; a visible cursor is drawn into the frame
   (KWin `drm_output.cpp:112-116`, `:307-324`). Rejected alternatives:
   - sway model (test async, fall back to vsync when the cursor moved): a visible,
     moving cursor turns tearing off almost all the time.
   - legacy cursor ioctl alongside async flips on amdgpu/i915: mixes two APIs on one
     plane against #244's one-commit-stream-per-CRTC model, the amdgpu async cursor
     path falls back to a blocking commit when the previous cursor commit has not
     finished (`drm_atomic_helper_async_check`), and on NVIDIA the legacy ioctl is the
     blocking commit #244 removed.
4. **Async rejection handling = submit directly, fall back on `EINVAL`.** No
   per-frame `TEST_ONLY` (estimated 50–150 µs per frame on NVIDIA, on the single
   server thread, ahead of every commit).

## Relation to the high-level design

`docs/high-level-design.md` lists "tear-free presentation by default" as a goal
(:13-14, :213). This design keeps tear-free as the default: nothing tears unless a
client asks for unsynchronized presentation (`PresentOptionAsync`: Vulkan
`IMMEDIATE`, GL swap interval 0), and then only while that client is fullscreen and
direct-scanout eligible. Model B does not distinguish whether the user or the
application chose unsynchronized presentation. The same change set updates the HLD
wording to say so, and corrects the stale statement that fullscreen direct scanout
is not implemented (it is, since the M2 direct-scanout work).

## Kernel rules this design relies on

From `drm_atomic_uapi.c` (Linux 7.2) and the drivers:

- `DRM_MODE_PAGE_FLIP_ASYNC` on an atomic commit needs
  `mode_config.async_page_flip`; userspace reads it as `DRM_CAP_ATOMIC_ASYNC_PAGE_FLIP`.
- In an async commit only `FB_ID`, `IN_FENCE_FD` and `FB_DAMAGE_CLIPS` of the
  **primary** plane may change. Other properties are accepted only with their current
  value (no-op).
- `OUT_FENCE_PTR` is a CRTC property whose current value reads as 0, so any pointer
  is a change: an async request must not carry it.
- For a non-primary plane, the core consults the driver's `atomic_async_check` only
  when a permitted property actually changes (`drm_atomic_uapi.c:1269-1285`). No
  desktop driver accepts the cursor plane in an async flip (amdgpu rejects it,
  `amdgpu_dm_plane.c:1283-1285`; i915 and nvidia-drm have no hook). An unchanged
  cursor property passes the core check but still pulls the plane into the state,
  where i915's own async check rejects planes without async support. **The async
  request carries no cursor-plane property at all.**
- Driver rejections seen in source:
  - i915: stride, modifier, format, rotation, size/position or alpha change; formats
    or modifiers without async support; joiner.
  - amdgpu: any update that is not a "fast update" (pitch, DCC, memory type).

## Design

### 1. Tearing state and transitions

**Condition, evaluated per Present on the direct-scanout path.** All three must hold:

- the Present's `options` contain `PresentOptionAsync`;
- the candidate passes the direct-scanout gate **with the cursor term ignored**: the
  existing `scanout_direct_eligible` inputs (`scanout.rs:38-56`), shape chain, CRTC
  eligibility, untransformed outputs and probation, with `cursor_hw` treated as true;
- every CRTC the frame would flip belongs to a device with
  `DRM_CAP_ATOMIC_ASYNC_PAGE_FLIP`.

The backend keeps a **tearing state**: `Off`, or `On` for one client window
(`dst_window_xid` + CRTC epoch). It enters `On` on the first Present that meets the
condition.

**A fully transparent cursor counts as hidden** (user decision after the NVIDIA run, 2026-10-14). Many clients hide
the pointer by defining an empty cursor image instead of XFIXES `HideCursor` (mpv's autohide, SDL's hidden cursor).
Treating it as visible would force the KWin model to compose every frame and never reach direct scanout, defeating
C1. A cursor whose image has alpha 0 on every pixel is classified when the cursor is created (once per cursor
image) and behaves like XFIXES-hidden everywhere the cursor's visibility is decided: the scene assigns
`CursorAssignment::Hidden`, `tearing_cursor_wants_visible` is false, and direct scanout is eligible. Outside tearing,
the same classification applies; nothing is drawn either way, so it only removes useless plane or software-cursor
work.

**While `On`:**

- **Cursor hidden** (`CursorPlaneMode::Hidden`): the frame goes through direct
  scanout as today, flipped async. The Hidden state already keeps the cursor out of
  the commit. Any producer of Hidden qualifies: XFIXES-hidden or off every output.
  The direct gate already accepts both.
- **Cursor visible:** the backend passes `hw_strategy_enabled = false` to the scene
  tick (`scene/tick.rs:561`).
  - The scene assigns `CursorAssignment::Sw` (`scene/build.rs:555-587`) through its
    existing HW→SW transition, and the client leaves direct scanout because the cursor
    term now fails.
  - The composed scene contains the fullscreen client plus the software cursor, and
    its flips go async.
  - Hiding the plane is not allowed in an async commit, so the transition frame is
    a normal vsync commit, once. KWin does the same.

**Entry handoff with a visible cursor (review r1 B-1).** Setting
`hw_strategy_enabled = false` is not enough on its own:
- While direct scanout is held with a `Hw` cursor, `maybe_composite` returns at
  `hold_direct` before the scene tick (`scanout.rs:2190-2212`). The scene never
  ticks, so it never assigns `Sw`.
- Entering `On` with a visible cursor while direct is held therefore runs an
  explicit handoff:
  1. Request the grouped unflip (new reason `tearing_cursor_handoff`).
  2. Materialize the retained direct source into the COW and keep direct ownership
     until the replacement retires (direct-scanout spec D:184-190, 222-231).
  3. Let the scene's HW→SW transition hide the plane. The hide runs at retirement
     and can fail (`scene/cursor.rs:529-533`).
  4. Composed async flips start only after the plane is **confirmed hidden**, on the
     frame after a successful hide (as in `a6f8909c`). Until then the composed
     frames go vsync.
- If the hide fails, `On` stays without tearing for visible-cursor frames: vsync,
  logged once. The existing hide-failure fallback handles the plane.
- While `On`, nothing may rebind the hardware cursor plane: XFIXES `ShowCursor`
  (`cursor.rs:484-492`), `refresh_direct_cursor_on_all_outputs` and
  `bind_direct_cursor_on_all_outputs` all respect the tearing state. A hidden→visible
  change while `On` with direct async flips takes the same handoff as entry.

**Probation (review r1 M-1).** The existing admission counter
(`admit_eligible_root`, 8 consecutive eligible root Presents, M2 plan:117-120)
advances only when the real direct gate passes, and an ineligible authoritative-root
Present resets it and requests an unflip (`present.rs:775-783`). With a software
cursor that would happen on every Present. Therefore:
- Tearing has its own probation: 8 consecutive authoritative-root Presents from the
  same window that carry `Async` and pass the gate with the cursor term ignored.
  It is reset by an authoritative-root Present that fails the gate with the cursor
  term ignored, or that lacks `Async`. It applies to entry only.
- While `On`, the "ineligible authoritative-root successor" decision in
  `try_present_direct` uses the gate with the cursor term ignored. A Present that is
  ineligible only because of the cursor neither resets direct probation nor requests
  an unflip; it takes the Copy path into the composed scene.
- Direct re-entry inside `On` (cursor becomes hidden) still needs the real direct
  gate, including its own probation, which keeps its original purpose of preventing
  direct/composed thrash on alternating full/region-limited Presents.

**Leaving `On` → `Off`:** on any of these:
- a Present from that window without `PresentOptionAsync`;
- the window no longer passes the gate (cursor term ignored);
- topology change, DPMS off, VT release;
- the window is destroyed or unmapped.

Then `hw_strategy_enabled` returns to its normal value, and the cursor policy
returns to whatever the exit state calls for (review r1 m-1):
- HW when the cursor is visible and allowed (the scene's SW→HW transition re-enables
  the plane with a vsync commit);
- SW when any output is transformed (`hw_cursor_allowed`, `scene/cursor.rs:545-549`);
- Hidden when XFIXES hides it;
- untouched while KMS submission is suspended (DPMS off, VT away). The normal
  resume path applies the policy.

The next eligible Present may re-enter direct scanout with vsync.

**Composed frames are not vblank-paced while `On`.**
- A new client frame is composed and submitted as soon as its damage arrives and no
  commit is in flight on that CRTC, without waiting for a vblank-aligned tick.
  Otherwise the async flip would only ever show at vblank.
- The plan must locate the scene's tick pacing and state which wake triggers the
  compose.

**Cursor-only recomposition cap.** While `On` with a visible cursor:
- Pointer motion without a new client frame recomposes at most once per refresh
  interval, measured from the previous cursor-only recomposition.
- A new client frame is never delayed by this cap and carries the current cursor
  position.

**Unchanged:**
- no `Async` → today's behaviour;
- non-fullscreen windows and the desktop: composed with vsync;
- the scene's own flips outside `On`: vsync.

### 1b. Core supersession parity with Xorg (amendment B, user-approved 2026-10-15)

Hardware runs showed that the core's supersession (`supersede_covered_pending_presents`,
yserver-core `present_ext.rs`) scraps a pending Present that is already due and only waiting for its source /
acquire fence whenever a same-target successor arrives. Explicit-sync Vulkan clients (NVIDIA's WSI, radv, DXVK,
VKD3D, CS2) send `PresentPixmapSynced` before the GPU finishes the frame. A GPU-bound client therefore has almost
every frame scrapped before its fence signals: CS2 on NVIDIA went from 311 direct flips/s to 7–50/s once the scene
loaded the GPU (330–360 Presents/s, 300–340 scrapped/s); mpv on the AMD iGPU showed 7 flips/s for 515 Presents/s.

Xorg does not hit this because an explicit-sync **flip** never waits in the server: `present_execute_wait`
(`present_execute.c:61-95`) waits for the acquire syncobj only for copies (`!vblank->flip`), so the flip
reaches the kernel at once and the driver waits for the fence. A due Xorg copy whose fence has not
signalled stays `queued` (the wait returns before `present_execute` clears it at `present_scmd.c:576`), so
Xorg's supersession (`present_scmd.c:803-823`) can scrap it. yserver waits for every acquire fence in the
core, flips included, so it needs the following rule to get Xorg's flip behaviour. It applies to every
Present, not only while tearing:

- a covered same-window, same-CRTC, same-target Present is scrapped when it would be parked for a future
  MSC, whether or not its source/acquire fence has signalled (its armed wait is released);
- a due Present (one that would execute now if its fence were ready) that waits for its source/acquire
  fence is never scrapped by a successor; it executes when its fence signals, in arrival order;
- once executed, the backend's single latest-wins successor slot still replaces an older ready frame while a flip
  is in flight, so a frame that is already stale is dropped there.
- order is preserved: a fence-waiting Present whose fence signals after a newer same-window, same-target Present
  has already executed completes as Skip (it would otherwise show an older frame after a newer one).

This amends the 2026-08-11 async-supersession spec (its sentence "requests still waiting for their acquire/source
fence remain in core and can be scrapped there") for due Presents. Due copies waiting for their fence are
kept where Xorg would scrap them; this is the one deliberate difference, because yserver cannot tell at that
point whether the frame will be flipped or copied. Memory is bounded by the client's
swapchain images, since each pending Present holds one of the client's buffers. Vsync Presents remain paced by vblank.

### 2. Building and submitting the commit

`submit_direct_scanout` (`modeset.rs:1570`) and `submit_scene_flip_with_fences`
(`page_flip.rs:183`) take a flip timing: `Vsync` (today) or `Async`.

**`Async` request:**
- flags `NONBLOCK | PAGE_FLIP_EVENT | PAGE_FLIP_ASYNC`;
- no `OUT_FENCE_PTR`. The scanout BO takes the no-fence path
  (`bo_state.rs:176`, `-1`) and the previous BO is released by the flip event;
- no cursor positions and no cursor-plane property;
- every other primary-plane property as today (no-op when unchanged).

The atomic commit sink records a new `AtomicCommitKind::Async`. The cursor-blame
attribution of #246 never runs for it, because it carries no cursor fields.

**`EINVAL` on an async submit:**
1. Resubmit **the same frame** as `Vsync`. If that also fails, it is today's error
   path.
2. The next frame tries `Async` again. Transitions make one frame fail, for example
   the first direct frame changing `SRC_*` and `CRTC_*` against the composed frame, or
   switching between composed and direct when the cursor shows or hides. After the
   vsync resubmit the state matches and async passes.
3. Two consecutive async `EINVAL`s with the same **shape** suspend async for that
   shape, logged once. The shape is framebuffer format, modifier, stride and size,
   the set of CRTCs, and direct vs composed. This handles permanent driver limits
   without failing every frame. A shape change clears the suspension.

**`EBUSY`:** unchanged. There is at most one commit in flight per CRTC and the latest
successor is retained. An async flip completes quickly, so the successor goes out on
its event.

**Present completion (review r1 M-2):**
- Direct frames complete as `Flip`, with MSC and UST from the async flip's event.
  The MSC is the current vblank, which is what Xorg reports.
- Composed frames keep today's `Copy` completion **unchanged**: readiness follows the
  GPU copy (`present.rs:154-181`), not a scene flip event. No Copy-to-scene-event
  association is introduced.
- Per-window ordering across Copy, Skip, Flip and unflip is preserved (D:191).

**BO retirement without an out fence.** With `out_fence_fd = -1` the BO's
`release_fence_fd` is `None` (`bo_state.rs:176-181`). Retirement then rests on the
flip event alone. The plan must show that `Retiring → Free` with no release fence
still waits until the replacement flip has retired and every GPU reader is done,
including the copied-scanout and lifecycle-reset paths, before the BO is reused.

### 3. Protocol advertisement

`PresentCaps` (`trait_def.rs:173-215`) advertises `PresentCapabilityAsync` when the
window's output device has `DRM_CAP_ATOMIC_ASYNC_PAGE_FLIP`. This is decoupled from
`flip_path`, which stays as it is. `async_may_tear` stays `false`, so
`PresentOptionAsyncMayTear` keeps being silently cleared (`present_ext.rs:1670`).

When the device lacks the capability, `Async` keeps today's mailbox behaviour
(`2026-08-11` spec).

### 4. Telemetry

Per second:
- async flips;
- vsync flips while `On`;
- async `EINVAL`s by transition kind.

Once per shape: the suspension log line. Nothing else.

## Testing

All tests drive real `Backend` entries with the recording atomic sink and the
injectable flip-event source (rules A/B/C/F; GPU-waiting tests run 20–30 times
under CPU load). Each test names the mutation that must make it fail:

- Async request: carries `PAGE_FLIP_ASYNC`, no `OUT_FENCE_PTR`, no cursor-plane
  property. Mutation: keep `OUT_FENCE_PTR`, or keep cursor positions.
- Entry needs all three conditions; `Async` on a non-fullscreen window or on a device
  without the capability stays vsync. Mutation: drop any one condition.
- Cursor hidden while `On`: direct async flips, no scene compose.
- Cursor visible while `On`:
  - `hw_strategy_enabled = false` reaches the scene;
  - one vsync transition commit, then async composed flips;
  - leaving `On` re-enables the plane with a vsync commit.
  - Mutation: async on the transition commit.
- `EINVAL`: the same frame is resubmitted vsync, and the next frame tries async.
  Two `EINVAL`s with the same shape suspend; a shape change clears the suspension.
  Mutation: suspend after one, or never suspend.
- Cursor-only recomposition cap: at most one per refresh interval; a client frame
  is not delayed. Mutation: remove the cap, or apply it to client frames.
- A Present without `Async` leaves `On`.
- `Flip`/`Skip` completion order unchanged with async flips.
- `PresentCaps` advertises `Async` exactly when the device has the capability.
- Entry from **held direct scanout with a visible HW cursor** (review r1 B-1):
  - grouped unflip;
  - COW materialized;
  - direct ownership kept until retirement;
  - hide at retirement;
  - first async composed flip only on the frame after a successful hide.

  Mutations: async before the hide is confirmed; skip the unflip request.
- Hide failure during the handoff: no async flip with the plane still bound.
- While `On`: `ShowCursor` and direct cursor refresh do not rebind the plane;
  hidden→visible during direct async flips takes the handoff.
- Probation (review r1 M-1):
  - tearing entry from an initially SW cursor;
  - sustained composed tearing does not reset or unflip on every Present;
  - alternating full/region-limited Presents still do not thrash direct entry.

  Mutation: reuse the direct-gate counter for tearing.
- Completion (review r1 M-2): several ready Copies and one composed async flip keep
  today's Copy completion. Direct→Copy ordering is preserved.
- End state (rule A), parameterized over exits to HW, SW (transformed output),
  Hidden (XFIXES) and suspended outputs: no tearing or suspended-shape state is
  left, and the cursor policy matches the exit state (review r1 m-1).

**Hardware** (needs the monitor on the card under test):
- NVIDIA (card1) and the AMD iGPU;
- clients: vkcube in `IMMEDIATE`, and a GL client with swap interval 0;
- runs: cursor hidden, and cursor visible while moving fast;
- check: visible tearing, async-flip counters, no repeated `EINVAL`, no EBUSY
  increase, Xid 0. Record the options NVIDIA's client libraries send.

`tools/cursor-bench` gains a tearing run so the maintainer can repeat it on Intel,
RX 6800 and Apple.

## Non-goals

- Tearing for composed desktops, non-fullscreen windows or the COW of a compositor
  that is not direct-scanout eligible.
- Showing every client frame; queue depth beyond today's single successor.
- `PresentCapabilityAsyncMayTear` / mailbox-vs-tearing distinction (Xwayland model).
- VRR / adaptive sync.
- Async cursor movement through the legacy cursor ioctl.
- Any `YSERVER_*` environment switch.
- Hardware overlay planes.

## Open items for review

- Multi-output direct scanout flips several CRTCs in one commit. Whether drivers
  accept that as async is driver-specific. The `EINVAL` path covers rejection;
  confirm on hardware.
- Per-Present mode switching: VKD3D-Proton with `VK_EXT_swapchain_maintenance1`
  picks FIFO or the unlocked mode per Present (`swapchain.c:3027-3033`). A client
  alternating `Async` and non-`Async` every frame with a visible cursor would pay a
  vsync handoff each time. In practice the switch happens at menus. The tearing
  probation (8 Presents) bounds re-entry; confirm that this is enough.
- Scene tick pacing: confirm that a damage wake composes immediately when no commit
  is in flight, or name the change needed (see §1, composed frames).
- Async flip completion latency per driver is not measured. It does not affect the
  goal, but it decides how often the successor slot skips.
