# Tearing on demand (async page flips) — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** a fullscreen, direct-scanout-eligible client that presents with `PresentOptionAsync` gets
`DRM_MODE_PAGE_FLIP_ASYNC` flips; everything else stays tear-free.

**Architecture:** a backend tearing state (`Off` / `On(window)`) decided per Present on the direct path.
- **For the whole `On` state** the hardware cursor plane is suppressed: nothing binds it, and the scene draws a
  visible cursor (KWin model). Async is authorised at the submit point only if **no output's cursor plane is
  physically visible** (plan review r1 B-1).
- **Cursor hidden while `On`:** direct-scanout commits go async.
- **Cursor visible while `On`:**
  - the hardware cursor plane is suppressed and the scene draws the cursor (KWin model);
  - the client falls back to the Copy path;
  - the composed shared-route flips go async.
- **Common to both:** async requests carry no `OUT_FENCE_PTR` and no cursor-plane property. An `EINVAL` resubmits
  the same frame with vsync. Two consecutive `EINVAL`s on one shape suspend async for that shape.

**Tech Stack:** Rust, `drm` 0.15 (`DriverCapability::AtomicASyncPageFlip`, `AtomicCommitFlags::PAGE_FLIP_ASYNC`),
the #244 atomic commit sink (`crates/yserver/src/drm/atomic.rs`), the 3b `OutputSink` seam, the M2 direct-scanout
state (`scanout_m2`).

**Status:** converged after 7 codex review rounds (findings r1–r6 incorporated, r7 0 findings; `docs/superpowers/findings/2026-10-14-tearing-plan-review-r{1..7}.md`). Awaiting user review.

**Spec:** `docs/superpowers/specs/2026-10-14-tearing-on-demand-design.md` (approved by the user 2026-10-14). The
maintainer approved the HLD change and the Async-only protocol choice, and asked that the visible-cursor composed path
be measured on hardware (Task 6).

**Spec open items resolved while planning:**
- **Scene tick pacing:** no change needed. `backend_scanout_next_wakeup` (`scanout.rs:2036-2041`) returns `now` as
  soon as the scene wants to compose and an output has no commit in flight (`has_output_ready_for_submit`). With
  async flips retiring quickly, a composed frame goes out as soon as its Copy damage arrives. Task 4 pins this with a
  test.
- **VKD3D-Proton per-Present mode switching:** covered by the tearing probation, since any non-Async root Present
  resets it. Task 3 tests alternation.
- **BO retirement without an out fence:** Task 2 must establish it before any async commit is issued.

## Global Constraints

- **Protocol:**
  - advertise `PresentCapabilityAsync` (0x1) only when the primary KMS device reports
    `DRM_CAP_ATOMIC_ASYNC_PAGE_FLIP`;
  - never advertise `PresentCapabilityAsyncMayTear`. `async_may_tear` stays `false`, so the core keeps clearing
    option `0x10` (`present_ext.rs:1670`, `:2107`).
- **Tearing entry needs all three:**
  - the Present carries `PresentOptionAsync` (0x1);
  - the direct-scanout gate passes with the cursor term ignored;
  - every CRTC in the transaction belongs to a device with the async capability.
- **Async request:**
  - flags `NONBLOCK | PAGE_FLIP_EVENT | PAGE_FLIP_ASYNC`;
  - no `OUT_FENCE_PTR` (the BO gets `out_fence_fd = -1`);
  - **no cursor-plane property at all**, not even unchanged values;
  - every other primary-plane property as today.
- **`EINVAL` on async:**
  - resubmit the same frame with vsync;
  - the next frame tries async again;
  - two consecutive async `EINVAL`s with the same shape suspend async for that shape (logged once);
  - a shape change clears the suspension;
  - shape = fourcc, modifier, pitch, width, height, the CRTC set, direct vs composed.
- **`EBUSY`:** unchanged. One commit in flight per CRTC; the latest-wins successor slot stays.
- **Copy completion:** unchanged; GPU-copy readiness (`present.rs:154-181`). Only direct frames take MSC/UST from the
  async flip event.
- **Copied (PRIME) scene route stays vsync.** Direct scanout is primary-device only (`submit_direct_frame` uses
  `primary_device()`), so a tearing client's composed frames are on the shared route. A copied route under `On` is a
  bug: `debug_assert!` without side effects, plus a vsync commit.
- No environment variable switches behaviour (project rule).
- **No test-only behaviour in production:**
  - no `cfg(test)` forks in production bodies;
  - test seams are `#[cfg(test)]` items production does not call, or constructor parameters;
  - no side effects in `debug_assert!`;
  - no sleeps to order events.
- **Test rules:**
  - **A:** end-state checks: no tearing state, suspension or deferred cursor wake left after exit; the cursor policy
    matches the exit state; the recorded commit sequence is exact.
  - **B:** advance only through `Backend` trait entries or `yserver_core::core_loop::run_core`.
  - **C:** hardware validation (Task 6).
  - **F:** each new test 5 times under 16 `nproc` busy loops started outside the tests and killed afterwards. Tests
    that wait on GPU work: 20–30 loaded runs.
- Every test names the mutation that must make it fail, and the mutation is run once.
- **State meanings:** before relying on `CursorPlaneMode::Hidden`, read `scene.rs:275-287`. Hidden is produced by
  XFIXES-hide **and** by off-output/dead-zone. Tests include both producers.
- **Gate per task:**
  1. `cargo +nightly fmt` first. Never `fmt --check` on unformatted code: it ate more than 16 GiB once.
  2. `cargo build --bin yserver`.
  3. `cargo test -p yserver --lib`.
  4. Each `crates/yserver/tests/*.rs` file the task touches.
  5. `cargo clippy --all-targets -- -D warnings`.
  6. `tools/check-layout.sh`.
  - New tests go in a new file, `crates/yserver/src/kms/render/backend/tests/tearing.rs`;
    `present_scanout.rs` is already 3947 lines.
- **Commits:** one per task. Trailers `Implemented-By: codex (gpt-6-luna xhigh)` and
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never a session URL.

## Review Focus

1. **Two outputs in one direct transaction.** A driver that rejects multi-CRTC async must reach suspension after two
   `EINVAL`s and then stay vsync without an `EINVAL` per frame. → Task 2 test
   `multi_crtc_async_einval_suspends_after_two`.
2. **A game toggling cursor visibility at menus.** Repeated visible↔hidden while `On` must never produce an async
   commit with the cursor plane still bound, and never leave the plane suppressed after exit. → Task 4 test
   `cursor_toggle_cycles_never_async_with_bound_plane`.
3. **The tearing window is unmapped or destroyed while `On` with a software cursor.** State exits and the HW cursor
   policy returns. → Task 5 test `tearing_window_destroy_exits_and_restores_cursor_policy`.
4. **VT switch or DPMS off while `On`.** No async commit after resume until probation passes again. → Task 3 test
   `vt_release_exits_tearing_and_resume_needs_probation`.
5. **An async Present from a non-fullscreen window while another window is `On`.** The state is untouched and no
   async commit is issued for the small window. → Task 3 test `async_from_non_fullscreen_window_does_not_affect_on`.

---

### Task 1: Async capability and protocol advertisement

**Files:**
- Modify: `crates/yserver/src/kms/render/platform.rs` (`KmsDevice`, ~L379)
- Modify: `crates/yserver/src/kms/render/platform/init.rs` (both `KmsDevice` constructions, ~L347 and ~L786)
- Modify: `crates/yserver-core/src/backend/trait_def.rs` (`PresentCaps`, L173-215, and its tests ~L320)
- Modify: `crates/yserver/src/kms/render/backend/kms/present.rs` (`backend_present_present_capabilities`, ~L1618)
- Modify: `crates/yserver-core/src/backend/trait_def.rs` (`Backend::present_capabilities`, L2584) and
  `crates/yserver/src/kms/render/backend/trait_impl.rs` (L1764)
- Modify: `crates/yserver-core/src/core_loop/process_request/present_ext.rs` (callers L1527, L1669, L2106)
- Modify: `crates/yserver/src/kms/render/platform/tests.rs` (the test `KmsDevice` constructors, ~L525, L1115, L1140)
- Test: `crates/yserver/src/kms/render/backend/tests/tearing.rs` (new; register it in `tests/mod.rs`)

**Interfaces:**
- Produces:
  - `KmsDevice::async_page_flip: bool`: read once at open with
    `get_driver_capability(DriverCapability::AtomicASyncPageFlip)`. An error or 0 means `false`; FreeBSD drm-kmod may
    lack the cap.
  - `PresentCaps::async_flip: bool`. `encode()` sets bit `0x1` when `flip_path || async_flip`. `flip_path` keeps its
    meaning and value.
  - `Backend::present_capabilities(&self, window: u32, crtc_id: u32) -> PresentCaps` (plan review r5 B-1). The core
    passes the CRTC that the window's Present would use, resolved by **the same rule as Present domain selection**
    (`select_present_domain`, `present_ext.rs:646-667`):
    - `QueryCapabilities` on a CRTC target: that CRTC;
    - `QueryCapabilities` on a window, and `PresentPixmap` / `PresentPixmapSynced`: the request's `target_crtc` if
      nonzero, else `default_present_crtc_for_window(state, window)`;
    - no CRTC: 0.
  - The KMS backend maps `crtc_id` through `present_crtc_output` (`present.rs:617`) to the owning `KmsDevice` and uses
    that device's `async_page_flip`. An unknown, off or 0 CRTC gives `async_flip = false`. Other backends keep their
    current answer.

**Invariants:**
- `backend_present_present_capabilities` returns `async_flip` from the device owning the resolved CRTC, and
  `async_may_tear = false`. This keeps the Phase-4.2 contract that capabilities are per window ("because Present's
  `QueryCapabilities` is per-window", Phase-4.2 spec:475-477, `363dd7d7`).
- Nothing else reads `async_flip`. The tearing decision (Task 3) reads the device flags directly.

- [ ] **Step 1: tests** (in `tearing.rs` and the `trait_def.rs` test module):
  - `present_caps_encode_async_bit_from_async_flip`: `PresentCaps { flip_path: false, async_flip: true,
    async_may_tear: false, syncobj: false }.encode()` has `0x1` set and `0x8` clear. Mutation: drop `async_flip` from
    `encode`.
  - `present_capabilities_follow_the_windows_output_device` (plan review r5 B-1):
    - two KMS devices, A with `async_page_flip = true` and B with `false`, one output each;
    - a window on A's output gets `& 0x1` set; a window on B's output gets it clear; a CRTC target answers for its own
      device; an unknown CRTC (0) answers clear;
    - swap the flags and the answers swap;
    - `& 0x8 == 0` always.
    - Driven through `QueryCapabilities` in the core loop (rule B).
    - Mutations: use the primary device; hard-code `async_flip: true`.
- [ ] **Step 2:** run them; they fail to compile or fail.
- [ ] **Step 3:** implement the field, the probe at both open sites, the encode change and the backend caps.
- [ ] **Step 4:** run the gate.
- [ ] **Step 5:** commit `feat(present): advertise PresentCapabilityAsync on devices with atomic async flips`.

### Task 2: Async request building, EINVAL fallback and shape suspension

**Files:**
- Modify: `crates/yserver/src/drm/atomic.rs`:
  - `AtomicCommitKind` (L20): add `Async`;
  - `CommitWindow` metrics;
  - `tests_support`: the recording executor already scripts results.
- Modify: `crates/yserver/src/drm/modeset.rs` (`submit_direct_scanout` L1570, `direct_scanout_request` ~L1625)
- Modify: `crates/yserver/src/drm/page_flip.rs` (`submit_scene_flip_with_fences` L183)
- Modify: `crates/yserver/src/kms/render/platform/scene_sink.rs` (`submit_shared_scanout_frame` ~L455)
- Create: `crates/yserver/src/drm/async_flip.rs`: the shape key and the suspension ledger, one responsibility. Register
  it in `drm/mod.rs`.
- Test: `crates/yserver/src/kms/render/backend/tests/tearing.rs`

**Interfaces:**
- Produces (in `drm/async_flip.rs`):
  - `pub(crate) enum FlipTiming { Vsync, Async }`
  - `#[derive(Clone, Eq, PartialEq, Hash, Debug)] pub(crate) struct AsyncShape { fourcc: u32, modifier: u64, pitch:
    u32, width: u32, height: u32, crtcs: Vec<crtc::Handle> /* sorted */, direct: bool }`
  - `pub(crate) struct AsyncFlipLedger`, with:
    - `fn allows(&self, shape: &AsyncShape) -> bool`
    - `fn note_einval(&mut self, shape: &AsyncShape) -> bool` (true = now suspended, log once)
    - `fn note_accepted(&mut self, shape: &AsyncShape)` (clears the consecutive count)
    - `fn clear(&mut self)`
  - `pub(crate) enum AsyncStream { Direct, Composed(crtc::Handle) }`. Each independent submission stream has its own
    history: the grouped direct transaction is one stream, and each shared-route output's composed flip is another
    (plan review r2 M-1). The ledger API takes `(stream, shape)`: `allows(stream, shape)`,
    `note_einval(stream, shape)`, `note_accepted(stream, shape)`, `clear()`.
  - **Ledger semantics, per stream:**
    - "consecutive" counts async `EINVAL`s on that stream's current shape, with no accepted async commit on it in
      between;
    - a different shape **on the same stream** resets that stream's count and lifts that stream's suspension;
    - other streams are untouched;
    - each stream remembers only its current shape.
  - **Ledger owner (plan review r1 M-1):** one `AsyncFlipLedger` lives in `PlatformBackend` (`platform.async_ledger`),
    because both submit routes run there or reach it synchronously: `submit_direct_frame` through `self.platform`,
    and the scene's `finish_frame` as `PlatformBackend`. Each route consults and updates it in the same call that
    submits. No outcome has to travel back through `FinishOutcome`. The backend clears it through
    `platform.async_ledger.clear()` on every exit from `On` (Task 3).
- Produces (submit functions): `submit_direct_scanout(.., timing: FlipTiming)` and
  `submit_scene_flip_with_fences(.., timing: FlipTiming)`.
  - **With `FlipTiming::Async`:**
    - flags gain `PAGE_FLIP_ASYNC`;
    - no `OUT_FENCE_PTR` (the out-fence holder stays `-1`);
    - `cursor_positions` is ignored (no cursor-plane property recorded);
    - kind `AtomicCommitKind::Async`.
  - **With `Vsync`:** byte-identical to today (a test pins this).
- Produces (callers):
  - `submit_direct_frame` (`scanout.rs:506`) and `submit_shared_scanout_frame` take a `FlipTiming`. If it is `Async`
    and the commit fails with `EINVAL`, they resubmit the same frame with `Vsync` (cursor positions included, as today)
    and report `AsyncOutcome::FellBack { suspended: bool }`. An accepted async commit reports
    `AsyncOutcome::Accepted`; a vsync request reports `AsyncOutcome::NotRequested`.
  - The BO phase handling of the resubmit uses the existing reject→recording transition
    (`transition_to_recording_after_atomic_reject`), then submits again; no new BO phase.
- Consumes: nothing from later tasks. Callers pass `FlipTiming::Vsync` until Task 3.

**Invariants:**
- An `EBUSY` on async is not an `EINVAL`. It follows today's EBUSY path and never counts toward suspension.
- #246's cursor attribution (`cursor_caused_rejection`) never runs for `AtomicCommitKind::Async`; it carries no cursor
  fields.
- **BO retirement without a release fence (spec §2):** read `bo_state.rs` (`transition_to_retiring`,
  `transition_to_free_after_retire`), `flip_events.rs` (~L564) and their callers.
  - The commit message must state where `Retiring → Free` waits for GPU readers when `release_fence_fd` is `None`,
    covering the shared route and lifecycle reset.
  - If any path frees a BO on the replacement event alone while a GPU reader may still run, stop and report; do not
    patch around it.

- [ ] **Step 1: tests** (`tearing.rs`, recording sink as in `tests/atomic_cursor.rs` `recording_sink`):
  - `async_direct_request_has_flag_no_out_fence_no_cursor`: a seeded atomic cursor position plus a direct submit with
    `Async` records one commit with `PAGE_FLIP_ASYNC`, kind `Async`, no property on the cursor plane, no
    `OUT_FENCE_PTR`. Mutations: keep cursor positions; keep the out fence.
  - `async_scene_request_has_flag_no_out_fence_no_cursor`: the same for a shared scene flip (use
    `set_scene_fence_properties`).
  - `vsync_requests_unchanged`: the `Vsync` direct and scene requests equal today's recorded property lists exactly.
    Mutation: add `PAGE_FLIP_ASYNC` unconditionally.
  - `async_einval_resubmits_same_frame_vsync`: script `EINVAL` then `Ok`. The records are [Async with framebuffer F,
    Vsync with the same F, carrying cursor positions], the outcome is `FellBack { suspended: false }`, and the next
    frame records `Async` again. Mutation: drop the resubmit.
  - `multi_crtc_async_einval_suspends_after_two` (Review Focus 1): two outputs, both direct, scripted `EINVAL` on every
    async. Frame 1 gives Async+Vsync, frame 2 gives Async+Vsync with `suspended: true`, frames 3 and 4 give Vsync only.
    A shape change (different CRTC set) records Async again. Mutation: suspend after one; never suspend.
  - `composed_async_einval_suspends_after_two` (plan review r1 M-1): drive two composed frames through
    `Backend` entries with scene flip timing `Async` and scripted `EINVAL` on every async. The commits are
    Async+Vsync, Async+Vsync, then Vsync only, and the platform ledger reports suspension. Mutation: keep the ledger
    out of `finish_frame`.
  - `two_outputs_composed_both_rejecting_each_reach_suspension` (plan review r2 M-1): two shared-route outputs A and B,
    both composing async with scripted `EINVAL` on every async. Interleaved per frame, A and B each record
    Async+Vsync twice, then Vsync only. Neither erases the other's history. Mutation: use a single current-shape slot.
  - `async_ebusy_is_not_suspension`: scripted `EBUSY` on async follows the EBUSY path, no vsync resubmit, ledger count
    0. Mutation: treat EBUSY as EINVAL.
- [ ] **Step 2:** run them; they fail.
- [ ] **Step 3:** implement `async_flip.rs`, the timing parameter, the fallback and the outcome reporting. Callers pass
  `Vsync` for now.
- [ ] **Step 4:** run the gate.
- [ ] **Step 5:** commit `feat(kms): async flip requests with EINVAL vsync fallback and shape suspension`.

### Task 3: Tearing state, probation and direct async flips (cursor hidden)

**Files:**
- Modify: `crates/yserver/src/kms/render/backend.rs` (new `TearingState` beside `ScanoutM2State`, ~L440)
- Modify: `crates/yserver/src/kms/render/backend/kms/present.rs` (`try_present_direct` gate, ~L735-790)
- Modify: `crates/yserver/src/kms/render/backend/kms/scanout.rs` (`submit_direct_frame` L506,
  `submit_queued_direct_successor` L569, `quiesce`/teardown exits)
- Modify: `crates/yserver/src/kms/render/backend/kms/crtc_config.rs` (`quiesce_before_topology_mutation` L67: exit)
- Modify: `crates/yserver/src/kms/render/backend/kms/session.rs` (`run_suspend`, DPMS off, `disable_output`: exit)
- Test: `crates/yserver/src/kms/render/backend/tests/tearing.rs`

**Interfaces:**
- Produces, in `backend.rs`:

  ```rust
  pub(super) enum TearingState { Off, On { window: u32 /* dst_window_xid */, crtc_epoch: u64 } }
  pub(super) struct Tearing {
      state: TearingState,
      probation: u8,      // consecutive qualifying authoritative-root Presents, entry only
      probation_owner: Option<u32>, // dst_window_xid of the streak; a different qualifying window restarts at 1
      // Tasks 4–5 add fields here. The ledger is `platform.async_ledger` (Task 2).
  }
  pub(super) const TEARING_ENTRY_PROBATION: u8 = 8; // same count as SCANOUT_M2_ELIGIBLE_ROOT_PROBATION
  ```

- Produces: `KmsBackend::tearing_gate_ignoring_cursor(&self, candidate, ...) -> bool`. It is the existing
  `scanout_direct_eligible` input set plus shape chain, CRTC eligibility, untransformed outputs and every CRTC's device
  `async_page_flip`, with `cursor_hw = true`.
  - Do not duplicate the gate: refactor `try_present_direct` (~L738-754) so the real gate and this one share one
    function, taking `cursor_term: bool`.
- Produces: `KmsBackend::exit_tearing(&mut self, reason: &'static str)`. It sets `Off`, zeroes the probation, clears
  `platform.async_ledger`, clears `hw_cursor_suppressed`, and is idempotent. Task 5 extends it to restore cursor
  policy.
- Produces: `SceneInput::hw_cursor_suppressed: bool` (`yserver-render`). The scene computes
  `hw_cursor = hw_cursor_allowed(outputs) && !input.hw_cursor_suppressed` (`tick.rs:212`). The backend sets it to
  `true` exactly while `On`.
- Produces: `SceneCompositor::cursor_transition_pending(&self) -> bool` (`yserver-render`). It is true while any
  output's submitted, unretired frame carries a `CursorTransition` (`PendingAck`, `scene.rs:231-244`). These are the
  two `yserver-render` additions; nothing else changes there.
- Produces: `PlatformBackend::any_cursor_plane_visible(&self) -> bool`. It is the physical plane state
  (`cursor_plane().visible(output)` on any output), never the scene's classification.

**Invariants:**
- **Probation** (spec §1 "Probation"):
  - it advances on an authoritative-root Present from window W carrying `0x1` that passes the gate with the cursor
    ignored;
  - it resets on an authoritative-root Present that fails that gate or lacks `0x1`;
  - **owner** (plan review r3 M-1): the streak belongs to `probation_owner`. A qualifying Present from a different
    window restarts the streak at 1 with that window as the owner; it never adds to another window's count;
  - entry happens at `TEARING_ENTRY_PROBATION`, for the owner;
  - **entry is serialized behind queued cursor transitions** (plan review r4 B-1). If
    `scene.cursor_transition_pending()` is true when the streak reaches the threshold, entry is deferred: the streak
    stays at the threshold, and the next qualifying owner Present after the transitions retired enters. Nothing that
    was queued before entry can retire into a bound plane while `On`. After entry, suppression stops new HW
    transitions from being created, which keeps the retirement-time binding of `a14edf52` (Stage-5 cursor
    plan:385-397) authoritative outside `On`;
  - the counter is independent of `scanout_m2`'s admission counter (`admit_eligible_root`), which keeps its meaning;
  - a Present for a window other than W while `On` neither enters nor exits (Review Focus 5) unless it is an
    authoritative-root Present, which then decides like any other.
- **Exits:**
  - an authoritative-root Present from W without `0x1`;
  - W fails the gate with the cursor ignored;
  - `quiesce_before_topology_mutation`;
  - `run_suspend` (VT release);
  - DPMS off;
  - `disable_output` (shutdown);
  - destroy or unmap of W: hook every site that already tears down direct scanout for a window. Find them by grepping
    the callers of `direct_frame_references_host_drawable` and `request_direct_unflip`, and list them in the commit
    message.
- **Cursor suppression for the whole `On` state** (plan review r1 B-1). From entry to exit, no path binds the
  hardware cursor plane:
  - `bind_direct_cursor_on_all_outputs` takes its hide branch while `On`, even when XFIXES is not hiding the cursor;
  - `refresh_direct_cursor_on_all_outputs` returns without touching the plane;
  - XFIXES `ShowCursor`'s direct rebind (`cursor.rs:484-492`) is skipped;
  - the sprite-refresh path (`cursor.rs:667-686`) treats the cursor as SW;
  - the scene, through `hw_cursor_suppressed`, never assigns HW.

  A cursor that becomes visible while `On` (ShowCursor, or motion from a dead zone onto an output) is not bound;
  Task 4's handoff makes it visible through the scene. `cursor_mode()` reports **retired** output modes
  (`scene/cursor.rs:110-137`) and is never used as the trigger (plan review r2 B-1). This deliberately overrides the M2 rule "on direct entry, bind the uploaded
  hardware cursor to every participating CRTC once" (M2 plan:67-69, `9c4ef8f0`) while `On`; outside `On` it is
  unchanged.
- **Direct submit timing:** `submit_direct_frame` and `submit_queued_direct_successor` pass `FlipTiming::Async`
  exactly when all of these hold. Otherwise they pass `Vsync`.
  - `On`;
  - the frame's window is W;
  - `!platform.any_cursor_plane_visible()`;
  - `platform.async_ledger.allows(shape(frame))`.

  The scene's `Hidden` classification is not the authority.
- **Completion:** direct frames still complete as `Flip` with the event's MSC/UST. No change to Copy.

- [ ] **Step 1: tests** (`tearing.rs`, driving `Backend::try_present_direct`/Present entries as
  `tests/present_scanout.rs` does, with XFIXES-hidden cursor **and** a dead-zone Hidden producer):
  - `entry_needs_async_option_gate_and_capability`: 8 qualifying Presents enter `On` and the 9th direct commit is
    `Async`. Then three variants: without `0x1`, with a non-fullscreen candidate, and with `async_page_flip = false`.
    None of them enters, and every commit is `Vsync`. Mutation: drop any one condition.
  - `probation_resets_on_non_async_root_present`: 7 qualifying Presents, 1 without `0x1`, 7 more: still `Off`.
    Mutation: no reset.
  - `async_from_non_fullscreen_window_does_not_affect_on` (Review Focus 5).
  - `non_async_present_from_w_exits`: `On`, then a W Present without `0x1`: `Off`, that commit is `Vsync`, and the
    ledger is clear (rule A).
  - `vt_release_exits_tearing_and_resume_needs_probation` (Review Focus 4): `On`, then VT release and resume. Commits
    stay `Vsync` until 8 new qualifying Presents. Also cover DPMS off/on.
  - `topology_change_exits_tearing`: `On`, then `quiesce_before_topology_mutation`: `Off`.
  - `entry_waits_for_queued_show_on_retire` (plan review r4 B-1): 7 qualifying Presents; a scene frame is accepted
    carrying `ShowOnRetire`; the 8th qualifying Present arrives before that frame retires, and the state stays `Off`.
    The frame retires and binds the plane outside `On`. The next qualifying Present enters `On`, and the handoff of
    Task 4 runs. No plane binding is recorded while `On`. Mutation: enter regardless of `cursor_transition_pending`.
  - `probation_restarts_when_another_window_takes_the_root` (plan review r3 M-1): 4 qualifying Presents from A, a
    restack raises B, then 4 qualifying Presents from B: still `Off`; 4 more from B: `On` for B. Mutation: a
    counter without an owner.
  - `alternating_async_and_vsync_presents_never_enter` (VKD3D-Proton per-Present switching): 40 W Presents
    alternating `0x9`/`0x8` stay `Off`, and every commit is `Vsync`. Mutation: advance probation on non-Async
    Presents.
  - `dead_zone_hidden_cursor_tears_like_xfixes_hidden`: the same entry with the Hidden state produced by the pointer
    in a dead zone between outputs. Mutation: gate async on XFIXES-hidden only.
  - `hidden_entry_never_binds_plane_on_retirement_show_or_motion` (plan review r1 B-1):
    - enter `On` with the cursor Hidden by the dead zone (XFIXES false);
    - retire a direct frame: `bind_direct_cursor_on_all_outputs` must not show the plane;
    - move the pointer onto an output, then XFIXES-hide and ShowCursor;
    - after every step, assert the physical plane state is not visible and every `Async` commit had no visible plane.
    - Mutation: leave the non-XFIXES bind branch active while `On`.
- [ ] **Step 2:** run them; they fail.
- [ ] **Step 3:** implement `Tearing`, the shared gate, entry, exits, cursor suppression and direct timing.
- [ ] **Step 4:** run the gate, including `crates/yserver/tests/*.rs` files that cover present and scanout.
- [ ] **Step 5:** commit `feat(kms): tearing state with its own probation; async direct flips when the cursor is
  hidden`.

### Task 4: Visible cursor — handoff, suppression and async composed flips

**Files:**
- Modify: `crates/yserver-render/src/kms/render/scene.rs` / `scene/tick.rs` (`SceneInput`, `hw_cursor` at
  `tick.rs:212`)
- Modify: `crates/yserver/src/kms/render/backend.rs` (`scene_input(...)` construction)
- Modify: `crates/yserver/src/kms/render/backend/kms/scanout.rs` (`maybe_composite` held-direct branch
  L2165-2212, `request_direct_unflip`, `bind_direct_cursor_on_all_outputs` L365,
  `refresh_direct_cursor_on_all_outputs` L421)
- Modify: `crates/yserver/src/kms/render/backend/kms/cursor.rs` (`ShowCursor` rebind L484-492; sprite refresh
  L667-686)
- Modify: `crates/yserver/src/kms/render/backend/kms/present.rs` (ineligible-root branch L775-783)
- Modify: `crates/yserver/src/kms/render/platform/scene_sink.rs` (`finish_frame` shared route: timing)
- Test: `crates/yserver/src/kms/render/backend/tests/tearing.rs`

**Interfaces:**
- Consumes (Task 3): `hw_cursor_suppressed`, `any_cursor_plane_visible()`, `platform.async_ledger`, `exit_tearing`.
- Produces:
  - `PlatformBackend::set_scene_flip_timing(output_idx, FlipTiming)`, read by `finish_frame` (shared route only).
    `maybe_composite` sets it for **every** output at the start of **every** pass. The default is `Vsync`, so stale
    timing cannot outlive the pass that decided it (plan review r1 M-2). Copied route: `debug_assert!(timing ==
    Vsync)` without side effects, and vsync is always used.
  - `KmsBackend::tearing_scene_still_owned(&self) -> bool` (plan review r3 B-1). It re-runs **the same shared gate
    function as admission** (Task 3, `cursor_term = false`) against the current scene, with no Present involved.
    - It uses an admission context that `Tearing::On` retains from the last admitted W Present: the target
      classification (`Cow`, `CowDescendant` or `Unredirected`, from `scanout_m0_target`, `scanout.rs:1312-1344`), W's
      and the root's host xids, the candidate offsets and the valid region.
    - The target kind is re-derived from the current tree, and the target-specific rules apply as at admission:
      - the topmost-window check (`scanout.rs:1377-1440`) only for `Unredirected`;
      - COW targets keep their own rule (`scanout_m2_is_authoritative_root`, `scanout.rs:27-34`);
      - the complete shape chain must cover the root (`direct_shape_chain_covers_root`, as in `present.rs:727-729`).
    - Also checked: root overlay empty, no transformed output, every participating device async-capable.
    - This restores, for composed tearing, the direct-scanout design's "stacking or shape change" invalidation
      (direct-scanout design:222-227, `9c4ef8f0`).
  - `Tearing` gains `hide_failed_logged: bool` (logs the hide failure once).

**Invariants:**
- **Composed async authorisation** (plan review r1 M-2). At the start of each `maybe_composite` pass while `On`:
  1. If `!tearing_scene_still_owned()`, call `exit_tearing("eligibility_lost")`. Every output's timing stays `Vsync`
     for this pass.
  2. Otherwise, for each output of W's transaction, timing is `Async` iff `!platform.any_cursor_plane_visible()` and
     `platform.async_ledger.allows(shape)`.
  - A dialog mapped above W, or any other scene change, thus returns the desktop to vsync without another W Present.
- **Handoff trigger = desired visibility, not the retired classification** (plan review r2 B-1). Add
  `KmsBackend::tearing_cursor_wants_visible(&self) -> bool`:
  - `!self.cursor_hidden` (XFIXES);
  - an effective cursor record exists;
  - its footprint at the current pointer position intersects at least one output (not in a dead zone).
- **Handoff from held direct with a visible cursor** (spec §1). While `On` and direct is held, the handoff is requested
  whenever `tearing_cursor_wants_visible()` is true. It is checked in three places, all **before** the held-direct
  early return in `maybe_composite` (L2165-2212):
  - at entry to `On` (Task 3's entry point);
  - in XFIXES `ShowCursor`, which while `On` requests the handoff instead of rebinding;
  - at the start of every `maybe_composite` pass, which covers pointer motion from a dead zone onto an output, because
    that branch already wakes the scene.

  The handoff:
  1. The unflip reason is `tearing_cursor_handoff`. The existing grouped unflip materializes the COW and keeps direct
     ownership until retirement (spec D:184-190, 222-231). This replaces the e118bc3e rule "ShowCursor during direct
     scanout binds the plane or unflips, and never leaves the cursor invisible" (atomic-cursor plan:306-307) while
     `On`, with the same guarantee: the cursor becomes visible through the composed scene.
  2. The scene's HW→SW transition hides the plane at retirement (`scene/cursor.rs:529-533`).
  3. Composed async is possible only from the next pass, and only if `any_cursor_plane_visible()` is false. That
     physical check is the single authority; no separate "confirmed" state is kept.
- **Hide failure:** the plane stays visible, so `any_cursor_plane_visible()` keeps every commit `Vsync`. Log once. The
  existing hide-failure fallback is untouched.
- **Ineligible only because of the cursor:** while `On` for W, an authoritative-root Present that fails the real gate
  but passes the gate with the cursor ignored must not call `reset_eligible_root_probation` or
  `request_direct_unflip("ineligible_authoritative_root_present")`. It returns `Ok(false)` (Copy path). Every other
  ineligible case keeps today's behaviour.
- **Fallback source during the handoff** (plan review r6 M-1). This keeps the M2 rule "when an ineligible
  authoritative successor requests unflip, its normal Copy is the current fallback" (M2 plan:110-113, `9c4ef8f0`).
  - Every authoritative W Present that takes the Copy path while the handoff unflip is requested, or while direct is
    still held, registers itself before returning `Ok(false)`, exactly as the ordinary ineligible-root branch does
    (`present.rs:755-768`): `unflip_fallback_source = source_id`, `unflip_shadow_ready = false`. This includes the
    Present that triggers entry.
  - Its Copy notification (`note_present_pixmap`, `present.rs:679-686`) then marks the shadow ready, so the lazy
    materialization (`scanout.rs:775-818`) never copies the older retained direct frame over it.
  - Only when no such successor exists (the handoff came from ShowCursor or motion) does the lazy copy of the retained
    frame apply, as today.
- **Visible→Hidden while `On`:**
  - direct re-entry needs the real direct gate and its own `scanout_m2` probation;
  - once re-entered, direct async applies (Task 3);
  - suppression continues until exit.

- [ ] **Step 1: tests** (`tearing.rs`):
  - `show_cursor_during_held_direct_hidden_tearing_makes_cursor_visible` (plan review r2 B-1):
    - held direct, XFIXES-hidden, `On`, then `ShowCursor`;
    - the handoff unflip is requested in that call or in the next `maybe_composite` pass, before any held-direct
      return;
    - after retirement, the next composed frame's scene build assigns `CursorAssignment::Sw` with a cursor draw in
      `scene.draws` (the cursor is actually visible), and no plane is bound.
    - Repeat for pointer motion from a dead zone onto an output.
    - Mutation: trigger the handoff from `cursor_mode()`, which leaves the cursor invisible.
  - `handoff_keeps_newest_copy_not_retained_direct_frame` (plan review r6 M-1):
    - held direct frame F0 with a visible HW cursor;
    - the entry Present F1 has distinct content, and no further client frames follow;
    - driven through the core Present entry, the composed frames after the handoff sample F1's content, never F0's.
    - Mutation: skip the fallback registration on the cursor-only branch.
  - `entry_from_held_direct_with_visible_hw_cursor_runs_handoff` (review r1 B-1):
    - start held direct with a visible HW cursor and reach `On`;
    - expect: unflip requested with reason `tearing_cursor_handoff`; COW materialized; direct ownership kept until the
      replacement retires; plane hidden at that retirement; the first `Async` scene commit only on the frame after the
      successful hide; every earlier commit `Vsync`.
    - Mutations: async before the hide is confirmed; skip the unflip request.
  - `hide_failure_keeps_visible_cursor_frames_vsync`: a scripted hide failure leaves the plane visible; no `Async`
    commit is recorded while it is visible.
  - `show_cursor_does_not_rebind_plane_while_on`: `On` with composed frames, then XFIXES ShowCursor and a sprite change:
    no plane show/upload recorded. Mutation: remove the guard in `ShowCursor`.
  - `shape_loss_without_w_present_returns_to_vsync` (plan review r3 B-1): composed tearing, then W stops presenting
    and its Bounding shape becomes partial (and, in a second case, empty). The next composed commit is `Vsync` and the
    state is `Off`. Mutation: revalidate geometry and stacking only.
  - `cow_descendant_composed_tearing_stays_on_across_passes` (plan review r3 B-1): a compositor's COW stage (COW
    descendant target) qualifies, enters `On` and composes with a visible cursor. 10 consecutive `maybe_composite`
    passes without new Presents keep `On` and `Async`. Mutation: apply the unredirected topmost check to COW targets.
  - `overlapping_window_without_w_present_returns_to_vsync` (plan review r1 M-2): composed tearing, then W stops
    presenting and a dialog maps above W. The next composed commit is `Vsync` and the state is `Off`. Mutation: skip
    `tearing_scene_still_owned` in `maybe_composite`.
  - `cursor_toggle_cycles_never_async_with_bound_plane` (Review Focus 2): 5 hide/show cycles while `On`. At every
    commit, if it is `Async` then no output's plane is visible.
  - `cursor_only_ineligibility_does_not_reset_direct_probation_while_on` (review r1 M-1): sustained composed tearing
    with 20 W Presents gives no `ineligible_authoritative_root_present` unflip and no `scanout_m2` probation reset.
    Also, with **alternating full/region-limited Presents from a compositor (not `On`)**, behaviour equals today's.
    Mutation: let the cursor-only case reset.
  - `initial_sw_cursor_enters_tearing`: the cursor is SW from the start (transformed outputs excluded), `On` after 8
    qualifying Presents, and composed async flips follow while no plane is visible.
  - `composed_frame_submits_without_waiting_for_vblank_tick`: `On`, no plane visible, the previous async flip retired, then a
    W Copy arrives. `next_wakeup` is `now` and the composed async commit is recorded in the same `maybe_composite`
    pass. Mutation: gate the scene deadline on a vblank target.
  - `copies_keep_copy_completion_with_composed_async` (review r1 M-2): two ready Copies and one composed async flip.
    Both complete as `Copy` with today's GPU-readiness timing. A direct `Flip` followed by a `Copy` keeps per-window
    order.
- [ ] **Step 2:** run them; they fail.
- [ ] **Step 3:** implement.
- [ ] **Step 4:** run the gate; GPU-waiting tests 20–30 loaded runs.
- [ ] **Step 5:** commit `feat(kms): visible cursor while tearing — KWin model with an explicit handoff from direct
  scanout`.

### Task 5: Cursor-only recomposition cap and exit cursor policy

**Files:**
- Modify: `crates/yserver/src/kms/render/backend/portable/pointer.rs` (SW branch, L677-682)
- Modify: `crates/yserver/src/kms/render/backend/kms/scanout.rs` (`backend_scanout_next_wakeup` L2030,
  `maybe_composite` after a composed submit)
- Modify: `crates/yserver/src/kms/render/backend.rs` (`exit_tearing` from Task 3, to restore the policy)
- Test: `crates/yserver/src/kms/render/backend/tests/tearing.rs`

**Interfaces:**
- `Tearing` gains:
  - `last_cursor_compose: Option<Instant>`;
  - `deferred_cursor_wake: Option<Instant>`;
  - `fn cursor_compose_interval(&self, platform) -> Duration`: one refresh interval of W's output (`picked.vrefresh`);
    if several outputs, the shortest.
- **Pointer motion while `On` with a visible (SW) cursor:**
  - if no composed frame went out within the interval, `wake_for_damage()` now;
  - otherwise set `deferred_cursor_wake = last_cursor_compose + interval`.
- `backend_scanout_next_wakeup` includes `deferred_cursor_wake`. When reached, `maybe_composite` calls
  `wake_for_damage()` and clears it.
- Any composed submit while `On` sets `last_cursor_compose = now` and clears `deferred_cursor_wake`. A new client frame
  (Copy damage) is never deferred.

**Invariants:**
- **The cap applies only while `On`.** Outside `On`, the SW branch wakes immediately, as today.
- **`exit_tearing` restores the policy for the exit state** (review r1 m-1):
  - clears `hw_cursor_suppressed` and `deferred_cursor_wake`;
  - the scene's normal strategy then yields HW when allowed, SW on transformed outputs (`hw_cursor_allowed`), Hidden
    when XFIXES-hidden;
  - with KMS suspended (DPMS off or VT away), it only clears state, and the resume path applies the policy.

- [ ] **Step 1: tests** (`tearing.rs`, driving pointer motion through the `Backend` input entry):
  - `cursor_only_motion_recomposes_at_most_once_per_refresh`: `On`, a visible SW cursor and no client frames. 20 motions within one
    interval give exactly one immediate compose plus one deferred compose at the interval. Mutation: remove the cap.
  - `client_frame_is_never_deferred_by_cursor_cap`: a motion just composed, then a W Copy arrives. It composes
    immediately, and the deferred wake is cleared. Mutation: apply the cap to client frames.
  - `exit_restores_cursor_policy_per_state`, parameterized:
    - (a) visible and allowed: HW, plane shown via a vsync commit;
    - (b) transformed output: SW;
    - (c) XFIXES-hidden: Hidden;
    - (d) exit by DPMS off: state cleared, no plane commit while off.
    - Rule A for each: no tearing, suspension or deferred wake left.
  - `tearing_window_destroy_exits_and_restores_cursor_policy` (Review Focus 3).
- [ ] **Step 2:** run them; they fail.
- [ ] **Step 3:** implement.
- [ ] **Step 4:** run the gate.
- [ ] **Step 5:** commit `feat(kms): cap cursor-only recomposition while tearing; restore cursor policy on exit`.

### Task 6: Telemetry, hardware validation and the visible-cursor measurement

**Files:**
- Modify: `crates/yserver/src/drm/atomic.rs` (`CommitWindow`: per-second `async`, `vsync_while_on`,
  `async_einval{direct,composed}`)
- Modify: `crates/yserver/src/kms/render/backend/kms/scanout.rs` and `present.rs`: per-second tearing latency summary
  while `On`:
  - `present_to_submit_us` p50/p99/max, split `direct` vs `composed`, measured from the backend's receipt of the
    Present (direct) or of the Copy damage (composed) to the accepted async commit;
  - composed GPU cost from the existing `avg_gpu_render_ns`;
  - async flip turnaround from the accepted async ioctl to its flip event (p50/p99/max), and `Skip` completions per
    second while `On`;
  - one `log::info!` line `tearing_summary` per second, only while `On`.
- Modify: `tools/cursor-bench/` (add `tests/tearing.sh`; update `README.md`)
- Modify: `docs/status.md` (an entry for this feature)

**Hardware runs (rule C).** Each run needs the monitor on the card under test, no other DRM master, and nothing
using the GPU (check `fuser /dev/dri/cardN /dev/nvidia*` and ask the user first). Stop all card1 runs on any Xid.

| Card | Client | Cursor | Expected |
|---|---|---|---|
| NVIDIA card1 | mpv `--gpu-api=vulkan --vulkan-swap-mode=immediate --fs` | hidden (`--cursor-autohide=always`) | `async` ≈ client fps above refresh, `async_einval` only at entry, visible tearing |
| NVIDIA card1 | same | visible, xdotool moving at 250 Hz | composed async; the cursor follows; no vsync commit after the handoff |
| NVIDIA card1 | mpv `--gpu-api=opengl --opengl-swapinterval=0 --fs` | hidden, then visible | same as Vulkan (NVIDIA sends `0x9`) |
| NVIDIA card1 | `--vulkan-swap-mode=mailbox` and `fifo` | either | no tearing state; no `async` commits |
| AMD iGPU | the same four rows with radv | — | same; Mesa sends `0x19`, and the core clears `0x10` |

For all runs: Xid 0, `EBUSY` not above the #244 baseline, no `panicked`, no `timed out waiting for`.

**Uncapped direct scanout (C1 goal) — acceptance:** with mpv `--vulkan-swap-mode=immediate --fs` and the cursor
hidden, on NVIDIA and AMD, report per second:
- async direct flips vs the client's frame rate;
- `Skip` completions;
- async flip turnaround, from the ioctl to the flip event (p50/p99/max).

The direct flip rate must follow the client well above the refresh rate, not stay at refresh. If a driver's
turnaround caps it, record that driver's number as its limit; it is not a defect of this plan.

**Visible-cursor measurement (maintainer request):** from `tearing_summary`, report for direct (cursor hidden) and
composed (cursor visible, moving):
- `present_to_submit_us` p50/p99;
- the composed GPU cost (`avg_gpu_render_ns`);
- Present-to-flip-event latency: the async event timestamp minus the Present receipt.

Compare with the same client under vsync (`fifo`) on the same build. The result goes into the PR description and
`docs/status.md`.

- [ ] **Step 1:** telemetry, with tests: `tearing_summary_counts_async_and_fallbacks` (recording sink: 3 async, 1
  `EINVAL` fallback → counters 3 / 1 / 1), mutation: count the vsync resubmit as async.
- [ ] **Step 2:** run the gate.
- [ ] **Step 3:** `tools/cursor-bench/tests/tearing.sh` runs the table above for one card, and the README explains
  the expectations.
- [ ] **Step 4:** the hardware runs. The user watches the monitor; send the per-step observation guide first.
- [ ] **Step 5:** `docs/status.md` entry with the measured numbers. Commit
  `feat(kms): tearing telemetry and hardware results`.

### Amendment A (after the NVIDIA hardware run, 2026-10-14, user-approved)

Hardware validation on NVIDIA found three defects; the user approved fixing all three.

1. **Transparent cursor = hidden** (spec §1, "A fully transparent cursor counts as hidden"). mpv hides the pointer with
   an empty cursor image; with tearing `On` the direct gate rejected every frame (`m1_gate_reject[cursor=134]`) and
   all frames were composed. Classify alpha-0-everywhere cursor images at creation (core, glyph, RENDER, animated
   cursors) and treat them as hidden in every visibility decision (scene assignment, `tearing_cursor_wants_visible`,
   direct gate, pointer fast path). Tests: an empty `CreateCursor`/`RenderCreateCursor` image while `On` reaches
   direct async; a cursor with one opaque pixel still takes the handoff; switching between the two through
   `DefineCursor` moves between direct and composed correctly. Mutation: classify by XFIXES only.
2. **The cursor-only recomposition cap missed XTest motion.** With xdotool moving the pointer, the scene built
   ~5,500 frames/s. Apply the cap at the single point every pointer-motion source reaches (physical, XTest,
   WarpPointer), not only the physical path. Test: XTest motion at 1 kHz while `On` with a visible cursor gives at
   most one cursor-only compose per refresh interval. Mutation: cap only the physical path.
3. **Async flip turnaround telemetry was wrong** (~1.2 s while 600 flips/s). Measure from the accepted async ioctl of
   that commit to its own flip event. Test with the recording sink and a scripted event source.

### Amendment B (after the CS2 and AMD runs, 2026-10-15, user-approved; spec §1b)

Core supersession parity with Xorg: `supersede_covered_pending_presents` scraps a victim only while it is parked for
a future MSC; due Presents waiting for their source/acquire fence are never scrapped and execute when the fence
signals, in arrival order. The backend's latest-wins successor slot is unchanged. One exception, to keep presentation order: a fence-waiting Present whose fence signals after a NEWER same-window, same-target Present has already executed completes as Skip instead of executing (showing it would put an older frame on screen after a newer one). This is not scrapping while waiting.

- Find every producer of "pending, waiting for fence" vs "parked for MSC" in `present_pending_exec` before changing
  the predicate; state both meanings in the commit message with file:line.
- Tests (core and Backend-driven): GPU slower than CPU — Presents every 3 ms with acquire points signalling 4 ms after
  submission, over ≥ 20 frames, tearing On: every frame executes (Flip or backend latest-wins Skip only when a flip
  is in flight), none is scrapped while waiting. Vsync-parked successors still scrap parked victims (existing tests
  stay green). The 2026-08-11 tests that asserted scrapping of fence-waiting requests are updated to the new rule,
  each change justified in the report. Mutation: scrap fence-waiting victims again.
- Warframe regression guard (the 2026-08-11 motivation): an uncapped async stream with a flip in flight stays uncapped
  (requests reach the backend, which keeps one successor).
