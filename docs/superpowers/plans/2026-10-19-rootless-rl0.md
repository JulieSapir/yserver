# Rootless RL0 (core module) — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** milestone RL0 of `specs/2026-10-19-rootless-core-design.md` (r3, §9). It adds `yserver-core::rootless`:
- server-owned Manual redirect of every root child;
- the topology reconcile and its `RootlessEvent` stream, with `ToplevelId`;
- the ICCCM/EWMH `WindowInfo` parser;
- the built-in WM policy;
- input mapping (pointer scope, local coordinates, activation);
- host serial acknowledgment.

All of it is unit-tested on `ServerState` + `RecordingBackend`, with a recording host double.

**Not in RL0:**
- pixels: the presenter, `HandoffKind`, `FrameEvent`, `CpuFrame`, the `RootlessHost` trait and `Platform::rootless_host`
  are RL1;
- root reads are RL1b;
- the macOS host and `--rootless/--rooted` are RL2;
- Wayland is RL3.

**Architecture:** rootless is a core mode, `ServerState.rootless: Option<Rootless>`. It is `None` unless an embedder
calls `rootless::enable`. Every new hook starts with `let Some(r) = state.rootless … else return`, so KMS, rooted
macOS and headless keep their current behaviour, call for call.

**Branch:** `feat/rootless-rl0`, cut from master after `plan/rootless-core` (spec + this plan) and P1 land. Plan
written against `348161e0`.

**Status:** reviewed by codex; jos's decisions D1-D8 applied (see "Decisions" at the end).

## Facts checked against the tree (348161e0)

The spec cites `cb8297d0`. These anchors were re-read on `348161e0`. Where the tree differs from the spec, that is
marked **Δ**.

| What | Where |
|---|---|
| Loop head deadline; `backend.next_wakeup()` | `core_loop/run.rs:2208-2251`, :2223 |
| `before_block`, then `poll` | `run.rs:2260`, :2268 |
| `settle_client_output` → `run_iteration_tail` → `settle_client_output` | `run.rs:2934`, :2943, :2944-2951 |
| Tail body; `drain_present_completions`; `maybe_composite`; idle vblanks | `run.rs:3078-3109`; :3097; :3104; :3108 |
| `Message::PlatformEvent` dispatch; `HostInput` dispatch | `run.rs:2793`; `run.rs:2560` → `dispatch_host_input` :1456 → `handle_host_input` :4299 → `backend.on_host_input` :4301 |
| Loop-order test to extend | `core_loop/run/tests.rs:3851` (`run_iteration_tail_drains_present_work_before_compositing`) |
| `Backend` trait; `next_wakeup`; `maybe_composite`; `on_platform_event` | `backend/trait_def.rs:631`; :1029; :1043; :1049 |
| `on_window_property_changed`; `on_window_became_top_level`; `sync_top_level_order` | `trait_def.rs:1081`; :1119; :1132 |
| `allocate_redirected_backing` | `trait_def.rs:1436` |
| `RecordedCall`; `MaybeComposite`; `impl Backend for RecordingBackend`; `with_redirect_activation` | `backend/recording.rs:36`; :251; :831; :776 |
| `RecordingBackend::on_host_input` records keys only (**Δ** for input tests) | `recording.rs:1272-1290` |
| Redirect bookkeeping | `composite_redirects.rs`: `ManualTaken` :28; `redirect_window` :119-138; `redirect_subwindows(parent, children, record)` :158 (**Δ** the spec's call omits `children`); `redirect_new_subwindow` :207; `reparent_subwindow` :216; `forget_windows` :236; `forget_client` :246 |
| The Composite request path that rootless startup copies | `process_request/composite_damage.rs:96-122` (COW filter, child list), :139-151 (`ManualTaken` → `BadAccess`, :163-171), :177-195 (`sync_redirect_backing` per target) |
| `sync_redirect_backing` → `activate_redirect_backing_for` | `process_request/redirect.rs:295-317` → :47 |
| New child of a redirected parent | `process_request/windows.rs:1277-1282` (CreateWindow) |
| Reparent in/out of root | `windows.rs:849-869` (`reparent_subwindow` + `sync_redirect_backing`) |
| Destroy | `windows.rs:476-487` (`release_redirects_on_destroyed_windows`, `forget_windows`) |
| `forget_client` callers: only the disconnecting client | `process_disconnect.rs:442`. Client ids start at 1 (`poll_tokens.rs:129`), so `SERVER_OWNER = ClientId(0)` (`resources/mod.rs:25`) never loses its records. The allocator wraps to 0 after `u32::MAX` ids (`poll_tokens.rs:133-137`): pre-existing, theoretical |
| Server-reserved XIDs `0x100`-`0x106` (`0x107` is free) | `resources/mod.rs:27-49` |
| Server windows today: synthesized LE bodies into `handle_create_window`/`handle_map_window` as `SERVER_OWNER` | `saver_dpms.rs:278-321` |
| Property hook sites | `props.rs:1102` (Change), :1237 (GetProperty delete), :1273 (Delete). The only other writer is `xkb_layout.rs:90` (root only) |
| **Δ Property data is stored in the client's byte order.** ChangeProperty data is an `OpaqueTail` (`yserver-protocol/src/x11/request_swap.rs:561-574`), and GetProperty writes it back raw (`x11/mod.rs:1911-1940`). Xorg swaps per format on the way in (`dix/swapreq.c:209-229`, `SwapRestS`/`SwapRestL`) and on the way out (`dix/property.c:540-549`). Internal LE readers already assume LE (`props.rs:1031`, :1043-1063) | |
| **Δ No AtMostOneClient check.** CWA stores the event mask with no exclusivity test (`windows.rs:1455-1468`). Xorg returns `BadAccess` for a second SubstructureRedirect/ResizeRedirect/ButtonPress selector (`dix/events.c:4528-4635`) | |
| MapWindow/ConfigureWindow redirect to a SubstructureRedirect holder | `windows.rs:2391-2414`, :1799-1821; `subscribers_by_id` `core_loop/fanout.rs:131` |
| ConfigureWindow handler; request type | `windows.rs:1681`; `ConfigureWindowRequest` `yserver-protocol/src/x11/mod.rs:261` |
| SetInputFocus handler; typed apply | `focus_pointer.rs:94`; `apply_core_input_focus` :153 (`pub(super)`) |
| SendEvent; recipients | `selection.rs:359`; `send_event_recipients` :486 |
| KillClient | `misc.rs:398-460` |
| Selections | `ServerState.selections: HashMap<AtomId, (ResourceId, u32)>` `server.rs:1091` |
| Hit test | `root_pointer_target_at` `server.rs:3143`. Callers: `server.rs:3493`, :3954, :4045, :4172; `selection.rs:493`; `pointer_fanout.rs:3813`; `xi/dispatch/grabs.rs:351`, :514; `focus_pointer.rs:580`; XBackend `portable/pointer.rs:57` |
| Content origin | `window_absolute_position` `resources/tree.rs:337` |
| **Δ The server reset replaces `*state` wholesale** (`core_loop/reset.rs:427`), so `rootless` is lost on a reset. The default policy is `-noreset` (`yserver/src/launch.rs:123-126`) | |
| Root resize from a host | `host_screen_resized` `run.rs:3569` |
| `HostInputEvent`; `Message` | `core_loop/message.rs:328`; :200. Exhaustive matches: XBackend `portable/devices.rs:408-760`, `host_x11/trait_impl.rs:195` |
| Size budget (`tools/check-layout.sh`: 5,000 code / 6,000 test lines, no exceptions file) | `run.rs` is **4862** lines, so it gets ≤ 40 new lines and all logic lives in `rootless/`. `server.rs` 4472, `trait_def.rs` 3210, `windows.rs` 3105, `recording.rs` 2868 |

## Global constraints

- **Off by default, real selection.** The mode is the `Option` in `ServerState`, set only by
  `rootless::enable(state, backend, policy)`. RL0 has no production caller. RL1 calls it from `run_headless
  --rootless`, RL2 from macOS. No `YSERVER_*` switch, no cfg gate, no kill-switch (project rule). RL0 adds no CLI flag
  either: a `--rootless` that changes nothing would be a lie (decision D3).
- **Rooted is identical.** Each hook is guarded by `state.rootless.is_some()`. Rooted proof: the full workspace test
  suite and lavapipe stay green without edits to existing tests. The one rooted fix RL0 needs is P1, its own PR.
- **Oracle.**
  - X behaviour comes from the X11 protocol, ICCCM 2.0, EWMH 1.5, `../xserver` 21.1.24 source, or Xwayland
    24.1.9 (`target/rootless-research/xwayland`).
  - The interface contract (event shapes, acks, popup parent order) comes from the spec section it implements, cited
    per test.
  - A test value with no such source is not written. If a value needs a measurement, the test waits for it.
- **Red first, mutation named.** Every test names the mutation that must turn it red, and that mutation is run once.
- **Typed server actions, no wire bodies.** New internal X actions call typed `*_as_server` entry points. The
  screensaver's LE-body pattern (`saver_dpms.rs:278-321`) is not copied.
- **Gate per task** (each command under `env -u RUSTC_WRAPPER`):
  1. `cargo +nightly fmt --all`
  2. `cargo clippy --workspace --all-targets -- -D warnings`
  3. `cargo clippy --workspace --all-targets --features xdmcp -- -D warnings`
  4. `cargo test --workspace --all-targets --locked`
  5. `YSERVER_ALLOW_SOFTWARE_VULKAN=1 VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json cargo test --workspace
     --locked --no-fail-fast -- --ignored`
  6. `tools/check-layout.sh`
- **Commits:** one per task, made by jos after the gate. Trailer `Co-Authored-By: Claude Opus 5.5 (1M context)
  <noreplyanthropic.com>`. Never a session URL. No HW smoke in RL0: nothing user-visible changes.

## Prerequisites (own PRs, not RL0 commits)

- **P1 — property byte order (D1, required).** `fix/property-byte-order`: ChangeProperty stores format-16/32 data in
  server (LE) order for big-endian clients, and GetProperty swaps it back per format for big-endian requesters (Xorg
  `dix/swapreq.c:209-229`, `dix/property.c:535-549`; Δ in the facts table). Tests: a BE writer's `CARDINAL/32` is
  stored canonical, an LE reader sees a BE writer's values (and the reverse), format 8 is never swapped, and a BE
  Append concatenates values. **RL0 depends on it:** Task 4's big-endian `WM_NORMAL_HINTS` vector and Task 5's BE
  ClientMessage decoding are only correct on top of P1. `feat/rootless-rl0` is cut after P1 merges.
- **P2 — SubstructureRedirect exclusivity (D2, independent).** A separate rooted fix: Xorg's AtMostOneClient check
  (`dix/events.c:4528-4635`), which gives a second client selecting SubstructureRedirect, ResizeRedirect or
  ButtonPress on a window `BadAccess`, with CWA's partial apply up to `CWEventMask` (`dix/window.c:1372-1377`,
  :1569-1574). RL0 does **not** rely on it: in rootless mode no client holds SubstructureRedirect on the root (the
  server holds no event mask at all), so RL0's root refusal (Task 5) is its own check either way. If P2 lands first,
  Task 5 reuses P2's partial-apply-and-`BadAccess` path instead of adding its own; if not, Task 5 adds the
  root-only check and P2 later folds it in.

## Module layout

`crates/yserver-core/src/rootless/` (registered in `lib.rs` as `pub mod rootless;`):

| File | Holds | ~lines |
|---|---|---|
| `mod.rs` | `Rootless`, `enable`, `reconcile_and_dispatch`, `host_request`, `CoordinatePolicy`, re-exports | 220 |
| `types.rs` | `ToplevelId`, `ToplevelKind`, `Geometry`, `GeometryFields`, `HostSerial`, `RootlessEvent`, `HostRequest`, `RootlessInput`, `WmRequest`, `InfoFields`, `NetWmState` | 260 |
| `topology.rs` | snapshot, top-level predicate, diff → events, popup parent | 420 |
| `acks.rs` | per-top-level `acked`/`client_fields`/`from_client` bookkeeping | 140 |
| `info.rs` | `WindowInfo` + parsers (pure functions over `PropertyValue`) | 380 |
| `wm.rs` | EWMH announce, `WM_STATE`, client lists, root ClientMessage → `WmRequest`, close/activate, size hints | 420 |
| `seat.rs` | pointer scope, local → root mapping, activation/deactivation | 220 |
| `tests/{mod,topology,acks,info,wm,seat}.rs` | `RecordingHost` fixture + tests | 1,600 |

---

### Task 1: Types, mode state, `Backend::on_rootless_events`, the tail call

**Files:**
- Create: `rootless/mod.rs` and `rootless/types.rs` with the spec §4.2/§4.4 types, `Clone + Debug + PartialEq`:

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)] pub struct ToplevelId(NonZeroU64);
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)] pub struct HostSerial(pub u64);
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)] pub struct GeometryFields(u8); // X Y WIDTH HEIGHT BORDER
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Geometry { pub x: i32, pub y: i32, pub width: u16, pub height: u16, pub border: u16 }
pub enum ToplevelKind { Managed, Popup { parent: Option<ToplevelId> } }
pub enum RootlessEvent { Mapped{..}, Unmapped{..}, Configured{..}, Restacked{..}, Reshaped{..},
                         InfoChanged{..}, WmRequest{..}, Destroyed{..}, ScreenChanged{..} } // exactly §4.2
pub enum CoordinatePolicy { Global { origin: (i32, i32) }, Opaque }  // Scale stays host-side (§4.0)
pub struct Rootless { /* next_id, policy, toplevels: HashMap<ResourceId, Tracked>, order: Vec<ToplevelId>,
                         dirty_props: HashSet<ResourceId>, pending_wm: Vec<(ResourceId, WmRequest)>,
                         scope: PointerScope, wm_check: ResourceId } */
pub fn enable(state: &mut ServerState, backend: &mut dyn Backend, policy: CoordinatePolicy)
    -> Result<(), RootlessError>;                     // body grows in Tasks 2 and 5
pub fn reconcile_and_dispatch(state: &mut ServerState, backend: &mut dyn Backend);
```

- `CoordinatePolicy::Global` carries no `Scale` in RL0. §4.0 keeps scaling at the host edge, so the core never
  reads S.
- Modify: `server.rs`. Add `pub rootless: Option<crate::rootless::Rootless>` to `ServerState` (:1048), set to `None`
  in `new` (:1682).
- Modify: `trait_def.rs`, after `on_platform_event` (:1049):
  `fn on_rootless_events(&mut self, _state: &ServerState, _events: &[RootlessEvent]) {}`.
  `recording.rs` gets `RecordedCall::RootlessEvents(Vec<RootlessEvent>)`.
- Modify: `run.rs:3097-3104`. Add one line between `drain_present_completions` and `maybe_composite`:
  `crate::rootless::reconcile_and_dispatch(state, backend);`.
  - It returns at once when `state.rootless` is `None`.
  - It does `take()` → `reconcile(&state)` → put back → `backend.on_rootless_events` when non-empty.
  - Taking the value out and putting it back avoids borrowing `state` twice.
- Modify: `reset.rs`.
  - Before step 6 (:409): call `reconcile_and_dispatch`, so hosts get `Unmapped`/`Destroyed` for the generation's
    windows.
  - Carry `Rootless::for_next_generation()` across the swap. It keeps `next_id` and the policy, so a `ToplevelId` is
    never reused across generations.
  - After step 7 (:429-433): re-run the startup redirect and the announce (Tasks 2 and 5).

- `Rootless` keeps a `pending: Vec<RootlessEvent>` queue that `reconcile` drains first. Later tasks feed it
  (`WmRequest`, `ScreenChanged`); in this task nothing in production does, so `reconcile` returns an empty list and
  the tail call is a no-op even in rootless mode. A `#[cfg(test)] pub(crate) fn queue_event_for_test` pushes onto it.

**Tests** (only what this commit can make green; the topology-driven cases are in Task 3):
- `run/tests.rs`: extend :3851 with `run_iteration_tail_dispatches_rootless_events_between_drain_and_compose`.
  - Enable the mode on `RecordingBackend::new().with_redirect_activation()`, so the test stays valid once Task 2
    requires it. Queue one `ScreenChanged { root: (640, 480) }`, then call `run_iteration_tail`.
  - The call log is `DrainCompletedPresentEvents` < `RootlessEvents([ScreenChanged{..}])` < `MaybeComposite`.
  - Mutation: move the call to `before_block`, or after `maybe_composite`. Source: spec §4.1 loop order.
- `rooted_tail_records_no_rootless_events`: the same tail without `enable`; there is no `RootlessEvents` entry.
  Mutation: drop the `None` guard.
- `empty_reconcile_records_nothing`: rootless on, nothing queued → no `RootlessEvents` entry (spec §4.2.1 rule 7).

**Gate**, then commit `feat(rootless): core mode state, event types and the reconcile slot before compose`. ~450
lines (types ~260).

### Task 2: Auto-redirect of every root child as the server (Manual)

**Files:**
- Create: `rootless/mod.rs::redirect_root_children(state, backend)`. It mirrors the Composite request path without a
  client:
  1. collect the targets exactly as `composite_damage.rs:111-122` does: the children of `ROOT_WINDOW` minus
     `COMPOSITE_OVERLAY_WINDOW`. Extract that filter into
     `pub(crate) fn subwindows_redirect_targets(state, parent) -> Vec<ResourceId>` and call it from both places;
  2. take `before = window_mode(t)` for each target;
  3. `state.composite_redirects.redirect_subwindows(ROOT_WINDOW, &targets, RedirectRecord { mode: Manual, owner:
     SERVER_OWNER })`;
  4. `sync_redirect_backing(state, backend, None, t, before, BackingDamage::Allocation)` per target
     (`redirect.rs:295`). Make it `pub(crate)`; it already is.
- `enable` fails with `RootlessError::NoRedirectActivation` when `!backend.supports_redirect_activation()`, because
  rootless without backings is meaningless. It fails with `RootlessError::ManualTaken` if a client already holds
  Manual on root. RL1 calls `enable` before any listener is accepted, so that second case is unreachable in
  production. It is still an error, never a silent overwrite.
- No other code changes. After startup, the existing machinery carries the redirect:
  - new root children through `redirect_new_subwindow` (`windows.rs:1277-1282`);
  - reparents through `reparent_subwindow` + `sync_redirect_backing` (`windows.rs:849-869`);
  - destroys through `forget_windows` (`windows.rs:487`).
- **Clash with a client compositor** (spec §6.5):
  - `RedirectSubwindows(root, Manual)` from any client hits `ManualTaken` → `BadAccess` (`composite_damage.rs:
    139-171`). That is what a compositor sees under Xwayland, and picom/xcompmgr exit with their usual message.
  - `RedirectWindow(top-level, Manual)` is refused too: the top-level already holds our Manual record
    (`composite_redirects.rs:119-131`).
  - Automatic redirects and Manual redirects of non-top-levels keep working.
  - A client's `Unredirect*` only removes its own records (`remove_one` matches the owner, `composite_redirects.rs:
    51-67`), so no client can strip the server's redirect.
  - Risk: a compositor that ignores the `BadAccess` and keeps running would draw into a COW that nothing shows. That
    is accepted (decision 2). The `BadAccess` is the whole defence.

**Tests** (`rootless/tests/topology.rs`, `RecordingBackend::new().with_redirect_activation()`):
- `enable_redirects_existing_root_children_manual_as_server`: two root children exist before `enable`. Both have
  `window_mode == Manual`, both records are owned by `SERVER_OWNER`, and the recorder shows
  `AllocateRedirectedBacking` for each mapped one. The COW gets no record. Mutation: skip existing children (redirect
  only at map/create).
- `root_child_created_after_enable_is_redirected_at_creation`: a CreateWindow by wire, still unmapped, has
  `window_mode == Manual` (Xorg `compCreateWindow`, `composite/compwindow.c:579-589`). Mutation: redirect at map.
- `client_compositor_manual_on_root_gets_bad_access`: the wire `RedirectSubwindows(root, Manual)` from client 1
  errors with `BadAccess`, the records are unchanged, and `Automatic` succeeds (`compalloc.c:336-339`).
- `client_disconnect_never_drops_server_records`: the client owning a top-level's sibling disconnects, and the
  remaining top-level keeps its `SERVER_OWNER` Manual record. Mutation: `forget_client(SERVER_OWNER)` on any
  disconnect.

**Gate**, then commit `feat(rootless): the server redirects every root child Manual at startup`. ~180 lines.

### Task 3: Topology reconcile → Mapped / Unmapped / Configured / Restacked / Reshaped / Destroyed

**Files:**
- Create: `rootless/topology.rs`.
  - **Snapshot** per tracked root child: `(ToplevelId, kind, Geometry, depth, above, bounding, input)`.
  - **Predicate** (spec §4.1; SetZero `topology.rs:534` [F]): a root child, `MapState::Viewable`, class
    `InputOutput`, not `COMPOSITE_OVERLAY_WINDOW`, not the `_NET_SUPPORTING_WM_CHECK` window, and not
    `SCREEN_SAVER_WINDOW` (decision D5). Override-redirect gives `Popup`, everything else `Managed`.
  - **`Rootless::reconcile(&mut self, state: &ServerState) -> Vec<RootlessEvent>`**, in this order:
    Only mapped top-levels are tracked: an unmapped window holds no id and no tracking entry.
    1. tracked windows that are gone or no longer root children (destroyed, reparented away, owner disconnected) →
       `Unmapped` then `Destroyed`. The id is retired with the tracking entry;
    2. tracked windows still root children but no longer viewable (unmap) → `Unmapped` only. `Unmapped` means
       "destroy the native window" (spec §6.3); the id and the tracking entry go with it. A window that is
       unmapped and later destroyed produces no further event (`Destroyed` is only ever emitted right after the
       `Unmapped` of step 1);
    3. new viewable top-levels in bottom→top order → `Mapped` with final geometry, `above` = the nearest lower
       tracked top-level that is not restacked in this batch, and `info` (`WindowInfo::default()` in this task;
       Task 4 fills it);
    4. geometry diffs → `Configured` (fields from Task 6; Task 3 emits `acked: None` and `client_fields` = the
       changed fields);
    5. order diffs on surviving top-levels → `Restacked { from_client: true }`;
    6. shape diffs (`state.shape_windows`, `server.rs:1241`) → `Reshaped`, in backing pixels: every rect translated
       by `(+bw, +bw)` (spec §4.0);
    7. queued `WmRequest`s → `WmRequest` events (Task 5).
  - **`ToplevelId`** comes from a `NonZeroU64` counter, never from the XID, and names **one mapped lifetime**: a
    fresh id is allocated at every `Mapped` (first map and every remap) and retired at the `Unmapped` that ends it.
    `Rootless` keeps `live: HashMap<ToplevelId, ResourceId>` for live ids only. Every host input
    (`HostRequest`, `RootlessInput`) is resolved through `live` first; an id that is not live is **dropped** with a
    debug log, never acted on and never acked. So a close, move, ack or pointer event the host queued for the native
    window it destroyed on `Unmapped` cannot reach the window's next mapped lifetime.
  - **`Geometry`**: `x, y` = `window.x, y` (the outer corner of a root child), `width, height` = content,
    `border = border_width` (spec §4.0).
- Transient re-resolution (after review): `transient_for` is re-resolved every reconcile, so a parent that maps
  after its transient, or goes away, reaches the host as `InfoChanged { TRANSIENT_FOR }`; a popup whose
  placement parent changes (its `WM_TRANSIENT_FOR` changed or resolved, or the parent was retired) gets
  `ParentChanged { id, parent }`. `ToplevelId` is a `NonZeroU64` that wraps instead of panicking.
- Popup parent (spec §10 order; SetZero `popups.rs:256-310` [F]): `WM_TRANSIENT_FOR` if it names a tracked
  `Managed`; else the pointer-scope top-level (Task 7); else the topmost `Managed` whose outer rect contains the
  popup's outer origin; else the topmost `Managed`; else `None`. This task implements the last three rungs; the
  `WM_TRANSIENT_FOR` rung lands with the parser (Task 4) and the pointer-scope rung with the scope (Task 7), each
  with its own test there, so every commit stays green.

**Tests** (`rootless/tests/topology.rs`). The `RecordingHost` fixture enables rootless and drives requests through
`dispatch_wire_request` (`process_request/tests/mod.rs:347`), then calls `reconcile_and_dispatch`. It returns the
events from the last `RecordedCall::RootlessEvents`.
- `create_map_configure_in_one_iteration_is_one_mapped_with_final_geometry`: Create (10,10) 100×50, Map, Configure
  (20,30) 120×60 bw 2 → exactly `[Mapped{geometry:(20,30,120,60,2)}]`. Mutation: emit per request.
- `unmap_emits_only_unmapped`: Map, reconcile, Unmap → exactly `[Unmapped{id}]`. Mutation: emit `Destroyed` on
  unmap.
- `remap_after_unmap_gets_a_fresh_id`: then Map → exactly `[Mapped{id: id2}]` with `id2 != id`. Mutation: keep the
  id across an unmap.
- `destroy_mapped_emits_unmapped_then_destroyed`; `destroy_after_unmap_emits_nothing`;
  `client_disconnect_emits_unmapped_then_destroyed` (through `process_disconnect`).
- `reparent_under_non_root_then_back`: `[Unmapped, Destroyed]`, then `[Mapped]` with a new id.
- `xid_reuse_gets_a_new_toplevel_id`: destroy W, then create a new window with the same XID and map it. The ids
  differ. Mutation: derive the id from the XID.
- `override_redirect_is_popup_with_parent_order`: the containing-top-level, topmost and `None` rungs. Mutation:
  always pick the topmost. (`popup_parent_prefers_transient_for` is in Task 4, `popup_parent_prefers_pointer_scope`
  in Task 7.)
- `restack_raise_emits_restacked_from_client`: `ConfigureWindow(stack_mode=Above)` on the lower of two →
  `Restacked { above: Some(upper), from_client: true }`.
- `shape_reaches_host_in_backing_pixels`: bw 5, bounding rect `(0,0,10,10)` → `Reshaped.bounding == [(5,5,10,10)]`.
  Mutation: drop `+bw`. Source: shape is relative to the content origin (`dix/window.c:1735-1742`), and the backing
  origin is the outer corner (`compalloc.c:600-606`).
- `unchanged_iteration_emits_nothing` (spec §4.2.1 rule 7).
- `run/tests.rs` `run_iteration_tail_reports_a_window_mapped_this_iteration` (moved here from the slot task): map a
  root child by a wire request in this iteration, then `run_iteration_tail` → `DrainCompletedPresentEvents` <
  `RootlessEvents([Mapped{..}])` < `MaybeComposite`. Mutation: reconcile in `before_block`. Source: spec §4.1.
- `reset_flushes_destroyed_and_keeps_ids_fresh` (moved here): a reset in rootless mode emits `Unmapped`+`Destroyed`
  for every mapped tracked top-level; the next generation's first window gets an id above every old one. Mutation:
  rebuild `Rootless::new()` on reset.

**Gate**, then commit `feat(rootless): reconcile root children into a top-level event stream`. ~420 + 380 test lines.

### Task 4: `WindowInfo` parser and the property dirty set

**Files:**
- Create: `rootless/info.rs`. Pure decoders over `&PropertyValue` (canonical LE after P1). Each has a typed
  result and is lenient on short data, as Xlib is:

```rust
pub fn parse_window_info(state: &ServerState, window: ResourceId, ids: &dyn Fn(ResourceId) -> Option<ToplevelId>)
    -> WindowInfo;
fn title(state, w) -> String;                // _NET_WM_NAME UTF8_STRING, else WM_NAME STRING (Latin-1) / COMPOUND_TEXT (ASCII subset)
fn wm_class(v) -> Option<(String, String)>;  // two NUL-terminated strings
fn wm_normal_hints(v) -> SizeHints;          // ≥15 CARD32s accepted; 18 = ICCCM 1 with base + gravity
fn wm_hints(v) -> (Option<bool>, bool);      // flags bit0 InputHint → input; bit8 XUrgencyHint
fn wm_protocols(state, v) -> Protocols;      // ATOM list: WM_DELETE_WINDOW, WM_TAKE_FOCUS, _NET_WM_PING
fn motif_decorations(v) -> Decorations;      // flags bit1 → decorations field: 0 None, bit0 All, else subset
fn net_wm_window_type(state, v, transient: bool) -> WindowType;
fn net_wm_state(state, v) -> NetWmState;
```

- Modify: `props.rs:1102`, :1237, :1273. Next to each `backend.on_window_property_changed(...)`, add
  `crate::rootless::note_property_changed(state, req.window, req.property)`. It inserts into `dirty_props` when the
  mode is on and does nothing otherwise. Reconcile re-parses dirty tracked windows and emits
  `InfoChanged { info, changed }`, where `changed` is the `InfoFields` diff against the last info. A
  `WM_TRANSIENT_FOR` change re-resolves `transient_for`.

**Test vectors** (`rootless/tests/info.rs`). Byte layouts are taken from the specs; the values are those of a
real-world hint set, written out in the test with their source:
- **`WM_NORMAL_HINTS`** (ICCCM §4.1.2.3; `Xutil.h:82-93`, :102-112 flag bits). The layout is flags, 4 pad, min w/h,
  max w/h, inc w/h, min_aspect n/d, max_aspect n/d, base w/h, gravity.
  - Vector: flags `PMinSize|PResizeInc|PBaseSize|PWinGravity` = `0x0350`, min 10×17, inc 6×13, base 4×4,
    gravity 1.
  - Expect `min=(10,17)`, `inc=(6,13)`, `base=(4,4)`, `max=None`.
  - The test runs once with an LE client and once with a BE client, both through the wire ChangeProperty. Both must
    parse identically; that is what P1 provides.
  - Mutation: decode the BE bytes as raw. 15-word data (pre-ICCCM-1) yields `base=None`.
- **`WM_HINTS`** (ICCCM §4.1.2.4; `Xutil.h:134-143`): flags `InputHint` with input 0 → `input_hint == Some(false)`.
  Flags `0x100` → `urgency`. No property → `input_hint == None`.
- **`_NET_WM_NAME` over `WM_NAME`** (EWMH §5.1, "MUST be used in preference"): both set → the UTF-8 title. Only
  `WM_NAME` `STRING` `0xE9` → `"é"` (Latin-1, ICCCM §2.7.1). Mutation: prefer `WM_NAME`.
- **`_MOTIF_WM_HINTS`**: layout from Motif `MwmUtil.h` as XWin uses it (`hw/xwin/winwindow.h:114-136`). Flags `2`,
  decorations `0` → `Decorations::None`; flags `0` → `Full`.
- **`_NET_WM_WINDOW_TYPE`** (EWMH §5.6): `[_NET_WM_WINDOW_TYPE_DIALOG]` → `Dialog`. An unknown atom first, then
  `NORMAL` → `Normal` ("first known atom"). No property + `WM_TRANSIENT_FOR` → `Dialog` (EWMH §5.6, the
  transient-for default).
- **`WM_CLASS`** (ICCCM §4.1.2.5): `"xterm\0XTerm\0"` → `("xterm","XTerm")`.
- `title_change_emits_info_changed_with_only_title`: a ChangeProperty of `_NET_WM_NAME` after map →
  `InfoChanged { changed: TITLE }`. Mutation: no dirty hook at `props.rs:1102`.
- `delete_property_marks_dirty`: the same through DeleteProperty (`props.rs:1273`).
- `popup_parent_prefers_transient_for` (rung 1 of the popup parent order, spec §10): an override-redirect window
  with `WM_TRANSIENT_FOR` = a lower managed top-level gets that parent, not the topmost. Mutation: drop the rung.

**Gate**, then commit `feat(rootless): parse ICCCM/EWMH window info and report changes`. ~380 + 330 test lines.

### Task 5: WM policy (EWMH announce, WM_STATE, client lists, root ClientMessages, SubstructureRedirect refusal)

**Files:**
- Create: `rootless/wm.rs`.
  - **Announce at `enable`** (spec §6.1):
    - a server-owned InputOnly, unmapped window `ROOTLESS_WM_CHECK_WINDOW = ResourceId(0x107)` (new constant in
      `resources/mod.rs`, next to :49), created through a typed `create_window_as_server`;
    - `_NET_SUPPORTING_WM_CHECK` on the root and on itself, plus `_NET_WM_NAME = "yserver"` (UTF8_STRING) on it
      (EWMH §3.13);
    - `_NET_SUPPORTED` = exactly `_NET_SUPPORTED, _NET_SUPPORTING_WM_CHECK, _NET_WM_NAME, _NET_ACTIVE_WINDOW,
      _NET_CLIENT_LIST, _NET_CLIENT_LIST_STACKING, _NET_WM_STATE, _NET_WM_STATE_{FULLSCREEN, MAXIMIZED_VERT,
      MAXIMIZED_HORZ, ABOVE, BELOW, HIDDEN}, _NET_WM_MOVERESIZE, _NET_CLOSE_WINDOW, _NET_FRAME_EXTENTS, _NET_REQUEST_FRAME_EXTENTS` (the extents are CARDINAL[4] zeros, set at map and in answer to a request, also before map);
    - `WM_S0` owned by the check window (ICCCM §2.8).
  - **`WM_STATE`** (ICCCM §4.1.3.1: type `WM_STATE`, format 32, `[state, icon=None]`):
    - `Mapped{Managed}` → `NormalState` (1), or `IconicState` (3) when `WmState::Iconic` is set by a minimize;
    - `Unmapped` of a still-existing window → `WithdrawnState` (0);
    - popups get none (ICCCM §4.1.10: override-redirect windows are not managed).
  - **`_NET_CLIENT_LIST`** (managed, in initial map order) and **`_NET_CLIENT_LIST_STACKING`** (bottom→top), both
    `WINDOW[]` on the root (EWMH §3.2), are rewritten when the set or the order changes.
  - Property writes go through `change_property_as_server(state, backend, window, prop, type, format, data)`. It is
    `pub(crate)` in `props.rs`, split from `handle_change_property` after validation: set, the backend hook, then
    PropertyNotify (`props.rs:1100-1122`).
  - **Root ClientMessages:** in `selection.rs:359`, after normal delivery (unchanged), call
    `crate::rootless::note_root_client_message(state, &req.event, sender_byte_order)` when the destination is
    `ROOT_WINDOW`, the type is 33, and the format is 32. It decodes the template in the **sender's** order (the
    template is not swapped, `selection.rs:365-369`) and queues:
    - `_NET_WM_STATE`: l0 = 0/1/2 remove/add/toggle, l1/l2 = state atoms (EWMH §5.7) → `SetFullscreen(bool)`,
      `SetMaximized(bool)` (only when both VERT and HORZ end up equal), `SetLayer`, `Minimize`. Toggle resolves
      against the window's current `_NET_WM_STATE`.
    - `WM_CHANGE_STATE` with l0 = 3 (ICCCM §4.1.4) → `Minimize`.
    - `_NET_WM_MOVERESIZE`: l2 direction 0-7 edges, 8 move, 11 cancel; l3 button (EWMH §4.3) →
      `BeginMove { button }`, `BeginResize { edges, button }`. Directions 9/10 (keyboard) map to the same with
      `button: 0`.
    - `_NET_ACTIVE_WINDOW` (EWMH §3.5) → `Activate`. `_NET_CLOSE_WINDOW` (EWMH §4.1) → `Close`.
  - **SubstructureRedirect refusal** (spec §6.5). In `handle_change_window_attributes` (`windows.rs:1307`), when
    rootless is on, the window is the root, and the mask has `SubstructureRedirect (0x0010_0000)` or `ResizeRedirect
    (0x0004_0000)`: apply the attributes whose CW bits sit below `CWEventMask` (bit 11), apply none of the later
    ones, and return `BadAccess`. That is Xorg's partial apply through `PatchUp` (`dix/window.c:1372-1377`,
    :1569-1574). The general AtMostOneClient gap is P2 (decision D2); if P2 has landed, reuse its path.
- **ConfigureRequest/MapRequest:** none are generated, by design. Nobody holds SubstructureRedirect on the root
  (refused above), so `windows.rs:2391-2414` and :1799-1821 find no holder and apply directly.

**Tests** (`rootless/tests/wm.rs`):
- `enable_announces_ewmh`: on the root, `_NET_SUPPORTING_WM_CHECK` = 0x107; on 0x107, the same value and
  `_NET_WM_NAME = "yserver"`; `_NET_SUPPORTED` is the exact list above; `GetSelectionOwner(WM_S0)` = 0x107. Source:
  EWMH §3.1, §3.13; ICCCM §2.8.
- `map_sets_wm_state_normal_and_client_list`: `WM_STATE` bytes = `[1,0]` as LE words, type atom `WM_STATE`;
  `_NET_CLIENT_LIST = [w]`. A PropertyNotify reaches a client selecting PropertyChange on w. Mutation: skip WM_STATE.
- `unmap_sets_withdrawn`; `popup_gets_no_wm_state`.
- `client_map_and_configure_apply_without_request_events`: a second client selecting SubstructureNotify on the root
  sees MapNotify/ConfigureNotify and never MapRequest/ConfigureRequest. The window geometry is what the client asked
  for.
- `substructure_redirect_on_root_is_bad_access`: CWA(root, event_mask = SubstructureRedirect|SubstructureNotify)
  → error 10 (BadAccess); the client's mask on the root is unchanged; a CWA without that bit still succeeds. Rooted
  control: the same request succeeds without `enable`. Mutation: drop the check.
- `net_wm_state_add_fullscreen_maps_to_wm_request`, and the same for `toggle maximized` (both atoms), `WM_CHANGE_STATE
  Iconic`, `_NET_WM_MOVERESIZE dir 8 button 1`, `_NET_ACTIVE_WINDOW`, `_NET_CLOSE_WINDOW`. Each comes from a **BE
  and an LE** sender, and each is also delivered to another root SubstructureNotify listener (unchanged delivery).
  Mutation: decode the template as LE.

**Gate**, then commit `feat(rootless): built-in WM policy (EWMH announce, WM_STATE, root client messages)`. ~420 + 380
test lines.

### Task 6: Host requests, typed server actions, serial acks

**Files:**
- Modify: `windows.rs:1681`. Split `handle_configure_window` into parse plus `pub(crate) fn configure_window_apply(state,
  backend, origin, client_id, sequence, request: ConfigureWindowRequest, source: ConfigureSource)`.
  `ConfigureSource::{Client, Host(HostSerial), Policy}`. Behaviour is unchanged for `Client`.
- Add `rootless::configure_window_as_server(state, backend, window, values, source)` →
  `configure_window_apply(.., SERVER_OWNER, SequenceNumber(0), ..)`.
- Add `set_input_focus_as_server` (validate viewable, then `apply_core_input_focus` `focus_pointer.rs:153`, made
  `pub(crate)`), `send_client_message_as_server` (event mask 0 → the window's creator, X11 SendEvent semantics, through
  `send_event_recipients` `selection.rs:486` made `pub(crate)`), and `kill_client_as_server` (`process_disconnect`, the
  `misc.rs:454-460` branch).
- Create: `rootless/acks.rs`. Per top-level, `acked: Option<HostSerial>`, `client_fields: GeometryFields`,
  `restack_acked`, `from_client` (spec §4.2.1 rules 1-7).
  - A `Host(serial)` configure sets `acked = max(acked, serial)` and clears the fields it sets.
  - A `Client` or `Policy` configure ORs the fields in its value mask.
  - Reconcile also ORs any changed field that no host request explains. This is a backstop for a geometry path that
    skips `configure_window_apply`; it is logged at debug.
  - Reset after emitting.
- `Rootless::host_request(state, backend, req: HostRequest)`, dispatched from a new `Message::Rootless(HostRequest)`
  arm in `run.rs` next to :2793 (≤ 4 lines; the logic stays in `rootless/`):
  - `Moved { x, y }` (Global only; Opaque logs and drops) → ConfigureWindow X|Y = the outer corner (spec §4.0).
  - `Resized { W, H }` → content `(W − 2bw, H − 2bw)`, then clamped by `SizeHints` (ICCCM §4.1.2.3: min/max, then
    `base + k·inc`, with base defaulting to min and min to base). A clamped dimension is a `Policy` field (rule 5).
  - `Restacked { above }` → stack_mode Above sibling (or Below with no sibling when `above == None`).
  - After any host move without a resize, a synthetic ConfigureNotify (ICCCM §4.1.5) copies the real one with
    `send_event` set, `event = window`, mask StructureNotify, as XWin does (`winmultiwindowwm.c:1294-1314`).
  - `CloseRequested` → `WM_DELETE_WINDOW` (ICCCM §4.2.8.1: `WM_PROTOCOLS`, format 32, `[WM_DELETE_WINDOW, time]`)
    when listed. Otherwise nothing happens (decision 12; see D6). `ForceKill` → `kill_client_as_server`.
  - `StateChanged` → `_NET_WM_STATE` + `WM_STATE` Iconic/Normal (decision 3: the window stays mapped).
  - `ScreenLayout { outputs }` accepts **exactly one** output in RL0 (D7). With one output,
    `host_screen_resized(w, h)` (`run.rs:3569`) is accurate: it sets the root and the single RandR output to the
    same size, which is the truth. With zero or several outputs the request is rejected (warn log, no change, no
    event): `host_screen_resized` would report the first RandR output at the bounding-box size, a layout that does
    not exist. Multi-output, with one RandR output per host output, is RL2. Chosen over a root-only resize because a
    root that disagrees with the only RandR output is also a misleading layout, and macOS RL2 needs the per-output
    model anyway. Then `ScreenChanged`.
  - `Activated`/`Deactivated` → Task 7.
  - `PopupDismissed` is left out of RL0 (D8).
  - Every request first resolves its id through `live` (Task 3); a stale id is dropped.

**Tests** (`rootless/tests/acks.rs`). Expectations are spec §4.2.1, cited per rule:
- `host_move_alone_acks_with_no_client_fields_and_sends_synthetic_configure` (rule 2; ICCCM §4.1.5).
- `host_move_plus_client_resize` → `acked 7, client_fields WIDTH|HEIGHT` (rule 3).
- `host7_client_move_host8` → `acked 8, client_fields ∅` (rule 3: the later host write clears X|Y).
- `host7_then_client_move` → `client_fields ⊇ X|Y` (rule 3). Mutation: drop client fields when acked is set (r0
  `ChangeOrigin`).
- `host_resize_clamped_by_hints_reports_width_height`. Hints min 10×17, inc 6×13, base 4×4, bw 0. `Resized(203,
  100)` → content `202×95`, because `4 + ⌊199/6⌋·6 = 202` and `4 + ⌊96/13⌋·13 = 95` (ICCCM §4.1.2.3), with
  `client_fields ⊇ WIDTH|HEIGHT` (rule 5).
- `noop_host_request_is_still_acked` (rule 2).
- `host_restack_acks_and_client_raise_sets_from_client` (rule 6).
- `resized_on_bordered_window_subtracts_two_bw`: bw 5, `Resized(110,60)` → content 100×50 (spec §4.0). Mutation: drop
  `−2bw`.
- `close_requested_sends_delete_window_when_listed`: the client gets a ClientMessage `[WM_DELETE_WINDOW, t]`
  with `send_event` set.
- `close_requested_without_protocol_never_kills`; `force_kill_disconnects_owner` (decision 12).
- `moved_is_dropped_under_opaque_policy`.
- `stale_id_requests_are_dropped`: map W (id1), unmap, remap (id2). Then inject, with id1: `CloseRequested` (no
  ClientMessage reaches the client), `Moved{serial 9}` (geometry unchanged, and the next `Configured` for id2 does
  not carry `acked: 9`), `Resized`, `Restacked`, `ForceKill` (the client stays connected). The same requests with
  id2 act. Mutation: resolve ids by XID, or keep ids across an unmap. Source: spec §4.2 (an id names one mapped
  lifetime).
- `screen_layout_accepts_one_output`: one output 1280×800 → root and the RandR output are 1280×800, plus
  `ScreenChanged { root: (1280, 800) }`. `screen_layout_rejects_two_outputs`: root and RandR unchanged, no event.
  Mutation: resize to the bounding box.

**Gate**, then commit `feat(rootless): host requests with serial acks and typed server actions`. ~380 + 420 test lines.

### Task 7: Input mapping, pointer scope, activation

**Files:**
- Modify: `message.rs:328`. Add **one** variant, `HostInputEvent::Rootless(RootlessInput)`, with
  `RootlessInput::{PointerEnter{origin, toplevel, local, time}, PointerMotionLocal{..}, PointerLeave{origin, time}}`
  (spec §4.4).
  - One variant keeps the exhaustive matches small. The two backends get a single arm that logs and drops, because
    the core converts these before any backend sees them: XBackend `portable/devices.rs` (inside :408-760) and
    `host_x11/trait_impl.rs:195`.
- Modify: `run.rs:4299` `handle_host_input`. When the event is `Rootless(input)`, call
  `crate::rootless::seat::map_input(state, backend, input)` and return.
  - Enter/Motion set `scope = Within(T)`. They forward `PointerMotion { x, y, relative: false, dx, dy, motion_delta:
    None }` with root `= window_absolute_position(T) − (bw,bw) + ⌊local⌋` (spec §4.4), and `dx/dy` = the change from
    `state.pointer_root`.
  - Leave sets `scope = Nowhere`. It forwards a release (`PointerButton { pressed: false }`) for each bit in
    `state.buttons_down` (`server.rs:1169`): button 1 → `0x110`, 2 → `0x112`, 3 → `0x111` (the inverse of the
    XBackend mapping). Then it forwards a motion to the current position, so the hit test returns the root and
    crossings deliver LeaveNotify.
  - An id that is not live (unknown, unmapped, or from an earlier mapped lifetime) is dropped (a stale host queue
    entry, Task 3).
- Modify: `server.rs:3143` `root_pointer_target_at`. With rootless on:
  - `Within(T)` → `hit_test_child(T, x, y)`, recursed as `pointer_target_at_inner` does; a miss gives `(ROOT, x, y)`;
  - `Nowhere` → `(ROOT, x, y)`;
  - rooted, or `Anywhere`, keeps today's body.

  Every caller listed in the Facts table inherits the scope, including the XBackend at `portable/pointer.rs:57`, so
  crossings, SendEvent PointerWindow and grabs agree.
- `seat.rs` activation:
  - `Activated { id }`: if `input_hint != Some(false)`, `set_input_focus_as_server(window, RevertTo PointerRoot,
    CurrentTime)`. If `WM_TAKE_FOCUS` is listed, send `[WM_TAKE_FOCUS, timestamp_now]`; the timestamp must not be
    CurrentTime (ICCCM §4.1.7, §4.2.8). Set root `_NET_ACTIVE_WINDOW = [w]`.
  - `Deactivated`: forward a `Key` release for every bit set in `state.keys_down` (`server.rs:1152`), then focus
    None, then `_NET_ACTIVE_WINDOW = [None]`.
  - Popups are never activated: `Activated` on a popup id is dropped (spec §6.2).
- `RecordingBackend::on_host_input` (`recording.rs:1272`) records pointer motion and button events as well, as
  `RecordedCall::HostPointer{..}`. The test needs to see what the core forwards.

**Tests** (`rootless/tests/seat.rs`):
- `local_origin_on_bordered_toplevel_hits_the_border`: T at (40,30) 100×50 bw 5. `MotionLocal(0,0)` → forwarded root
  (40,30); `root_pointer_target_at(40,30) = (T, −5, −5)`. Source: the border is inside the window's hit box
  (`mi/miwindow.c:756-760`), and event coordinates are relative to the content origin (`drawable.x`,
  `dix/window.c:888`). Mutation: no `−bw`.
- `local_bw_bw_is_content_origin`: `MotionLocal(5,5)` → root (45,35) → `(T, 0, 0)`.
- `bordered_popup_maps_the_same_way` (spec §10, SetZero `seat.rs:273-317` [F]).
- `scope_follows_native_window_not_x_stacking`: A above B in X with both overlapping; `PointerEnter(B, p)` where p is
  inside both hits B. Mutation: unscoped hit test. Source: spec §4.4 (the native system is authoritative).
- `leave_releases_buttons_then_moves_to_root`: button 1 down, `PointerLeave` → recorded `HostPointer` release
  `0x110`, then motion; the hit test returns the root. LeaveNotify delivery itself is XBackend's crossing code,
  covered by the lavapipe test below.
- Lavapipe (`#[ignore]`, `crates/yserver/src/surface/tests.rs`, `headless_backend`): `rootless_leave_sends_leave_
  notify`. Enter T, then Leave: a client selecting LeaveWindow on T gets LeaveNotify with detail `NotifyAncestor`
  (protocol §11: the pointer moves from T to its parent, the root). Mutation: skip the motion after the scope change.
- `activated_input_false_take_focus_sends_only_take_focus` (ICCCM §4.1.7 Globally Active): no focus change, one
  ClientMessage `[WM_TAKE_FOCUS, t≠0]`.
- `activated_passive_sets_focus_and_active_window`.
- `deactivated_releases_keys_and_clears_focus`: one key down → a recorded `HostKey` release; `core_focus` = None;
  `_NET_ACTIVE_WINDOW = [0]`.
- `rooted_hit_test_unchanged`: without `enable`, overlapping windows hit the X-topmost.
- `stale_id_input_is_dropped`: map, unmap, remap W (id1 → id2). `PointerEnter`/`PointerMotionLocal` with id1 forward
  nothing (no `HostPointer` recorded) and leave the scope unchanged; `Activated { id1 }` changes no focus. The same
  with id2 act. Mutation: resolve the id by XID.
- `popup_parent_prefers_pointer_scope` (rung 2, spec §10): with scope `Within(B)` and no transient-for, a popup
  mapped over A gets parent B. Mutation: drop the rung.

**Gate**, then commit `feat(rootless): local-coordinate input, pointer scope and activation`. ~260 + 380 test lines.

### Task 8: Transient placement, status entry

**Files:**
- `wm.rs`, Global policy only (spec §6.1): a `Managed` window mapped with `WM_TRANSIENT_FOR` and neither `USPosition`
  nor `PPosition` (`Xutil.h:102-104`) is centred on its parent's outer rect by a `Policy` configure before `Mapped`
  is emitted. Others are left to the host.
- `docs/status.md`: one entry covering RL0's scope, what is inert until RL1, and decisions D1-D8 as answered.

**Tests:**
- `transient_without_position_is_centred_on_parent`: parent (100,100) 400×300 bw 0, dialog 200×100 → outer origin
  (200,200). The `Mapped.geometry` already has it.
- `transient_with_pposition_is_left_alone`.
- `opaque_policy_never_places`.

**Gate**, then commit `feat(rootless): centre unpositioned transients; status`. ~80 + 90 test lines.

## Testing strategy

- **Recording host double.** RL0 has no `RootlessHost` trait (that is RL1, in the `yserver` crate). The core-side
  double is `rootless/tests/mod.rs::RecordingHost`. It owns a `ServerState` with rootless enabled and a
  `RecordingBackend::new().with_redirect_activation()`.
  - It drives client requests through `dispatch_wire_request` and runs `reconcile_and_dispatch`.
  - It returns the `RootlessEvent`s from `RecordedCall::RootlessEvents`.
  - It injects `HostRequest`/`RootlessInput` with a monotonic `HostSerial` it allocates itself, as a real host does.
  - RL1's `RecordingRootlessHost` wraps the same scenarios behind the trait.
- **Event-sequence tests** cover map, unmap, configure, restack, shape, destroy, disconnect, reparent and
  override-redirect (Task 3).
- **The serial coalescing matrix** (Task 6) is spec §4.2.1 rules 1-7, one test per rule.
- **Input mapping** with a bordered top-level and popup (Task 7).
- **WM policy:** ConfigureRequest/MapRequest never generated, SubstructureRedirect refused, `_NET_WM_*` on the root
  from both byte orders (Task 5).
- **Loop order** (Task 1 with a queued event; Task 3 with a window mapped in the same iteration).
- **The rooted-equality guard:** every hook has a rooted control test without `enable`, plus the unchanged existing
  suite.
- **Not in RL0:** the spec §10 `next_wakeup` cases. RL0's core needs no wakeup, because every topology change ends in
  a tail (spec §4.1 "Reconcile itself never needs a wakeup"). The damage/readiness/readback deadlines belong to the
  RL1 presenter. See D4.

## Risks

- **`run.rs` headroom.** At 4862 lines, two arms plus one tail call fit; anything more must go into `rootless/`.
  `check-layout.sh` catches an overflow.
- **P1 is a rooted behaviour change** (its own PR). It affects BE↔LE property exchange. Every internal reader
  already decodes LE, so they become correct for BE writers. Any hidden reader that relied on raw client order would
  break; grep `window_property(` readers in that PR's review.
- **Configure choke point.** `client_fields` relies on every top-level geometry change passing through
  `configure_window_apply`. The reconcile backstop (Task 6) keeps the host correct if one does not, but it reports
  the change as a client change. That is safe, never stale.
- **Reset.** A `-reset` server must flush and re-enable (Task 1). It is untested on hardware in RL0, and the default
  is `-noreset`.
- **WM_S0 takeover.** A WM run with `--replace` can take WM_S0 and then fails on SubstructureRedirect. WM_S0 is then
  owned by a dead manager. That is accepted; the refusal is what matters (decision 2).
- **Popup parent guess** can pick the wrong parent for multi-window apps. It is a placement hint only; Wayland is the
  host that cares (RL3).
- **Leaf + backing memory** (spec §11). It does not apply in RL0 (no pixels), but enabling the redirect in RL1 is
  where it shows up.

## Decisions (jos, after the codex review)

1. **D1:** the property byte-order fix is its own PR (P1) and lands before RL0; RL0 depends on it.
2. **D2:** SubstructureRedirect exclusivity (AtMostOneClient) is a separate rooted fix (P2), before or independent of
   RL0. RL0's root refusal does not depend on it; it reuses P2's path if P2 is already in.
3. **D3:** no `--rootless` flag in RL0. It arrives with `run_headless --rootless` in RL1.
4. **D4:** the `next_wakeup` contribution moves to RL1 with the presenter; RL0 has nothing to wake for. The spec's
   RL0 list is updated to match.
5. **D5:** the screensaver window (`SCREEN_SAVER_WINDOW`) is excluded from rootless top-levels.
6. **D6:** decision 12 holds: a close request never kills. `CloseRequested` sends `WM_DELETE_WINDOW` when listed and
   otherwise does nothing; only `ForceKill`, sent by the host after a repeated close and an explicit confirmation,
   kills. The spec's §6.4 and §10 are fixed to match.
7. **D7:** `ScreenLayout` in RL0 accepts exactly one host output and rejects any other count; per-output RandR
   and multiple outputs are RL2 (after the second codex review: a bounding-box resize would misreport the first
   RandR output).
8. **D8:** `HostRequest::PopupDismissed` is left out of RL0 until the SetZero interface review (RL3).

## Size

| Task | Code | Tests |
|---|---|---|
| 1 types, mode, tail slot, reset | 330 | 80 |
| 2 auto-redirect | 90 | 90 |
| 3 reconcile | 420 | 430 |
| 4 WindowInfo | 380 | 330 |
| 5 WM policy | 420 | 380 |
| 6 host requests + acks | 380 | 420 |
| 7 input + scope + activation | 260 | 380 |
| 8 transients + status | 80 | 90 |
| **Total** | **~2,350** | **~2,200** |

P1 (own PR, before RL0): ~60 code + 60 test lines.

Order of commits: P1 merged → 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8. Each commit's gate is green on its own.
- 3 needs 2 (top-levels must be redirected before they are tracked).
- 4 needs P1.
- 5's popup/WM_STATE needs 3.
- 6's clamping needs 4's `SizeHints`.
- 7's popup activation rule needs 3.
- Tasks 4 and 5 can swap.
