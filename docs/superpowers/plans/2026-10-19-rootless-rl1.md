# Rootless RL1 (presenter + recording host) — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** milestone RL1 of `specs/2026-10-19-rootless-core-design.md` (r3, §9). It adds `yserver::rootless`, the layer
that delivers each top-level's pixels:
- the `RootlessHost` trait, `HandoffKind`, owned `CpuFrame`/`DmabufFrame` and `FrameEvent` (§4.5);
- the presenter: readiness per top-level (§4.5.1), per-window damage from the store's epoch snapshot/ack, acked only on
  delivery, and stale-generation handling on resize (§4.5);
- two handoffs: a GPU copy of the backing into a per-window swapchain (`VulkanSurface`) and an asynchronous CPU readback
  into a canonical image with a reusable buffer pool (§4.5.2);
- the `next_wakeup` contribution deferred from RL0 (RL0 D4; spec §4.1 "Wakeups");
- a recording host for tests.

**Not in RL1** (see Decisions):
- a `--rootless` CLI mode and the vng scenario (D2, D3);
- the dmabuf handoff (types only, D4);
- root reads (RL1b, D8);
- the macOS host (RL2).

**Architecture:** rootless is a feature of the **surface platform** (KMS is never rootless, spec §7). The presenter is a
field of `SurfaceBackendState`, next to the shell link. The one new seam hook, `Platform::on_rootless_events`, defaults to a
no-op, so KMS stays untouched call for call. The presenter runs at the end of the surface platform's `maybe_composite`,
in the same tail as the reconcile that RL0 placed before it (spec §4.1, §4.6). Host → presenter events travel on the
existing `ShellLink` and are drained in `on_platform_event`. Pixel access that touches drawable layouts lives in
`yserver-render`, because `record_layout_transition` is `pub(crate)` there.

**Branch:** `feat/rootless-rl0` continues (the PR comes at RL2), or `feat/rootless-rl1` cut from it; jos decides. The plan
is written against `fb83716b` (master `9625b3c8` + spec/plan docs + 11 RL0 commits).

**Status:** reviewed by codex; jos decided D1-D8 as recommended (see "Decisions"). Task 7 was revised after review
for two GPU lifetime gaps: presentation-safe swapchain destruction and keeping the backing alive during a copy (Tasks
7a, 7, 7b).

## Facts checked against the tree (fb83716b)

| What | Where |
|---|---|
| RL0 tail call, then compose | `core_loop/run.rs:3102` (`reconcile_and_dispatch`), :3109 (`maybe_composite`) |
| Loop head deadline; `PlatformEvent` dispatch (before the tail) | `run.rs:2223`; :2793 |
| `Backend::on_rootless_events(&mut self, &ServerState, &[RootlessEvent])`, default no-op | `backend/trait_def.rs:1053-1058` |
| `RootlessEvent::Mapped { id, window, kind, geometry, depth, info, above }`; `Rect` (i32, i32, u16, u16) | `rootless/types.rs:149-201`; :87-93 |
| XBackend forwards `next_wakeup`/`maybe_composite`/`on_platform_event` to `P::` hooks; it has **no** `on_rootless_events` override | `kms/render/backend/trait_impl.rs:183-193` |
| Platform seam: `next_wakeup` :206, `maybe_composite` :210, `on_platform_event` :215 | `kms/render/backend/seam.rs:28` |
| **Δ spec §4.6:** a `Platform::rootless_host()` accessor would borrow the whole backend; the presenter needs `store`, `engine`, `platform.gpu` and the host at once. `pstate` is a sibling field | `kms/render/backend.rs:283`, `pstate` :558 |
| Surface hooks: `next_wakeup` (scene branch gated by `any_surface_visible`), `on_platform_event` (drains link, `service_surfaces`), `maybe_composite` | `surface/platform.rs:857-877`, :879-885, :941-965 |
| `SurfaceBackendState` (holds `link`, `ui`) | `surface/platform.rs:91-106` |
| Headless backend = surface backend with no outputs; `output_layouts()` empty | `surface/platform.rs:179-190`, :147-157 |
| **Δ The surface module is built only for `any(not(kms), test)`**, so it is absent from Linux/FreeBSD release builds; `run_headless` is `not(kms)` only | `yserver/src/lib.rs:22-25`, :304-307; `surface/mod.rs:13-14` |
| `ShellLink`: `post` + `wake` (one `PlatformEvent` until drained), `drain`; `ShellEvent` is `Copy` | `surface/link.rs:38`, :91, :76, :130; :17 |
| Manual redirect: the window leaves the scene, the backing **stays** scene-participating | `core_loop/process_request/redirect.rs:140-173` |
| Store damage: `damage` accumulates only on participating drawables; `peek_presentation_damage` / `ack_presentation_damage` (epoch-based, drops rects ≤ the snapshot epoch) | `yserver-render/.../store.rs:1246`, :1352, :1373-1397 |
| With no outputs the scene never acks: `can_submit_scene` needs an output ready | `portable/compose.rs:39-40` |
| Window → backing: `Window.host_xid` (**Δ** spec cites `window.rs:1021` for `redirected_backing`, now :1023); `core.host_window_to_backing: HashMap<u32, PixmapHandle>` | `resources/window.rs:1012`, :1023; `kms/core.rs:2594` |
| Every backing size change is a **new pixmap** (exact-fit check, `compReallocPixmap`), so a new `DrawableId`; ids are never recycled | `portable/redirect.rs:1048-1078`, :946-1000; `store.rs:45` |
| Submit boundary before an external read: `flush_before_damage_exposure` (batch, frame close, submit group) | `portable/compose.rs:182` |
| Synchronous readback model to split (flush, CB, transitions, copy, submit, **wait**) | `engine/put_get.rs:215-385` |
| `begin_op_cb` / `end_and_submit_op` (`pub(super)`); `record_layout_transition` (`pub(crate)`); `touch_render_fence` | `engine/frame.rs:1269`, :1308; `store.rs:755`, :1454 |
| `StagingBuffer::new_for_readback` (HOST_CACHED-preferred), `invalidate_for_read`; module private | `engine/staging.rs:85`, :215; `engine.rs:41` |
| `FenceTicket::poll_signaled`, `wait`; `GpuCore::acquire_fence_ticket` | `gpu/fence.rs:93`, :105, :432 |
| Per-surface swapchain: non-blocking acquire, copy CB with acquire wait / ready signal, present, out-of-date rebuild; composite alpha picks OPAQUE first; `Drop` waits device idle | `surface/swapchain.rs:325-449`, :625-755, :685-693, :955-975 |
| `VK_EXT_headless_surface` swapchains already run on lavapipe in the `--ignored` gate | `surface/swapchain.rs:79-87`; `surface/tests.rs:271-302` |
| Dmabuf export of a promoted drawable; promotion **blocks** on a copy | `yserver-render/.../vk/dri3.rs:443`; `engine/export.rs:27` |
| Size budget (5,000 / 6,000 lines) | `surface/platform.rs` 1011, `swapchain.rs` 1065, `seam.rs` 694, `run.rs` 4870 (RL1 adds nothing to it) |

## Global constraints

- **Rooted is identical.** The presenter is `None` unless `rootless::start` installs it. The seam hook defaults to a no-op,
  and KMS implements none of it. Proof: the full workspace suite and the lavapipe run stay green with no edits to
  existing tests (Task 5 is a refactor gated on exactly that).
- **No CLI, no env switch** in RL1 (D2). The only caller of `rootless::start` is the test fixture.
- **Never a fence wait on the core thread on the CPU path** (spec §4.5). GPU swapchains are retired, never destroyed
  in place. Retirement polls present fences (maintenance1); without that extension it falls back to one
  `vkQueueWaitIdle`, never `device_wait_idle` (Task 7, Risks).
- **Oracle.** Expected values come from a spec rule (§4.0, §4.5, §4.5.1, §4.5.2, cited per test), from the X protocol, or
  from an existing tested reader used as a reference (`Engine::get_image` on the backing). A value with no such source is
  not written.
- **Red first, mutation named and run once.**
- **Gate per task** (each command under `env -u RUSTC_WRAPPER`):
  1. `cargo +nightly fmt --all`
  2. `cargo clippy --workspace --all-targets -- -D warnings`
  3. `cargo clippy --workspace --all-targets --features xdmcp -- -D warnings`
  4. `cargo test --workspace --all-targets --locked`
  5. `YSERVER_ALLOW_SOFTWARE_VULKAN=1 VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json cargo test --workspace
     --locked --no-fail-fast -- --ignored`
  6. `tools/check-layout.sh`
- **macOS builds the module** (surface is `not(kms)`) but nothing calls it until RL2. `rootless/mod.rs` carries
  `#![cfg_attr(not(test), allow(dead_code))]`, as `surface` does (`lib.rs:24`); RL2 removes it. The local gate cannot
  run macOS clippy; jos's push runs it in CI (`ci.yml:165`).
- **Commits:** one per task, made by jos after the gate. Trailer `Co-Authored-By: Claude Opus 5.5 (1M context)
  <noreply@anthropic.com>`. Never a session URL. No HW smoke: nothing user-visible changes.

## Module layout

`crates/yserver/src/rootless/` (in `lib.rs`: `#[cfg(any(not(kms), test))] pub(crate) mod rootless;`, as `surface`):

| File | Holds | ~lines |
|---|---|---|
| `mod.rs` | `start`, `Presenter` (per-window table, tick, events), re-exports | 260 |
| `host.rs` | `HandoffKind`, `RootlessHost`, `CpuFrame`, `DmabufFrame`, `FrameEvent`, `FrameSeq` | 150 |
| `frames.rs` | pure model: `Readiness`, `DamageHistory`, pool fill plan, held snapshots | 260 |
| `cpu.rs` | CPU path: readback start/complete, canonical image, pool, `CpuFrame` build | 260 |
| `window_swapchain.rs` | GPU path: per-window swapchain over the shared `Chain` | 240 |
| `tests/{mod,frames,cpu,readiness,gpu}.rs` | `RecordingHost`, fixture, tests | 1,300 |

In `yserver-render`: `engine/handoff.rs` (~160 lines: `begin_readback`, `PendingReadback`) and two `DrawableStore` methods.

---

### Task 1: Asynchronous readback and external-read barriers in `yserver-render`

**Files:**
- Create `engine/handoff.rs` (registered in `engine.rs` next to `mod put_get;`):

```rust
/// A submitted copy of rects of a drawable into host memory. Never waits.
pub struct PendingReadback {
    ticket: FenceTicket,
    staging: Arc<StagingBuffer>,
    /// (rect in drawable pixels, byte offset in `staging`), tightly packed BGRA rows.
    regions: Vec<(vk::Rect2D, u64)>,
}

impl PendingReadback {
    /// Whether the copy finished. A poll, never a wait.
    pub fn poll(&self, vk: &VkContext) -> Result<bool, vk::Result>;
    /// Copy every region into `dst` (stride `dst_stride`, BGRA). Only after `poll` returned true.
    pub fn write_into(&self, dst: &mut [u8], dst_stride: usize) -> Result<(), vk::Result>;
    pub fn regions(&self) -> impl Iterator<Item = vk::Rect2D> + '_;
}

impl RenderEngine {
    /// `get_image`'s flush + transition + copy + submit for several rects of a depth-24/32
    /// drawable, without the wait (put_get.rs:215-385). Rects are clamped to the extent; empty
    /// rects are dropped. Touches the drawable's render fence, so its storage outlives the copy.
    pub fn begin_readback(&mut self, store: &mut DrawableStore, gpu: &mut GpuCore,
                          src: DrawableId, rects: &[vk::Rect2D]) -> Result<PendingReadback, RenderError>;
}
```

- `store.rs`, beside `record_layout_transition` (:755): two `pub` wrappers for the GPU path (Task 6). They do what
  `get_image`'s two transitions do (put_get.rs:300-337), for a caller's command buffer:

```rust
/// Transition `id` to TRANSFER_SRC_OPTIMAL after all prior writes; returns its image and extent.
pub fn begin_external_read(&mut self, id: DrawableId, vk: &VkContext, cb: vk::CommandBuffer)
    -> Option<(vk::Image, vk::Extent2D)>;
/// Back to SHADER_READ_ONLY_OPTIMAL for the scene and paint paths.
pub fn end_external_read(&mut self, id: DrawableId, vk: &VkContext, cb: vk::CommandBuffer);
```

- `gpu/fence.rs:105` `FenceTicket::wait`: increment a `thread_local!` counter, read by `pub fn
  core_thread_fence_waits() -> u64`. It is the "engine's wait counter" of spec §10. It is thread-local because the
  tests run in parallel and each test is its own core thread.

**Tests** (`engine/tests/handoff.rs`, `#[ignore = "needs live Vulkan ICD"]`, `live_gpu()` + `alloc_drawable_3a`):
- `begin_readback_matches_get_image_per_rect`: fill a 64×32 drawable with two colours split at x=20, then read back
  `[(0,0,8,8), (30,10,12,6)]`. Poll in a bounded loop (2 s). `write_into` a zeroed 64×32 canvas. Each rect's bytes equal
  `get_image` of that rect, and the bytes outside the rects stay zero. Oracle: `get_image` (put_get.rs:215).
  Mutation: give every region buffer offset 0.
- `begin_readback_never_waits`: `core_thread_fence_waits()` is unchanged across `begin_readback` and the polls, and
  it rises by one across a `get_image` (control: the counter counts). Mutation: call `ticket.wait` in `begin_readback`.
- `external_read_restores_the_sampled_layout`: after begin/end in one submitted CB, a following `get_image` succeeds and
  reads the same bytes (the layout bookkeeping is consistent). Mutation: skip `end_external_read`.

**Gate**, then commit `feat(render): asynchronous multi-rect readback and external-read barriers`. ~200 + 150 test lines.

### Task 2: The presenter's pure model (readiness, damage history, buffer pool)

**Files:** create `rootless/host.rs` (types only) and `rootless/frames.rs`. No Vulkan, so these run in the normal
`cargo test`.

```rust
// host.rs — spec §4.5, owned and Send
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)] pub struct FrameSeq(pub u64);
pub enum HandoffKind { VulkanSurface, Dmabuf, Cpu }
pub struct CpuFrame { pub id: ToplevelId, pub seq: FrameSeq, pub backing_gen: u64, pub width: u16, pub height: u16,
                      pub stride: u32, pub depth: u8, pub pixels: Vec<u8>, pub damage: Vec<Rect> }
pub struct DmabufFrame { pub id: ToplevelId, pub seq: FrameSeq, pub backing_gen: u64, pub fds: Vec<OwnedFd>,
                         pub stride: u32, pub modifier: u64, pub format: u32, pub damage: Vec<Rect> }
pub enum FrameEvent { SetupDone { id }, Ready { id }, Delivered { id, seq }, Dropped { id, seq },
                      Released { id, seq, pixels: Vec<u8> } }

// frames.rs
pub(crate) enum Readiness { Setup, ReadyFirst, Busy, Ready }
pub(crate) struct WindowFrames {                 // one per mapped top-level
    readiness: Readiness, visible: bool, next_seq: u64,
    gen: u64,                                    // backing_gen = backing DrawableId::as_u64 (never recycled)
    last_delivered: Option<FrameSeq>,            // at this gen
    history: DamageHistory,                      // damage of the last POOL frames, by seq
    held: VecDeque<(FrameSeq, u64 /*store epoch*/)>, // snapshots awaiting Delivered
}
impl WindowFrames {
    pub fn on_setup_done(&mut self);
    pub fn can_submit(&self) -> bool;            // ReadyFirst | Ready
    pub fn submitted(&mut self, damage: Vec<Rect>) -> FrameSeq;  // → Busy, history.push
    pub fn on_ready(&mut self);                  // Busy → Ready, or ReadyFirst while !visible
    pub fn on_delivered(&mut self, seq: FrameSeq) -> Delivery;   // { first: bool, ack_epoch: Option<u64> }
    pub fn on_dropped(&mut self, seq: FrameSeq); // stays Busy (§4.5.1 "Delivered/Dropped do not make it ready")
    pub fn new_generation(&mut self, gen: u64);  // resets last_delivered + history: next damage is full
    pub fn frame_damage(&self, n: FrameSeq, full: Rect) -> Vec<Rect>;          // since last delivered (§4.5.2)
    pub fn fill_plan(&self, buffer: Option<(FrameSeq, u64)>, n: FrameSeq) -> FillPlan; // Full | Rects(..)
}
pub(crate) const POOL: usize = 3;   // Xwayland window buffers (xwayland-window-buffers.c:137-158)
```

**Tests** (`rootless/tests/frames.rs`). The expected values are the spec rules, cited per test:
- `setup_then_first_frame_then_ready`: `Setup` refuses to submit, `on_setup_done` gives `ReadyFirst`, a submit gives
  `Busy`, `on_delivered(1)` reports `first: true`, and `on_ready` gives `Ready` (§4.5.1 diagram). Mutation: make
  `Setup` submittable.
- `ready_is_not_implied_by_delivered`: after `on_delivered`, `can_submit()` is false until `on_ready` (§4.5.1).
- `dropped_first_frame_returns_to_ready_first`: submit 1, `on_dropped(1)`, `on_ready` → `ReadyFirst`, still invisible;
  the retry's `frame_damage` is the full extent (§4.5.1 liveness (a): nothing was delivered at this gen). Mutation:
  leave it `Busy` after `Dropped` + `Ready`.
- `damage_since_last_delivered_includes_dropped_frames`: frames 1 (delivered), 2 (dropped, rect A), 3 (rect B) →
  `frame_damage(3) == [A, B]` (§4.5.2). Mutation: compute from the previous *submitted* frame.
- `history_shorter_than_gap_gives_full`: last delivered 1, frames 2-5 dropped, `POOL == 3` → frame 6 damage = full.
- `fill_plan_uses_the_buffers_own_seq`: a buffer released holding seq 4, frame 6 → `Rects(d5 ∪ d6)`; a buffer holding
  seq 1 → `Full` (older than the history); a buffer of another gen → `Full`; a fresh buffer → `Full` (§4.5.2).
  Mutation: copy only the current frame's damage.
- `new_generation_forgets_delivery`: after `new_generation`, `frame_damage` is the full extent (§4.5 resize rule).
- `delivered_acks_the_newest_held_snapshot`: held (1, e10), (2, e14) → `on_delivered(2)` returns `ack_epoch: 14` and
  empties `held`; `on_delivered(1)` alone returns 10 and keeps (2, e14) (§4.5.2 "Order"). Mutation: ack at submit.

**Gate**, then commit `feat(rootless): presenter model (readiness, damage since delivery, buffer pool)`. ~360 + 300
test lines.

### Task 3: `RootlessHost`, the presenter shell, the seam hook, the frame link

**Files:**
- `host.rs`: the trait (spec §4.5, plus `frame_wanted`, D7):

```rust
pub trait RootlessHost {
    /// Asked once at `Mapped`. Dmabuf is answered with Cpu until RL3 (D4).
    fn handoff(&self, id: ToplevelId, depth: u8) -> HandoffKind;
    /// VulkanSurface: called by the presenter on `SetupDone`, on the core thread.
    fn create_surface(&mut self, id: ToplevelId, vk: &VkContext) -> io::Result<vk::SurfaceKHR>;
    fn submit_cpu(&mut self, frame: CpuFrame);
    fn submit_dmabuf(&mut self, frame: DmabufFrame) { drop(frame) }
    /// The first frame was delivered (GPU: presented): make the window visible.
    fn first_frame(&mut self, id: ToplevelId);
    /// Lifecycle, after the presenter applied the batch (so a swapchain is gone before its surface).
    fn on_events(&mut self, events: &[RootlessEvent]);
    /// `id` has a frame waiting for `Ready`: arm the pacing signal (display link, frame callback).
    fn frame_wanted(&mut self, _id: ToplevelId) {}
    /// The presenter destroyed its last swapchain and the VkSurfaceKHR of `id` (added in Task 7):
    /// the host may free the native window/layer now.
    fn surface_released(&mut self, _id: ToplevelId) {}
}
```

- `mod.rs`: `pub(crate) struct Presenter { host: Box<dyn RootlessHost>, windows: HashMap<ToplevelId, Presented> }`,
  where `Presented { host_xid: u32, depth: u8, kind: HandoffKind, frames: WindowFrames, path: Path }` and
  `Path::{Cpu(CpuState), Gpu(Option<WindowSwapchain>)}`. They are filled by Tasks 4 and 6.

```rust
/// Install the presenter and turn the core mode on (rootless::enable).
pub(crate) fn start(state: &mut ServerState, b: &mut XBackend<SurfacePlatform>,
                    host: Box<dyn RootlessHost>, policy: CoordinatePolicy) -> Result<(), RootlessError>;
impl Presenter {
    fn apply_events(&mut self, state: &ServerState, events: &[RootlessEvent]); // Mapped → entry (host_xid from
                                                       // state.resources.window(w).host_xid), Unmapped → teardown
    fn apply_frame_events(&mut self, events: Vec<FrameEvent>);                  // unknown / retired ids dropped
}
```

- `seam.rs`: `fn on_rootless_events(_b: &mut XBackend<Self>, _state: &ServerState, _events: &[RootlessEvent]) {}`.
  `trait_impl.rs` gets `fn on_rootless_events(&mut self, state, events) { P::on_rootless_events(self, state, events) }`.
- `surface/platform.rs`:
  - `SurfaceBackendState.rootless: Option<Presenter>`;
  - `on_rootless_events` → `presenter.apply_events(state, events)`, then `host.on_events(events)`;
  - `on_platform_event` also drains `link.drain_frames()` into `apply_frame_events`.
- `surface/link.rs`: `post_frame(&self, FrameEvent)` and `drain_frames(&self) -> Vec<FrameEvent>`. This is a second queue,
  because `ShellEvent` is `Copy` (:17) and `Released` carries a `Vec`. It reuses `wake()`.
- `rootless/tests/mod.rs`: the `RecordingHost` behind `Arc<Mutex<HostLog>>` records every call (with the `CpuFrame`s
  moved out). Its knobs are `setup: Immediate | Withheld`, `ready: Immediate | Manual`,
  `deliver: Immediate | Manual | DropNext`, `release: Immediate | Never | Manual` and `handoff`. Immediate answers are
  posted to the link, as a real host does from its UI thread. The fixture's `tail(&mut f)` runs `reconcile_and_dispatch`,
  `maybe_composite` and `on_platform_event` in the loop's order (`run.rs:2793`, :3102, :3109).

**Tests** (`rootless/tests/mod.rs` + `readiness.rs`, lavapipe `#[ignore]` via `headless_backend`):
- `rooted_surface_backend_has_no_presenter`: a `RecordingBackend`-free control. Without `start`, `on_rootless_events`
  changes nothing and `pstate.rootless.is_none()`. Mutation: install the presenter in `surface_backend`.
- `host_sees_the_batch_after_the_presenter`: map W → the host log shows `on_events([Mapped])` and the presenter holds
  W's entry with `host_xid` = W. On unmap, the entry is gone before `on_events([Unmapped])` is recorded. Mutation:
  forward first.
- `frame_events_for_a_retired_id_are_ignored`: map, unmap, remap (id1 → id2). `SetupDone{id1}` posted late changes
  nothing for id2 (spec §4.2: an id names one mapped lifetime). Mutation: key entries by XID.

**Gate**, then commit `feat(rootless): RootlessHost, presenter table, seam hook and frame link`. ~330 + 260 test
lines.

### Task 4: CPU path (asynchronous readback, canonical image, pool, ack on delivery)

**Files:** `rootless/cpu.rs`, `mod.rs` (`Presenter::tick`), `surface/platform.rs` (`maybe_composite` calls
`rootless_tick` after `compose_tick`, :960).

```rust
pub(crate) struct CpuState {
    canonical: Option<Canonical>,         // { gen, width, height, stride, pixels } full extent (§4.5.2)
    pool: Vec<PoolBuffer>,                // ≤ POOL; { seq, gen, pixels: Option<Vec<u8>> } (None = with the host)
    in_flight: Option<InFlight>,          // { gen, full: bool, snapshot: Option<DamageSnapshot>, readback }
    needs_full: bool,                     // new window or new gen (§4.5 "first frame is full-extent")
}
impl XBackend<SurfacePlatform> { pub(crate) fn rootless_tick(&mut self); }
```

The tick does this per CPU window:
1. Resolve the backing: `core.host_window_to_backing[host_xid]` → `store.lookup` → `DrawableId`. A gen different from
   `frames.gen` → `new_generation`, `needs_full`, and the canonical image is dropped.
2. In flight and `poll()` true:
   - a stale gen → discard, never ack, set `needs_full`;
   - else `write_into` the canonical image (on the first one, allocate it at the backing extent);
   - assign the seq, then build a `CpuFrame`: pick a released buffer of this gen, else allocate while `pool.len() <
     POOL`, then fill it per `fill_plan`, set `damage = frame_damage`, record the snapshot epoch in `held`, call
     `submit_cpu` and go `Busy`.
3. Not in flight, `can_submit()`, a pool buffer is free or allocatable, and (`needs_full` or the store snapshot is
   non-empty): call `flush_before_damage_exposure` once per tick (compose.rs:182), `peek_presentation_damage`, then
   `begin_readback` of either the full extent or the snapshot's rects clamped to the extent.

`Delivered(n)` acks the store with the newest held snapshot up to n (`ack_presentation_damage`, store.rs:1373) and
calls `first_frame` on the first one. `Dropped` acks nothing. `Released(n, pixels)` returns the buffer with its seq. A
teardown moves an in-flight readback to `retiring` (dropped once `poll` is true), so the staging buffer never dies under
the GPU.

**Tests** (`rootless/tests/cpu.rs`, lavapipe, host `Immediate` everywhere unless stated; "full readback" =
`engine.get_image` of the backing at the same moment):
- `first_frame_is_full_extent_and_equals_the_backing`: W 40×30 bw 2, PolyFillRectangle on W, then tails until a frame
  arrives (bounded, 2 s). `CpuFrame` 44×34, `damage == [(0,0,44,34)]`, pixels == the full readback. Source: §4.0
  (native content = outer extent), §4.5 (first frame full). Mutation: use store damage for the first frame.
- `child_damage_arrives_in_backing_pixels`: T bw 3 with child C at (5,6) 10×8. After the first frame, fill C at
  (0,0,10,8) → the next frame's `damage == [(8,9,10,8)]` = (bw+5, bw+6). Source: §4.0 (backing (0,0) = outer corner,
  content at (bw,bw)); X child coordinates are relative to the parent's content origin. Mutation: offset by the content
  origin twice.
- `snapshot_acked_only_on_delivered`: `deliver: DropNext`. A fill of rect A is in frame 1, which is dropped, and the store
  still has A. After `Ready` + `Delivered(2)`, frame 2's `damage ⊇ A` and the store is empty. Mutation: ack at
  submit; ack on `Dropped`.
- `resize_during_readback_discards_and_repaints_full`: `ready: Manual`. Start a readback, then ConfigureWindow
  60×40 before the poll. No frame of the old gen reaches the host, and the next frame is 64×44 (bw 2) and full with a new
  `backing_gen`. Mutation: deliver a stale-generation frame.
- `host_drops_a_resized_frame_then_retries_full`: the host answers `Dropped` + `Ready` → the retry has the new gen and
  full damage (§4.5.1 liveness (b)).
- `frames_outlive_the_call`: the host moves each `CpuFrame` to a spawned thread, which keeps it until the test joins
  and then compares its bytes with the full readback taken at that frame's seq. Mutation: let the pool reuse a buffer
  the host has not released (the held frame's bytes change).
- `pool_buffers_released_out_of_order_stay_canonical`: `release: Manual`. Run five frames with disjoint fills; release
  buffers in the order 2, 1, 3. Every delivered frame equals the full readback at its seq, and a buffer whose seq fell
  out of the history gets a full copy (§4.5.2). Mutation: fill a recycled buffer with only the current damage.
- `submissions_are_in_seq_order`: seqs reach `submit_cpu` strictly increasing (§4.5.2 "Order"). Mutation: allow two
  readbacks in flight.
- `cpu_path_never_waits_on_the_core_thread`: `core_thread_fence_waits()` is unchanged across 20 tails with frames
  (spec §10). Mutation: `get_image` instead of `begin_readback`.

**Gate**, then commit `feat(rootless): asynchronous CPU frames with a canonical image and buffer pool`. ~380 + 520 test
lines.

### Task 5: Readiness gating and the `next_wakeup` contribution

**Files:** `mod.rs` (`Presenter::next_wakeup(&self, store, now) -> Option<Instant>`), `surface/platform.rs:857-877`
(chain it into `next_wakeup`).

The tick and `next_wakeup` share one predicate, `wants_frame(w)`: `can_submit` and no readback in flight and a pool
buffer is available and (`needs_full` or the store snapshot is non-empty). This is the rule `scene_wants_scene`
documents (compose.rs:12-21). If the two disagreed, the loop would busy-spin or strand paint. The contributions are:
- some `wants_frame` → `now`;
- some readback in flight → `now + 1 ms` (the polled-Present convention, compose.rs:221-222; D6);
- a window with damage that is `Busy` → none, and the presenter calls `host.frame_wanted(id)` once per Busy period (D7);
- otherwise `None`.

**Tests** (`rootless/tests/readiness.rs`, lavapipe):
- `withheld_setup_gets_no_frame_and_no_wakeup`: `setup: Withheld`, map + fill → no `submit_cpu`, no readback, and
  `next_wakeup() == None` (§4.5.1; §4.1 "damage while the host is not ready → no deadline"). Mutation: treat `Setup` as
  ready.
- `setup_done_alone_starts_the_first_frame_in_the_same_tail`: `ready: Manual`. Post `SetupDone`; one `tail` → a
  readback is in flight (CPU) before any `Ready` exists. `first_frame` is recorded only after the host's
  `Delivered` (§4.5.1). Mutation: gate the first frame on `Ready`.
- `later_frames_wait_for_ready`: after frame 1 is delivered, a fill → no readback until `Ready` is posted, and
  `frame_wanted(id)` is recorded once.
- `next_wakeup_rules`: Ready + damage → `≤ now`; in flight → `now + 1 ms`, and `None` again once it completed and the
  host is `Busy`; nothing pending → `None` (spec §10). Mutation: keep the 1 ms deadline after completion.
- `tick_and_wakeup_agree`: after any tail, if `next_wakeup() <= now` then the next `rootless_tick` starts or completes
  something (a counter on the presenter). Run with the pool exhausted (`release: Never`, 3 frames out) → `None`, no
  spin. Mutation: drop the pool condition from `wants_frame`.
- `rooted_next_wakeup_unchanged`: without `start`, the surface `next_wakeup` equals today's value on the same fixture.

**Gate**, then commit `feat(rootless): readiness gating and next_wakeup for frames`. ~120 + 260 test lines.

### Task 6: Extract the swapchain core from `SurfaceOutput` (no behaviour change)

**Files:** `surface/swapchain.rs` → `surface/swapchain/{mod.rs, chain.rs}`. `Chain` owns what the rooted output and a
rootless window share:
- surface functions, the `Swapchain` (handle, images, extent, ready semaphores);
- `rebuild(extent, alpha: AlphaMode)` (:625-755);
- `destroy` (:770);
- acquire (:345-367), with the `NOT_READY`/`TIMEOUT`/`OUT_OF_DATE` outcomes as an enum;
- the present call (:422-439).

`AlphaMode::{Opaque, Premultiplied}`. `SurfaceOutput` passes `Opaque` and keeps today's order exactly
(OPAQUE, INHERIT, PRE_MULTIPLIED, POST_MULTIPLIED, :685-693). `pick_alpha(supported, AlphaMode)` is a pure function.

**Tests:** every existing `surface/tests.rs` swapchain test stays green, unedited; that is the gate for "no behaviour
change". New pure tests (`chain.rs`):
- `opaque_mode_keeps_todays_order`: supported = {INHERIT, OPAQUE} → OPAQUE.
- `premultiplied_mode_prefers_premultiplied`: supported = {OPAQUE, PRE_MULTIPLIED} → PRE_MULTIPLIED; only {OPAQUE} →
  OPAQUE. Source: §4.5 (X ARGB visuals are premultiplied).

**Gate**, then commit `refactor(surface): share the swapchain core`. ~−200 / +250 lines, 40 test lines.

### Task 7a: Present fences where available, validation errors countable

Two lifetime gaps in the first draft of Task 7: a copy fence does not show that **presentation** has finished with a
swapchain or its semaphores (Vulkan guide, "Swapchain semaphore reuse"), and the GPU copy did not keep the backing
alive. This task adds the device support for the first gap and the test instrument for both.

Facts (fb83716b, and measured on this box):
- No code enables a swapchain maintenance extension (`grep maintenance1` finds nothing). `wants_present` only requires
  `VK_KHR_swapchain` (`yserver-render/.../vk/device.rs:591`).
- Lavapipe here exposes `VK_EXT_swapchain_maintenance1`, `VK_EXT_surface_maintenance1`, `VK_KHR_surface_maintenance1`
  and `VK_EXT_headless_surface`. It does not list `VK_KHR_swapchain_maintenance1` (`vulkaninfo` with the lvp ICD,
  2026-10-19). KosmicKrisp is **unverified** [A]: RL2 runs `vulkaninfo` on m4/eiger before it relies on present fences.
- Validation runs in debug builds whenever `VK_LAYER_KHRONOS_validation` is installed (`device.rs:472-476`). The
  layer is installed here, so the lavapipe gate (a debug `cargo test`) already runs under validation. Errors are only
  logged (`vk_debug_callback`, `device.rs:1666-1692`); nothing can assert on them.

**Files:**
- `device.rs`, for a `wants_present` context:
  - enable `VK_KHR_swapchain_maintenance1`, else `VK_EXT_swapchain_maintenance1`, when the device has it;
  - enable the instance extensions it requires (`VK_KHR`/`VK_EXT_surface_maintenance1`,
    `VK_KHR_get_surface_capabilities2`) and the `swapchainMaintenance1` feature;
  - record the result as `pub swapchain_maintenance1: bool` on `VkContext`.
  A device without it is not an error.
- `device.rs`: the debug messenger's `user_data` points at an `Arc<AtomicU64>` owned by the `VkContext`, which counts
  messages of severity ERROR. Add `pub fn validation_enabled(&self) -> bool` and `pub fn validation_errors(&self) ->
  u64`. The count is per context, so parallel tests (each with its own instance) do not see each other's errors.

**Tests** (`yserver-render`, lavapipe `#[ignore]`):
- `presenting_context_enables_swapchain_maintenance1_when_offered`: on a headless-surface context, the flag equals
  "the device lists either extension". On lavapipe it is true (measured above). Mutation: never enable it.
- `validation_errors_are_counted_per_context`: with validation enabled (else the test prints why and returns), a
  deliberately invalid call that the layer checks at call time (`vkCreateImage` with a zero extent,
  VUID-VkImageCreateInfo-extent-00944; on the unexpected success the image is destroyed) bumps this context's count,
  and a second context's count stays 0. Mutation: one global counter.

**Gate**, then commit `feat(render): enable swapchain maintenance1 and count validation errors per context`. ~90 + 80
test lines.

### Task 7: GPU path (per-window swapchain, `VulkanSurface`)

**Files:** `surface/swapchain/chain.rs` (present fences, retirement), `rootless/window_swapchain.rs`, `mod.rs`
(`Path::Gpu`, the retiring list), `host.rs` (one hook).

```rust
pub(crate) struct WindowSwapchain {
    chain: Chain, gen: u64, extent: vk::Extent2D, cbs: Vec<vk::CommandBuffer>, pool: vk::CommandPool,
    syncs: Vec<FrameSync>,          // { acquire: vk::Semaphore, copy: Option<FenceTicket>, present: Option<vk::Fence> }
    next_sync: usize,
}
impl WindowSwapchain {
    pub fn new(vk: &Arc<VkContext>, surface: vk::SurfaceKHR, extent: vk::Extent2D, depth: u8) -> Result<Self, vk::Result>;
    /// Acquire (non-blocking), copy the whole backing via begin/end_external_read, present.
    pub fn present(&mut self, gpu: &GpuCore, store: &mut DrawableStore, backing: DrawableId, gen: u64)
        -> Result<Presented, vk::Result>;
    /// Hand everything to retirement; never destroys in place.
    pub fn retire(self) -> Retiring;
}
pub(crate) enum Presented { Shown, NotReady, OutOfDate }
pub(crate) struct Retiring { /* chains (current + old), syncs, surface, id */ }
impl Retiring { pub fn try_destroy(&mut self, vk: &VkContext) -> bool; }   // true = gone
```

**Backing lifetime during the copy.** The copy CB is submitted with a `FenceTicket` from `GpuCore::acquire_fence_ticket`
(`gpu/fence.rs:432`) as its fence. `store.touch_render_fence(backing, ticket.clone())` (`store.rs:1454`) follows, as
`get_image` does (`put_get.rs:350`) and as Task 1's `begin_readback` does. The store retires a drawable only once its
`last_render_ticket` has signalled (`retire` decision `store.rs:1144`, `poll_pending_retire` :1475). So a backing freed
by a resize (`reallocate_redirected_backing`) or an unmap/destroy while the copy runs outlives the copy. This relies on
the existing same-queue ordering convention for overwritten tickets (`frame.rs:1302-1306`). The same ticket is the sync
slot's copy marker: a slot, its CB and its acquire semaphore are reused only after `poll_signaled`.

**Semaphores** (Vulkan guide, "Swapchain semaphore reuse"):
- Acquire semaphores rotate per sync slot (3, as `SYNC_COUNT`). A slot is reused only after the copy that waited on its
  semaphore has completed (its ticket signalled). That is what `SurfaceOutput` does today (`swapchain.rs:341`) and
  what the guide requires for acquire semaphores.
- If an acquire succeeded but the copy submit failed, the semaphore is signalled and never waited on. That slot's
  semaphore is destroyed and recreated, never reused.
- Present-wait ("ready") semaphores are **per swapchain image**, indexed by the acquired image index, as
  `SurfaceOutput` does (`swapchain.rs:372`). Per the guide, a present semaphore may be reused once the same image has
  been acquired again, which per-image indexing guarantees. With maintenance1, the present fence below gives the same
  guarantee explicitly.

**Presentation completion, and destroying a swapchain:**
- **With maintenance1:**
  - every `vkQueuePresentKHR` chains `VkSwapchainPresentFenceInfoEXT` with the slot's present fence (reset before
    reuse, reused only once signalled);
  - a swapchain, its per-image semaphores and its slots' objects may be destroyed once every present fence issued on it
    and every copy ticket has signalled;
  - images acquired but never presented (copy submit failed) are released with `vkReleaseSwapchainImagesEXT`.
- **Without maintenance1** (KosmicKrisp if [A] fails): `try_destroy` calls `vkQueueWaitIdle(graphics_queue)` once, then
  destroys. This is a bounded core-thread wait, only at unmap and resize. It is the guide's fallback, and the only way
  without the extension to know a present has finished with its semaphore.
- **Never in place:**
  - `Unmapped` moves the window's swapchain into `Presenter.retiring` before `host.on_events` (Task 3 order).
  - A gen change (resize) or `OutOfDate` builds the new chain with `oldSwapchain` = the current one and moves the old
    one to `retiring`. It is not destroyed right after creation, as `SurfaceOutput::rebuild_swapchain` does today
    (`swapchain.rs:709-710`): that is the same hazard, fixed for the rooted path in Task 7b.
  - `retiring` is polled in the tick and in `before_block`. Polling is a fence status query, not a wait. `next_wakeup`
    adds the 1 ms poll deadline while `retiring` is non-empty (D6 convention).
- **The surface outlives its swapchains.** The presenter owns the `VkSurfaceKHR` from `create_surface` and destroys it
  only after the last chain on it is gone. Then it calls a new hook, `RootlessHost::surface_released(&mut self, id:
  ToplevelId)` (default no-op), and only then may the host free the native layer/window. The host hides the window at
  `Unmapped` (user-visible at once) and frees it at `surface_released`. This is an interface addition for RL2 (D7's
  sibling).
- `Shown` / `NotReady` / `OutOfDate` handling and the ack-after-present rule are unchanged: `Shown` = delivery (ack,
  `first_frame` on the first one, `Busy`); `NotReady` → `Busy` + `frame_wanted`.
- A gen change rebuilds at the new extent. The copy is **full extent** (D5). Depth 32 → `Premultiplied`.

**Tests** (`rootless/tests/gpu.rs`, lavapipe). The host's `create_surface` returns a `VK_EXT_headless_surface`
(`NativeSurface::headless`, swapchain.rs:79), and the context comes from `NativeSurface::create_context` (tests.rs:271).
Every test ends by asserting `vk.validation_errors() == 0` when `vk.validation_enabled()` (Task 7a); without the layer it
prints that validation was off.
- `setup_done_presents_the_first_frame_in_the_same_tail`: `SetupDone` + one tail → one present, and `first_frame` in
  that tail (§4.5.1 GPU row). Mutation: wait for `Ready`.
- `presented_image_holds_the_backing`: the last presented image (Chain helper modelled on `copy_on_screen_into`,
  swapchain.rs:581) equals the full readback of the backing. Mutation: copy from the window's leaf.
- `gpu_ack_after_present`: the store snapshot is non-empty before the tail and empty after `Shown`. Mutation: ack before
  acquire.
- `gpu_copy_keeps_the_backing_alive`:
  - present, then in the same iteration ConfigureWindow W to a new size (the old backing is freed by
    `reallocate_redirected_backing`, `portable/redirect.rs:986-989`), then `reclaim_retired_gpu_work`;
  - deterministic part: right after `present`, the old backing's `last_render_ticket` is the copy's ticket
    (`Rc::ptr_eq`);
  - the old `DrawableId` stays in the store until that ticket polls signalled, and is gone after;
  - zero validation errors;
  - variant: UnmapWindow + DestroyWindow instead of the resize.

  Mutation: skip `touch_render_fence` (the ticket check fails, and validation reports the destroyed image in use where
  the copy is still running).
- `unmap_retires_instead_of_destroying`: unmap right after a present → the chain is in `retiring`, and
  `surface_released(id)` is recorded only at a later tail, after the present fences signalled (lavapipe has
  maintenance1). The host log has `on_events([Unmapped])` before `surface_released`. Mutation: destroy in place.
- `fallback_without_maintenance1_waits_queue_idle_once`: the context's `swapchain_maintenance1` forced false (a
  `#[cfg(test)]` override on the presenter, not on the device) → `try_destroy` succeeds in the same tail after one
  queue-idle, and the validation count is 0.
- `rapid_resizes_retire_old_swapchains_cleanly`: eight resizes with a present between each → every old chain passes
  through `retiring` and is gone after the fences signalled, and validation is clean (it reports semaphore reuse while
  pending, VUID-vkAcquireNextImageKHR-semaphore-01779, and destroying a swapchain in use). Mutation: destroy the old
  swapchain right after creating the new one (today's `SurfaceOutput` order).
- `failed_copy_submit_does_not_reuse_the_acquire_semaphore`: inject one submit failure (test hook) → the slot's
  semaphore is recreated and 20 later frames present with validation clean.

**Gate** (step 5 runs under validation here; the test output states whether the layer was on), then commit
`feat(rootless): GPU frames through a per-window swapchain`. ~420 + 380 test lines.

### Task 7b: `SurfaceOutput` retires old swapchains the same way (rooted fix)

`SurfaceOutput::rebuild_swapchain` destroys the old swapchain right after creating the new one
(`swapchain.rs:709-710`), while a present on it may still hold its semaphores. That is the gap Task 7 closes for rootless.
Move the rooted output onto `Chain`'s retirement: present fences with maintenance1, queue-idle otherwise. `Drop` keeps
`device_wait_idle` (`swapchain.rs:960`). This is a rooted behaviour change, so jos may take it as its own PR.

**Tests:** the existing surface swapchain tests unedited; plus `surface_resize_storm_is_validation_clean`: eight window
resize events with a frame between each on the headless surface → zero validation errors. Mutation: the old destroy
order.

**Gate**, then commit `fix(surface): retire old swapchains after their presents complete`. ~40 + 50 test lines.

### Task 8: Layout rule, status, spec deltas

- `tools/check-layout.sh` rule 4: add `crates/yserver/src/rootless` to the window-system grep. Rootless is OS-neutral
  like `surface`.
- `docs/status.md`: one entry covering RL1's scope, what stays test-only, and the decisions below.
- Spec deltas (one docs commit, decided by jos): §4.6 (presenter in `SurfaceBackendState`, one seam hook, no
  `rootless_host()` accessor), §4.5 (`frame_wanted`; full-extent GPU copy in RL1; Dmabuf deferred), §7/§9 (no headless
  CLI in RL1).

**Gate**, then commit `docs(rootless): RL1 status, layout rule and spec deltas`. ~10 + docs.

## Follow-up and optional tasks (D3, D4)

- **9 — Linux headless + `--rootless` + vng (D3, the promised follow-up before RL2 hardware results).**
  - Build `surface` and `rootless` on every target (`lib.rs:22-25`), and honour `--headless` on KMS builds (today it is
    ignored, `lib.rs:318-320`).
  - `--rootless` with `--headless` installs a `RecordingHost` that logs events and writes `<dir>/<id>-<seq>.bgra` plus a
    header line per delivered frame to `--rootless-record DIR`.
  - `tools/vng-shot.sh` gains a `--server-arg`.
  - `tools/vng-scenarios/rootless-headless.sh` starts xterm, xeyes (shaped) and an override-redirect `xmessage`. It
    asserts the event log, and that each frame cropped by `(bw, bw)` equals `xwd -id` (python compare, as
    `pixel-oracle-check.py` does). The border width is read from `xwininfo`, never assumed.
  - ~350 lines.
- **10 — Dmabuf (D4).** Promote the backing exportable at its first frame, export with `export_promoted`
  (dri3.rs:443), carry a sync-file per frame, and test on lavapipe if it exposes `VK_KHR_external_memory_fd` [A].
  ~250 lines.

## Testing strategy

- **Pure model** (Task 2) in the normal suite: readiness, damage since delivery, pool fill plans, held-snapshot acks.
- **Lavapipe** (Tasks 1, 3-5, 7): a real `headless_backend`/headless-surface backend with rootless started through the
  same `rootless::start`, requests through `process_request`, and the loop's order through `tail()`. Pixel oracle =
  `get_image` of the backing.
- **Spec §10 RL1 list, mapped:**

  | Spec §10 case | Test |
  |---|---|
  | `next_wakeup` cases | `next_wakeup_rules`, `withheld_setup_gets_no_frame_and_no_wakeup` |
  | child damage in backing pixels | `child_damage_arrives_in_backing_pixels` |
  | no core-thread fence wait | `cpu_path_never_waits_on_the_core_thread` |
  | ack only on `Delivered` | `snapshot_acked_only_on_delivered` |
  | stale generation discarded | `resize_during_readback_discards_and_repaints_full` |
  | `CpuFrame` outlives the call | `frames_outlive_the_call` |
  | readiness | Task 5 tests |
  | liveness (a), (b) | `dropped_first_frame_returns_to_ready_first`, `host_drops_a_resized_frame_then_retries_full` |
  | canonical image | `pool_buffers_released_out_of_order_stay_canonical` |
  | seq order | `submissions_are_in_seq_order` |
- **Rooted guard:** `rooted_surface_backend_has_no_presenter`, `rooted_next_wakeup_unchanged`, the unedited surface
  suite (Task 6), and the full workspace suite.
- **Not in RL1:** the vng scenario (D3, follow-up before RL2 hardware results) and hardware (RL2).

## Risks

- **Damage completeness** (spec §11). The presenter trusts store damage on the backing. A paint path that writes a
  backing without `store.damage` shows as a stale native window. The first-frame-full rule hides it at map; the
  child-damage test covers the common route. Real toolkits are only exercised by the optional vng task or RL2.
- **The scene must stay a non-consumer.** If any rooted path acks backing damage with zero outputs, frames go stale.
  Today it cannot (`compose.rs:39-40`); a future change that composes with zero outputs would break rootless.
  `snapshot_acked_only_on_delivered` catches it.
- **Swapchain teardown.** With maintenance1, retirement is non-blocking (fence polls). Without it, each retirement
  costs one `vkQueueWaitIdle` on the core thread, at unmap and at resize. If KosmicKrisp lacks maintenance1 [A],
  menus pay that wait at menu speed (spec §11). RL2 checks `vulkaninfo` first and measures. The host must keep its
  native layer until `surface_released`, so an unmapped window is hidden at once but freed later.
- **Overwritten render tickets.** The backing's `last_render_ticket` keeps only the newest touch. A later paint's ticket
  covering the copy relies on the same-queue ordering convention the engine already uses (`frame.rs:1302-1306`). It is
  not a new assumption, but the GPU copy now depends on it too.
- **Validation coverage depends on the box.** The lavapipe gate runs under validation only where the layer is
  installed (it is here, `device.rs:472-476`). CI may not have it. The GPU tests print whether validation was on.
- **Memory** of the CPU path: canonical + up to three pool buffers + staging ≈ 5× the window's bytes. That is acceptable
  for the Wayland shm / Windows consumers; macOS uses the GPU path.
- **Full-extent GPU copy** per frame costs bandwidth on large windows (D5).
- **1 ms polling** while a readback is in flight costs wakeups during animations (D6).
- **macOS dead-code lints:** the module compiles there unused until RL2. The `cfg_attr` covers it, but the local gate
  cannot prove it; CI's macOS clippy does.
- **Refactor of `SurfaceOutput`** (Task 6) touches the rooted macOS present path; the unedited lavapipe swapchain tests
  are the guard.

## Decisions (decided by jos as recommended, after the codex review)

1. **D1 — presenter placement.** In `SurfaceBackendState`, with one seam hook `on_rootless_events`, instead of the spec's
   `Platform::rootless_host()` accessor. Reasons:
   - rootless is surface-only (§7);
   - the presenter needs `store`/`engine`/`gpu` and the host at once;
   - the surface module is not built on Linux release, which keeps all of it out of the KMS binary.

   Recommended.
2. **D2 — CLI.** Test-only in RL1, no `--rootless` flag; the flag arrives with RL2 (macOS), where it is real. A headless
   rootless CLI would first need `surface` built on Linux (`lib.rs:22-25`) and `--headless` made real on KMS builds,
   which is its own change. Recommended: test-only.
3. **D3 — vng scenario: the promised follow-up, done BEFORE we rely on RL2 hardware results.** It needs D2's Linux
   headless work (Task 9, ~350 lines) and lands right after RL1, as its own commits. It is the only check with real
   toolkits (xterm, xeyes, an override-redirect window) before macOS. RL2 hardware findings are not trusted as
   attribution until this scenario is green.
4. **D4 — dmabuf.** RL1 has the types only, and `handoff == Dmabuf` falls back to Cpu with one warning. The
   implementation (optional Task 10) waits for RL3, which has the only consumer. Reasons:
   - promotion blocks on a copy (`export.rs:27`);
   - per-frame sync needs its own design (#100);
   - lavapipe export support is unverified.

   Recommended.
5. **D5 — GPU copy is full extent in RL1.** A damage-limited copy needs swapchain images to keep their content between
   presents, which Vulkan does not promise and a CAMetalLayer drawable may not do [A]. Measure on KosmicKrisp in RL2,
   then reuse `DamageHistory` per image.
6. **D6 — readback completion by a 1 ms poll** only while a readback is in flight. A sync-file fd in `poll_fds` (spec
   §4.1) helps Linux only and comes with RL3.
7. **D7 — `RootlessHost::frame_wanted`** (spec Δ). Without it, a host whose pacing signal is armed on demand (macOS
   display link `arm_tick`, `surface/swapchain.rs:307`) never sends `Ready` for a frame it does not know is waiting.
   Recommended.
8. **D8 — RL1b root reads: separate**, as spec decision 8 already places it. It is independent of RL2 (it reads
   backings, not hosts), so it can run alongside RL2. It needs a non-output compose of backings over the root
   background (generalising `scene/root_readback.rs:58`), which is scene work with its own review. Recommended:
   separate.
9. **Branch:** open: continue on `feat/rootless-rl0` (one PR at RL2), or cut `feat/rootless-rl1` from it.
10. **Task 7b** is a rooted fix (`SurfaceOutput` destroys an old swapchain right after creating the new one,
    `swapchain.rs:709-710`). Open: keep it in RL1, or take it as its own PR.

## What RL2 (macOS) needs from this interface

- `MacRootlessHost: RootlessHost`, owned by the presenter.
  - `handoff` → `VulkanSurface` for every window.
  - `create_surface` → `vkCreateMetalSurfaceEXT` on the window's `CAMetalLayer`, generalising `NativeSurface`'s
    creator closure. Called on the core thread after the host posted `SetupDone` from the main queue: NSWindow ordered
    in with `alphaValue = 0`, layer attached [A].
  - `first_frame` → `alphaValue = 1` on the main queue.
  - `frame_wanted` → arm that screen's display link, which posts `Ready`.
  - `surface_released` → free the NSWindow/CAMetalLayer. At `Unmapped` the host only orders the window out.
- Check on m4/eiger whether KosmicKrisp exposes `VK_KHR`/`VK_EXT_swapchain_maintenance1` (Task 7a [A]). Without it,
  every unmap/resize pays a queue-idle (Risks).
- The `Chain` takes a copy scale (Task 6 keeps the slot-copy scale code shared) so RL2 can NEAREST-blit at S ≠ the
  screen scale (decision 6). The layer's `drawableSize` = backing extent × scale is the host's job.
- `run_windowed` gains a rootless mode with **no** `SurfaceOutput`, calling `rootless::start` with `CoordinatePolicy::
  Global`. `output_layouts()` stays empty, so the scene is inert. `--rootless/--rooted` parse there.
- Per-output RandR and multi-screen `ScreenLayout` (RL0 D7). The presenter has no output notion, so nothing here
  blocks it.
- Measurements RL2 owes the interface: swapchain create/destroy latency per popup, the teardown wait (Risks), drawable
  content preservation (D5).
- Nothing in RL1 assumes Linux, fds or a CPU path for macOS. `DmabufFrame` holds `OwnedFd`, which is Unix-only. Before
  Windows (RL4) it becomes `#[cfg(unix)]` or a platform handle type; that is noted, not done.

## Size

| Task | Code | Tests |
|---|---|---|
| 1 render: async readback, external read, wait counter | 200 | 150 |
| 2 pure model | 360 | 300 |
| 3 host trait, presenter shell, seam hook, link | 330 | 260 |
| 4 CPU path | 380 | 520 |
| 5 readiness + `next_wakeup` | 120 | 260 |
| 6 swapchain core extraction | +250 / −200 | 40 |
| 7a maintenance1 + per-context validation count | 90 | 80 |
| 7 GPU path (present fences, retirement, backing ticket) | 420 | 380 |
| 7b `SurfaceOutput` retirement (rooted fix) | 40 | 50 |
| 8 layout rule, status, spec deltas | 10 | — |
| **Total** | **~2,000** | **~2,040** |

Follow-up: Task 9 (vng, D3) ~350. Optional: Task 10 (dmabuf, RL3) ~250.

Order of commits: 1 → 2 → 3 → 4 → 5 → 6 → 7a → 7 → 7b → 8, then the follow-up 9. Each commit's gate is green on its own.
- 4 needs 1 (`begin_readback`), 2 and 3.
- 5 needs 4 (an in-flight readback to time).
- 7 needs 1 (`begin/end_external_read`), 3, 6 and 7a; 7b needs 7's `Chain` retirement.
- 6 is independent of 2-5 and may move earlier.
