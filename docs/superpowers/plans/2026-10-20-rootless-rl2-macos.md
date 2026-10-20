# Rootless RL2 (macOS) — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** milestone RL2 of `specs/2026-10-19-rootless-core-design.md` (r3, §9): the first user-visible rootless
step. On macOS every top-level X window becomes its own `NSWindow`. The window has a `CAMetalLayer`, a per-window
swapchain from RL1's presenter, native move/resize/minimize/close/fullscreen, focus by activation, and input in
window-local coordinates through RL0's mapping. The app bundle starts rootless and `just macos-run` stays rooted
(decision 1).

**Smallest useful scope.** Follow-ups (list at the end): per-output RandR (D1), runtime display changes (D4), SHAPE
(D5), per-screen pacing (D3), root screenshots (RL1b), window menus and fancy decorations, pointer motion outside our
windows, `_NET_WM_MOVERESIZE` resize.

**Architecture:**
- Three layers, as RL1 placed them:
  1. `yserver-core::rootless` is untouched.
  2. The OS-neutral surface platform and presenter (`crates/yserver/src/{surface,rootless}`) gain a rootless frame
     clock, cursor and RandR behaviour (Task 2), two presenter→host hooks that a layer-backed surface needs (Task 3),
     and, in a new `rootless/native/`, the bookkeeping every Global-policy host shares: coordinates, echo
     suppression, style mapping, focus/pointer/close state, frame pacing (Tasks 4-7). All of it is tested on Linux.
  3. `crates/yserver/src/macos/rootless/` is the AppKit half (Tasks 9-13).
- **Threads:** the core thread owns the presenter and `MacRootlessHost`; the main thread owns every `NSWindow`; the
  CVDisplayLink thread ticks the frame clock.
- **Crossings:** core→main only through `DispatchQueue::main().exec_async`, as `MainQueueUi` does (`macos/app.rs:350-416`);
  main→core only through the `ShellLink` channel (`surface/link.rs:39`). The core thread never waits on the main thread;
  `exec_sync` stays confined to the clipboard bridge's own thread (`macos/pasteboard.rs:14-18`), as today.

**Branch:** `feat/rootless-rl2`, cut from the rebased RL0/RL1 head of `feat/rootless-rl0` (82827610, on master with
PR #272), with a draft PR to master so macOS CI runs on every push (D10, decided).
Written against `e358edf6`; revised after codex review against `82827610` (the rebase moved `surface/platform.rs`
and `surface/swapchain/mod.rs` by 3 lines; those anchors are updated).

**Status:** revised after codex review 1 (warp across displays, state feedback, MOVERESIZE resize, decisions).

## Facts checked against the tree (e358edf6; touched anchors re-checked at 82827610)

| What | Where |
|---|---|
| Rooted macOS: delegate opens ONE window, starts the core thread with a `WindowSpec` | `macos/app.rs:251-319`; `surface/server.rs:41`, `run_windowed` :111 |
| App quits when its last window closes; close button asks to quit | `macos/app.rs:169-172`; :178-181 |
| `MainQueueUi` targets `windows().firstObject()` for title, size and cursor | `macos/app.rs:356-366`, :399-415; `macos/cursor.rs:58-64` |
| Window = `NSWindow` + `CAMetalLayer` (contentsScale, drawableSize, opaque, gravity topLeft) + `YserverView` | `macos/window.rs:23-67` |
| Metal surface on the core thread; `CATransaction::flush` commits the driver's layer writes after each swapchain change | `macos/window.rs:90-114`; `surface/swapchain/mod.rs:643-649` |
| Rebuild takes `caps.current_extent` unless it is `u32::MAX` (rooted and rootless alike) | `surface/swapchain/mod.rs:665-680`; `rootless/window_swapchain.rs:183-243` |
| Rootless GPU copy is 1:1, clamped, clearing the rest; no copy scale | `rootless/window_swapchain.rs:452-505`, `record_backing_copy` :800 |
| Input: view → `ShellInput` → `RawPointer` → `x_position` (root-clamped, view = root) | `macos/view.rs:19-144`; `macos/input.rs:121-140`, :174-187; `surface/input/pointer.rs:28-40` |
| Keys: one app-wide local key monitor, every key to X (Cmd-Q too) | `macos/input.rs:74-93`; `macos/app.rs:1-10` |
| `ShellLink`: `post`, `post_frame`, `send_input`; **no** way to send `Message::Rootless` | `surface/link.rs:94`, :103, :123 |
| Core: `Message::Rootless` → `host_request`; `HostInputEvent::Rootless` → `map_input` | `core_loop/run.rs:2794`; :4305 |
| `HostRequest` / `RootlessInput` / `HostOutput` | `rootless/types.rs:214`, :253, :205 |
| `ScreenLayout`: exactly one output → `host_screen_resized` (no backend storage resize) | `rootless/host.rs:345-359`; `core_loop/run.rs:3574` |
| `RootlessHost` (`create_surface` :83, `first_frame` :90, `frame_wanted` :95, `surface_released` :98) | `rootless/host.rs:79` |
| GPU tick: surface after `SetupDone`, present, `Shown` = delivered, `NotReady` → wait for `Ready` | `rootless/gpu.rs:78-154` |
| Presenter `start` with a `CoordinatePolicy`; `Global { origin }` is not read by the core | `rootless/mod.rs:353-362`; core `rootless/mod.rs:48-51` |
| **Gap:** with zero outputs the scene never uploads/shows the cursor plane (`fits` needs an output), so `flush_host_cursor` never sends a cursor | `surface/platform.rs:347-349`, :414-425; `scene/cursor.rs:577-631` |
| Same bytes the scene would upload: `CursorRecord.bgra_bytes` | `portable/cursor.rs:680`, :1077-1104 |
| **Gap:** Present MSC/UST and idle arming read `outputs.first()` only; with zero outputs MSC stays 0 and a parked NotifyMSC never fires | `surface/present.rs:37-52`; `core_loop/run.rs:3128-3197` |
| RandR on the surface platform: one `WINDOW-0` output at `output_size` (= root with zero outputs) | `surface/present.rs:61-75`, :122-175; `surface/platform.rs:114-139` |
| **Gap:** `RRSetCrtcConfig` → `request_window_size` → `set_content_size` on `firstObject()`, which in rootless would be an X window | `surface/platform.rs:567-602`; `macos/app.rs:399-415` |
| Pointer clamp: root extent, then `crtc_root_rects` = `output_layouts()` (empty with zero outputs) | `portable/pointer.rs:594-603`, :525-533 |
| Display link: one CVDisplayLink over the active displays ticking a `FrameClock`; one tick hook only | `macos/display_link.rs:103-145`; `surface/clock.rs:82-84` |
| `--scale`: copy factor F and X px per point `xpp = backing / F` | `surface/resize.rs:24-38`; `macos/scale.rs:30-75` |
| `--rootless` needs `--headless` today; the bundle is detected and sets `pick_free_display` | `launch.rs:178-187`, :281-284; `bin/yserver.rs:37` |
| Bundle needs macOS 26 (`LSMinimumSystemVersion`) | `Justfile:213-214` |
| objc2 0.6.5 / app-kit 0.3.2 / quartz-core 0.3.2 (all default features): `setAlphaValue`, `setLevel`, `setIgnoresMouseEvents`, `addChildWindow_ordered`, `orderWindow_relativeTo`, `toggleFullScreen`, `miniaturize`, `performWindowDragWithEvent`, `setTabbingMode`, `setAnimationBehavior`, `setContentResizeIncrements`, `CALayer::setMagnificationFilter` + `kCAFilterNearest` | `crates/yserver/Cargo.toml:52-57`; registry `NSWindow.rs:1381`, :1327, :2136, :1650, :1109, :1472, :978, :1782, :1912, :1458, :906; `CALayer.rs:601`, :1227 |
| Layout rule 4: `surface/` and `rootless/` must not match `objc2\|AppKit\|\bMetal\|target_os` (comments included) | `tools/check-layout.sh:52-57` |
| macOS CI: clippy + tests + KosmicKrisp `--ignored`, **only on PRs and master pushes** | `.github/workflows/ci.yml:2-6`, :165-208 |

## Global constraints

- **Rooted is unchanged.** `macos/app.rs`'s rooted path, `run_windowed`, `SurfaceOutput` and `YserverView` keep their
  behaviour. Rootless code lives in new files and enters through one `match` on the shell mode. Proof: the existing
  `surface/tests.rs` suite stays green and unedited, and jos's rooted smoke on eiger (`just macos-run`) is unchanged.
- **Linux gate per task** (each command under `env -u RUSTC_WRAPPER`):
  1. `cargo +nightly fmt --all`. It formats the cfg'd-out macOS modules too.
  2. `cargo clippy --workspace --all-targets -- -D warnings`
  3. `cargo clippy --workspace --all-targets --features xdmcp -- -D warnings`
  4. `cargo test --workspace --all-targets --locked`
  5. `YSERVER_ALLOW_SOFTWARE_VULKAN=1 VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json cargo test --workspace
     --locked --no-fail-fast -- --ignored`
  6. `tools/check-layout.sh`
- **macOS gate ("the gate" in Tasks 9-13).** Linux never compiles `src/macos/`, so there the Linux gate only proves
  nothing else broke. On eiger: `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace
  --all-targets --locked`, and the KosmicKrisp ignored suite (`ci.yml:204-208`). CI runs them only once a PR is open
  (D10).
- **Commits:** one per task, by jos. Linux tasks (1-8) after the gate; macOS tasks (9-13) after the gate **and** jos's
  eiger smoke for that task (user-visible: no commit before HW smoke). Trailer `Co-Authored-By: Claude Opus 5.5 (1M
  context) <noreply@anthropic.com>`; never a session URL.
- **Oracle.** A spec rule (§4.0, §4.2.1, §4.4, §5, §6, cited per test), AppKit's documented coordinate model (points,
  bottom-left origin, y up), or an existing tested function as reference (`x_position`, `window_randr`,
  `FrameClock`). Motif decoration bits come from `../xserver/hw/xwin`'s MWM definitions, read at implementation time.
- **Red first, mutation named and run once** (Linux tasks).
- **No env switches, no kill-switches.** The mode comes from `--rooted`/`--rootless` and the bundle default.

## Module layout

| File | Holds | ~lines |
|---|---|---|
| `rootless/native/mod.rs` | re-exports; doc: shared by Global-policy window-system hosts (no OS names, rule 4) | 20 |
| `rootless/native/space.rs` | `GlobalSpace`: display union, X ⇄ native points, warp point | 130 |
| `rootless/native/geometry.rs` | `HostSerials`, `GeometrySync` (§4.2.1 rule 4, echo suppression) | 140 |
| `rootless/native/style.rs` | `NativeStyle`, `native_style`, `level_for`, `content_limits`, `NetStateSync` | 170 |
| `rootless/native/seat.rs` | `FocusTracker`, `PointerTracker`, `CloseTracker`, `restack_requests` | 190 |
| `rootless/native/pacing.rs` | `FramePacer` (wanted ids → `Ready` per tick) | 50 |
| `rootless/native/tests.rs` | pure tests | 650 |
| `macos/rootless/mod.rs` | `MacRootlessHost` (core thread), `LayerTable`, `RootlessUi` | 230 |
| `macos/rootless/shell.rs` | `RootlessShell` (main thread): events → NSWindows, requests out | 420 |
| `macos/rootless/window.rs` | NSWindow + CAMetalLayer per top-level from a `NativeStyle` | 160 |
| `macos/rootless/view.rs` | `RootlessView` (mouse, enter/exit, cursorUpdate) | 170 |
| `macos/rootless/delegate.rs` | `RootlessWindowDelegate` (one per window, ivar `id`) | 150 |

### Task 1: Mode selection (`--rooted` / `--rootless`, bundle default)

**Files:** `launch.rs`, `bin/yserver.rs:37`, `lib.rs:298-316`.

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShellMode { Rooted, Rootless }
// LaunchOptions: `rooted: bool` (new flag), `rootless` (exists), `from_bundle: bool` (not argv; set like
// `pick_free_display`, bin/yserver.rs:37).
/// Decision 1 + spec §7. `window_shell` = this build has a window shell (`cfg!(target_os = "macos")`, passed by lib.rs).
pub fn shell_mode(o: &LaunchOptions, window_shell: bool) -> Result<ShellMode, String>;
```

Rules (replacing `validate_rootless`): `--rooted` + `--rootless` → error; an explicit flag wins; with no flag,
rootless iff `window_shell && from_bundle && !headless`, else rooted; `--rootless` without `--headless` and without a
window shell → error (today's KMS message); `--rootless-record` needs `--rootless --headless` (the recording host is
headless-only); `--rooted --headless` is accepted.

`lib.rs` calls `shell_mode` in all three `run`s; `macos::run` takes the mode.

**Tests** (`launch.rs` tests, normal suite):
- `shell_mode_follows_decision_1`: bundle + window shell → Rootless; no bundle → Rooted; bundle + `--headless` →
  Rooted. Mutation: default to Rootless everywhere.
- `explicit_flags_win_and_conflict`: `--rooted` in a bundle → Rooted; both flags → Err.
- `rootless_without_a_shell_needs_headless`: `window_shell=false`, `--rootless` → Err; with `--headless` → Ok.
  This replaces `rootless_flags_parse_and_need_headless` (`launch.rs:1257`) and keeps its assertions.
- `record_needs_headless_rootless`: `--rootless --rootless-record d`, window shell, no `--headless` → Err.

**Gate**, then commit `feat(launch): --rooted/--rootless with a rootless default for the app bundle`. ~70 + 60 test.

### Task 2a: A rootless Present clock

**Files:** `surface/platform.rs`, `surface/present.rs`.

`SurfacePlatform.clock: Option<Arc<FrameClock>>` (`set_clock`); `fn msc_clock(&self) -> Option<&FrameClock>` =
`outputs.first().map(SurfaceOutput::clock)`, else `self.clock`; `surface_ust_msc`, `arm_surface_msc` and the RandR
refresh (`present.rs:37-75`) use it.

**Tests** (`surface/tests.rs`, lavapipe, modelled on `surface_present_clock_follows_ticks_and_wakes_an_armed_target_once`
:526):
- `rootless_clock_drives_msc_without_outputs`: headless backend + `set_clock`. Three ticks → `(msc, ust) == (3, ust3)`.
  `arm_surface_msc(&[5])` → `Ok(1)`. Ticks 4 and 5 → one `PlatformEvent`. Mutation: keep `outputs.first()`.
- `headless_without_a_clock_is_unchanged`: no clock → `(0, 0)` and `Ok(0)`, as today.

**Gate**, commit `feat(surface): a frame clock for Present without outputs`. ~25 + 50 test.

### Task 2b: The cursor reaches the shell without outputs

**Files:** `portable/cursor.rs`: `pub(crate) fn sprite_key(&self) -> Option<(u64, (u16, u16))>`, the effective
record's `(version, hot)` or `None` when `effective_cursor_hidden()`, copying no bytes. `surface/platform.rs:414-425`:
when rootless, `flush_host_cursor` compares `sprite_key()` with `cursor.sent` and only on a change builds a
`CursorImage` from `get_active_cursor_image()` (the bytes `portable/cursor.rs:680` hands the scene).

Animated cursors are unverified here: whether `effective_cursor_xid` names a `cursor_records` entry for an animated
cursor is checked at implementation; if not, they show the arrow until a follow-up.

**Tests** (lavapipe, rootless fixture + `RecordingUi`, modelled on :954, :991, :1057):
- `rootless_shell_gets_the_root_cursor`: after `rootless::start` and one `maybe_composite`, one `Cursor(Some(v, hot,
  size))` equal to the root cursor record. Mutation: drop the rootless branch (no call at all).
- `rootless_define_and_hide_cursor`: DefineCursor on a mapped window + pointer inside → the new version; XFIXES
  HideCursor → `Cursor(None)`; ShowCursor → back; two idle tails send nothing. Mutation: send every tail.

**Gate**, commit `feat(surface): the host cursor follows the sprite in rootless mode`. ~45 + 90 test.

### Task 2c: RandR configuration requests fail in rootless

**Files:** `surface/platform.rs:567-590` and :1005-1008: when rootless, `apply_window_crtc_config` and
`set_logical_screen_size` return `Err` (spec §5: a real error status, never a silent lie), so `request_window_size`
is never reached. RL2 sends no `ScreenLayout` (the union is fixed at startup, D4); the core's `ScreenLayout` path,
which resizes no backend storage (fact row), is the runtime-display follow-up's to fix.

**Tests** (lavapipe): `rootless_refuses_randr_configuration`: `RRSetCrtcConfig` → status `RRSetConfigFailed` and no
`ContentSize` on the RecordingUi; `RRSetScreenSize` → an error and the root keeps its size. Mutation: drop the
rootless check.

**Gate**, commit `fix(surface): rootless refuses RandR configuration`. ~20 + 50 test.

### Task 3: Presenter hooks around a swapchain rebuild

A layer-backed surface reports `currentExtent` from its layer [A: KosmicKrisp, see Task 9's eiger step]. The
existing rooted path relies on the shell setting `drawableSize` *before* the core rebuilds (`macos/resize.rs:44-49`)
and on a commit after it (`swapchain/mod.rs:643-649`). For a client-driven resize, a rootless host cannot be ahead of
the core, so the presenter tells it.

**Files:** `rootless/host.rs` (two default no-op hooks), `rootless/gpu.rs`, and
`rootless/window_swapchain.rs`:
- `fn rebuild_due(&self, extent: vk::Extent2D, gen: u64) -> bool`, the predicate `present` uses at :259-264;
- a `rebuilt: bool` flag taken by `take_rebuilt()`.

```rust
/// Before a swapchain (re)build at `extent` backing pixels, on the core thread.
fn surface_resizing(&mut self, _id: ToplevelId, _extent: (u32, u32)) {}
/// After it, whatever the outcome: commit what the driver changed on the native surface.
fn surface_rebuilt(&mut self, _id: ToplevelId) {}
```

**Tests** (`rootless/tests/gpu.rs`, lavapipe; `RecordingHost` logs the two new `Call`s):
- `rebuild_hooks_bracket_every_swapchain_change`: map W 40×30 bw 2, `SetupDone` + tail → `Resizing(id, (44, 34))`,
  the present, then `Rebuilt(id)`, in that order; resize to 60×40 → `Resizing(id, (64, 44))` before the next
  present; a plain damage frame → no hook. Source: §4.0 (native content = outer extent). Mutation: call
  `surface_resizing` after the present.

**Gate**, commit `feat(rootless): host hooks around swapchain rebuilds`. ~35 + 60 test.

### Task 4: `GlobalSpace` and window-local pointer positions

**Files:** `rootless/native/space.rs`; `surface/input/pointer.rs` gains `local_position`.

```rust
/// Points, bottom-left origin, y up (the window system's global space).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct NativeRect { pub x: f64, pub y: f64, pub width: f64, pub height: f64 }

/// X root = bounding box of every display (spec §5 Global, D1); one scale S = X px per point (decision 6, D2).
pub(crate) struct GlobalSpace { left: f64, top: f64, primary_top: f64, scale: f64, root: (u16, u16) }
impl GlobalSpace {
    /// `displays[0]` is the primary (`NSScreen.screens` first: origin (0,0), menu bar); `primary_top` = its height.
    pub(crate) fn new(displays: &[NativeRect], scale: f64, max_side: u16) -> Self;
    pub(crate) fn root(&self) -> (u16, u16);
    pub(crate) fn scale(&self) -> f64;
    /// X outer corner + native content extent (w+2bw, h+2bw) → native content rect.
    pub(crate) fn to_native(&self, origin: (i32, i32), extent: (u16, u16)) -> NativeRect;
    /// Native content rect → X outer corner and extent, whole X pixels (round).
    pub(crate) fn to_x(&self, content: NativeRect) -> ((i32, i32), (u16, u16));
    /// Root pixel → Quartz global point (origin the primary's top-left, y down) for the pointer warp.
    pub(crate) fn to_warp_point(&self, root: (i32, i32)) -> (f64, f64);
}
/// View points (bottom-left, y up) → backing pixels from the content's top-left; unclamped
/// (a drag may leave the window), spec §4.4 pointer-local.
pub(crate) fn local_position(raw: &RawPointer) -> (f64, f64);
```

Formulas:
- `x_pt = left + x/S`;
- `y_pt(bottom) = top − (y + h)/S`;
- `to_x` inverts both: `x = (x_pt − left)·S`, `y = (top − y_pt(top))·S`;
- the root is `⌈(right−left)·S⌉ × ⌈(top−bottom)·S⌉`, clamped to `max_side`;
- warp: `qx = left + x/S`, `qy = y/S − (top − primary_top)`. The root's (0,0) is the union's top-left and the Quartz
  origin the primary's top-left; they differ by `top − primary_top` when a display reaches above the primary, and by
  `left` when one lies to its left.

**Tests** (`native/tests.rs`):
- `union_of_two_displays_is_the_root`: primary (0,0,1512,982) and a taller second display at (1512,0,1920,1200), at
  S=1 → root 3432×1200, `top = 1200`; at S=2 → 6864×2400. Mutation: use the primary's height for the top (gives
  982).
- `x_to_native_and_back_is_identity`: every corner of both displays, bw 0 and 5, S ∈ {1, 2} → `to_x(to_native(o, e))
  == (o, e)`. Mutation: drop the `+h` in the y formula.
- `outer_corner_is_the_content_origin`: X (0,0) maps to the union's top-left point (§4.0, §5 table).
- `warp_point_offsets_by_the_union_top_left`, primary P = (0,0,1512,982) in every case; Source: Quartz y = primary
  height − Cocoa y, the convention `macos/cursor.rs:198-202` uses. Mutation: drop the `top − primary_top` term (fails
  "above" and "left").

  | Layout (second display, Cocoa) | Root | Root → warp point | P's top-left in root |
  |---|---|---|---|
  | P alone, S=2 | 3024×1964 | (10,20) → (5, 10) | (0,0) → (0,0) |
  | above: (0,982,1920,1080), S=1 | 1920×2062 | (10,20) → (10, −1060); S=2: (10,20) → (5, −1070) | (0,1080) → (0,0) |
  | left: (−1920,0,1920,1080), S=1 | 3432×1080 | (0,0) → (−1920, −98) | (1920,98) → (0,0) |
  | below: (0,−1080,1920,1080), S=1 | 1920×2062 | (10,1000) → (10, 1000) | (0,0) → (0,0) |
- `native_to_root_on_every_side` (`to_x`, S=1, same three layouts; content rects in Cocoa points): above:
  (100,1500,400,300) → origin (100,262); left: (−1800,200,400,300) → (120,580); below: (100,−500,400,300) →
  (100,1182). Each is also `to_native`'s inverse. Mutation: measure y from the primary's top instead of the union's
  (fails "above" and "left").
- `local_position_flips_and_scales_without_clamping`: view height 100 pt: (0,100) → (0,0); (10,0) at S=2 →
  (20,200); (−3,110) → (−3S, −10S). Reference for in-range values: `x_position` (`pointer.rs:28`). Mutation: clamp
  like `x_position`.

**Gate**, commit `feat(rootless): global display space and window-local pointer positions`. ~155 + 170 test.

### Task 5: `GeometrySync`, serials and echo suppression

```rust
pub(crate) struct HostSerials(u64);
impl HostSerials { pub(crate) fn next(&mut self) -> HostSerial; }

/// What X has or was asked for, per window. The native window is only ever compared with this.
pub(crate) struct GeometrySync { origin: (i32, i32), extent: (u16, u16), last_sent: Option<HostSerial> }
pub(crate) struct NativeTarget { pub origin: Option<(i32, i32)>, pub extent: Option<(u16, u16)> }
impl GeometrySync {
    pub(crate) fn mapped(g: &Geometry) -> Self;
    /// `Configured` (§4.2.1 rule 4): apply exactly `client_fields`; `None` = leave the native window.
    pub(crate) fn configured(&mut self, g: &Geometry, acked: Option<HostSerial>,
                             client_fields: GeometryFields) -> Option<NativeTarget>;
    /// The native content is now `(origin, extent)` (user drag, system constraint or our own setFrame):
    /// the requests to post, each with a fresh serial. Equal to what X has → nothing (echo).
    pub(crate) fn native_changed(&mut self, id: ToplevelId, origin: (i32, i32), extent: (u16, u16),
                                 serials: &mut HostSerials) -> Vec<HostRequest>;
}
```

This replaces a static "applying" flag. It is the spec §4.4 feedback guard, applied at the host edge: a native
notification caused by our own `setFrame` reports what `configured` just recorded, so nothing is posted. A frame the
system constrained (title bar under the menu bar) differs, so it is posted and X follows the native window.

**Tests**, one per §4.2.1 rule:
- `own_move_acked_needs_nothing`: `native_changed` → `Moved(7)`. `configured(acked 7, ∅)` → `None` (rule 4).
- `client_resize_coalesced_with_host_move`: after `Moved(7)`, `configured(acked 7, WIDTH|HEIGHT)` → target with the
  extent only (rule 3).
- `newer_request_in_flight_ignores_older_ack`: `Moved(7)`, `Moved(8)`, `configured(acked 7, ∅)` → `None` (rule 4).
- `client_move_applies_and_does_not_echo`: `configured(None, X|Y)` → target origin. `native_changed` with that origin
  → no request. Mutation: compare with the previous native frame instead of what X has (an echo `Moved` appears).
- `size_hint_clamp_snaps_the_native_window`: `Resized(9)` 300×200, `configured(acked 9, WIDTH|HEIGHT)` with 290×200
  → target extent 290×200 (rule 5); the echo posts nothing.
- `system_constraint_is_reported`: target origin y=−20, native reports y=25 → `Moved { y: 25 }`.
- `live_drag_posts_every_step`: three `native_changed` → three `Moved` with increasing serials (decision 4; the core
  coalesces per iteration).

**Gate**, commit `feat(rootless): native geometry sync with serial acks`. ~140 + 170 test.

### Task 6: Style mapping, content limits and native window state

```rust
/// Below = one under `NSNormalWindowLevel` (AppKit names no such level; `NSWindowLevel` is an integer, D13).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum NativeLevel { Below, Normal, Floating, PopUp }
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct NativeStyle {
    pub titled: bool, pub closable: bool, pub miniaturizable: bool, pub resizable: bool,
    pub level: NativeLevel, pub can_activate: bool, pub child_of: Option<ToplevelId>, pub fullscreen: bool,
}
pub(crate) fn native_style(kind: ToplevelKind, info: &WindowInfo) -> NativeStyle;
pub(crate) fn level_for(layer: Layer) -> NativeLevel;          // Above → Floating, Below → Below, Normal → Normal
pub(crate) struct ContentLimits { pub min: Option<(u16, u16)>, pub max: Option<(u16, u16)>, pub inc: Option<(u16, u16)> }
/// WM_NORMAL_HINTS in native content (outer) X pixels: min/max + 2bw, base for a missing min (ICCCM §4.1.2.3).
pub(crate) fn content_limits(hints: &SizeHints, border: u16) -> ContentLimits;

/// What the native window is, read after any change, whoever caused it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct NativeFlags { pub zoomed: bool, pub fullscreen: bool, pub minimized: bool, pub level: NativeLevel }
/// `_NET_WM_STATE` as the host last reported it. The core never writes it on a client's request (it only emits a
/// `WmRequest`, `wm.rs:323-378`) and computes the next toggle from the property (`info.rs:376`), so every native
/// change must come back as `StateChanged` (`host.rs:321-344`) or a later toggle acts on stale state.
pub(crate) struct NetStateSync { reported: NetWmState }
impl NetStateSync {
    pub(crate) fn mapped(client: NetWmState) -> Self;
    /// Client-owned fields (`skip_taskbar`, `modal`) from `InfoChanged(STATE)`; they ride along in every report.
    pub(crate) fn client_state(&mut self, client: NetWmState);
    /// The full state to send when `native` differs from what X has; `None` when equal (no echo).
    pub(crate) fn native(&mut self, id: ToplevelId, native: NativeFlags) -> Option<HostRequest>;
}
```

Rules (spec §8.1; XWin `winApplyHints` as precedent, `winmultiwindowwm.c:1723-1959`):
- **Managed:** titled/closable/miniaturizable unless `Decorations::None` (a `Subset` is titled iff it has the TITLE
  bit); resizable unless `min == max`; level `level_for` of `state.above`/`state.below`; `child_of = transient_for`;
  `fullscreen` (collection behaviour) for Normal/Dialog types.
- **Popup:** untitled, `PopUp` level, `can_activate = false`, no child relation (the level keeps it on top).
- **State:** `zoomed` → `maximized_vert && maximized_horz`; `fullscreen`; `minimized` → `hidden`; level Floating →
  `above`, Below → `below`, Normal → neither. A window maximized in one direction only by its client reports both
  after a native zoom (AppKit zoom has no single axis).

**Tests:** `managed_defaults`, `motif_none_is_borderless`, `fixed_size_is_not_resizable`,
`transient_dialog_is_a_child`, `above_floats`, `below_sinks` (level `Below`), `popup_never_activates` (§6.2),
`limits_add_the_border_and_fall_back_to_base`. Motif vectors from the XWin definitions (Global constraints). Mutation
per test: drop the rule it names. State (EWMH §5.7 atoms; the core's `net_wm_state_atoms`, `host.rs:195`):
- `user_zoom_reports_maximized`: `native(zoomed)` → `StateChanged { maximized_vert, maximized_horz }`; the same flags
  again → `None`; unzoom → both cleared.
- `client_request_is_reported_back`: a `SetMaximized(true)` the host applied shows up as `native(zoomed)` and yields a
  `StateChanged`, exactly as a user zoom does. Mutation: report only user-initiated changes (the client path then
  leaves `_NET_WM_STATE` stale).
- `layers_report_above_and_below`: Floating → `above`; Below → `below`, not `above`; Normal → neither.
- `fullscreen_and_minimize_report`: user fullscreen → `fullscreen`; minimize → `hidden`, other fields kept.
- `client_fields_ride_along`: `skip_taskbar` from `client_state` survives a native zoom report. Mutation: build the
  report from native flags alone.

Core-side (yserver-core test only, no core code change), `rootless/tests/wm.rs`:
`toggle_after_state_changed_uses_the_new_state`: a client `_NET_WM_STATE` toggle MAXIMIZED_VERT+HORZ →
`WmRequest::SetMaximized(true)`; host `StateChanged { maximized }` → property set; the same toggle again →
`SetMaximized(false)`; same for ABOVE and BELOW (`SetLayer`). Mutation: drop the `set_words` in `StateChanged`.

**Gate**, commit `feat(rootless): native window style and state reporting from WindowInfo`. ~170 + 190 test.

### Task 7: Focus, pointer, close and stacking state; frame pacing; `ShellLink::request`

```rust
pub(crate) struct FocusTracker { active: Option<ToplevelId>, resign_pending: bool }
impl FocusTracker {
    pub(crate) fn became_key(&mut self, id: ToplevelId) -> Option<HostRequest>; // Activated, once
    pub(crate) fn resigned_key(&mut self);                                    // only marks
    /// After the key change settled (next main-queue turn): `key` = our window that is key now, if any.
    pub(crate) fn settle(&mut self, key: Option<ToplevelId>) -> Option<HostRequest>; // Deactivated
    pub(crate) fn app_deactivated(&mut self) -> Option<HostRequest>;
}
pub(crate) struct PointerTracker { inside: Option<ToplevelId>, buttons: u8, leave_pending: bool }
impl PointerTracker {
    pub(crate) fn moved(&mut self, id: ToplevelId, local: (f64, f64), time: u32) -> RootlessInput; // Enter on a new id
    pub(crate) fn exited(&mut self, id: ToplevelId, time: u32) -> Option<RootlessInput>;  // deferred while a button is down
    pub(crate) fn button(&mut self, pressed: bool, time: u32) -> Option<RootlessInput>;   // last release flushes a deferred Leave
    pub(crate) fn inside(&self) -> Option<ToplevelId>;
}
pub(crate) enum CloseAction { Request, ConfirmKill }
pub(crate) struct CloseTracker { asked: bool }
impl CloseTracker { pub(crate) fn clicked(&mut self) -> CloseAction; }
/// Native front-to-back vs X bottom-to-top → `(id, above)` restacks that make X match (§6.4).
pub(crate) fn restack_requests(native_front_to_back: &[ToplevelId], x_bottom_to_top: &[ToplevelId])
    -> Vec<(ToplevelId, Option<ToplevelId>)>;
/// Ids whose frame is wanted, answered with `Ready` on the next display tick (D3, RL1 D7).
pub(crate) struct FramePacer { wanted: Mutex<Vec<ToplevelId>>, link: Arc<ShellLink> }
impl FramePacer { pub(crate) fn want(&self, id: ToplevelId); pub(crate) fn tick(&self); }
// surface/link.rs
pub(crate) fn request(&self, request: HostRequest);   // Message::Rootless, dropped before connect
```

`PointerTracker` input origins come from `devices::pointer_origin()`.

**Tests:**
- `switching_between_our_windows_never_deactivates`: key A, resign A, key B, `settle(Some(B))` → exactly
  `[Activated A, Activated B]` (§6.2: focus moves without a None in between). Mutation: post Deactivated on resign.
- `app_switch_deactivates_once`: resign, `settle(None)` → one `Deactivated`.
- `leave_waits_for_the_button` (§4.4: `PointerLeave` releases buttons, so never mid-drag): down, exit A → `None`;
  up → `PointerLeave`. Mutation: Leave at exit.
- `enter_then_motion`: the first `moved` on B is `PointerEnter`, the next `PointerMotionLocal`.
- `second_close_asks_to_kill` (decision 12): `Request`, then `ConfirmKill`; per mapped lifetime (a remap restarts).
- `restack_moves_only_what_differs`: native [C, A, B], X [A, B, C] → restacks that, applied to a vector model,
  reproduce the native order; already equal → none.
- `pacer_answers_each_wanted_id_once`: want A, A, B; tick → `Ready A`, `Ready B` on the link; tick again → nothing.
  Mutation: no drain.
- `request_reaches_the_core_in_order_with_input` (`link.rs` tests): `send_input` then `request` → `HostInput` then
  `Rootless` on the channel.

**Gate**, commit `feat(rootless): native seat, close and stacking state, frame pacer, link requests`. ~240 + 220 test.

### Task 8: `run_rootless` on the surface platform

**Files:** `surface/server.rs`, `surface/platform.rs` (`set_clock` from 2a).

```rust
pub(crate) struct RootlessSpec {   // fields pub(crate)
    surface_extension: &'static CStr, root: (u16, u16) /* GlobalSpace::root() */, clock: Arc<FrameClock>,
    link: Arc<ShellLink>, ui: Arc<dyn ShellUi>, clients: Arc<AtomicUsize>, pasteboard: Box<dyn Pasteboard>,
    host: Box<dyn RootlessHost + Send>,
}
/// The zero-output backend a rootless shell runs on (testable without a shell).
pub(crate) fn rootless_backend(vk: Arc<VkContext>, spec: &RootlessSpec, layout: Option<String>)
    -> io::Result<XBackend<SurfacePlatform>>;
pub(crate) fn run_rootless(opts: launch::LaunchOptions, spec: RootlessSpec) -> io::Result<()>;
```

`run_rootless` = `serve(...)` with build `VkContext::new_with_options { wants_present: true, surface_extension:
Some(ext), .. }` + `rootless_backend`, setup `rootless::start(state, backend, host, CoordinatePolicy::Global { origin:
(0, 0) })`, and `on_display` → `ui.set_title`; then `link.disconnect()` and `ui.quit`, as `run_windowed` (:111-156).

**Tests** (lavapipe; headless surface extension, the RL1 `RecordingHost`):
- `rootless_backend_has_no_outputs_a_clock_and_the_root_size`: `output_layouts()` empty, one RandR output at `root`
  (`window_randr`), `msc_clock()` is the spec's. Mutation: build with a `SurfaceOutput`.
- `rootless_backend_presents_a_mapped_window`: `rootless::start` + map + fill → one present and `first_frame`.

**Gate**, commit `feat(surface): run_rootless, the zero-output server for a rootless shell`. ~90 + 70 test.

**macOS tasks (9-13):** none compiles on Linux; there the Linux gate only shows nothing else changed. Correctness is
on eiger.

### Task 9: `MacRootlessHost` and the first visible windows (no input yet)

**Files:** `macos/run.rs` (`run(opts, mode)`); `macos/app.rs` (`Ivars.mode`; `did_finish_launching` → `open_window`
when rooted or the new `start_rootless`; `terminate_after_last_window_closed` → `mode == Rooted`); new
`macos/rootless/{mod,shell,window}.rs`. The per-window delegate arrives in Task 10.

```rust
// macos/rootless/mod.rs, core thread
pub(super) struct MacRootlessHost { link: Arc<ShellLink>, layers: Arc<LayerTable>, pacer: Arc<FramePacer> }
pub(super) struct LayerTable(Mutex<HashMap<ToplevelId, LayerPtr>>);   // LayerPtr from macos/window.rs:90
impl RootlessHost for MacRootlessHost {
    fn handoff(&self, _: ToplevelId, _: u8) -> HandoffKind { HandoffKind::VulkanSurface }
    fn create_surface(&mut self, id: ToplevelId, vk: &VkContext) -> io::Result<vk::SurfaceKHR>; // metal_surface's call, on `layers[id]`
    fn submit_cpu(&mut self, frame: CpuFrame) { drop(frame) }          // never chosen
    fn first_frame(&mut self, id: ToplevelId);                         // main: alpha 1, accept mouse, key if managed
    fn on_events(&mut self, events: &[RootlessEvent]);                 // clone → exec_async → RootlessShell::apply
    fn frame_wanted(&mut self, id: ToplevelId) { self.pacer.want(id) }
    fn surface_resizing(&mut self, id: ToplevelId, extent: (u32, u32)); // layer.setDrawableSize in a CATransaction
    fn surface_rebuilt(&mut self, _: ToplevelId) { CATransaction::flush() }
    fn surface_released(&mut self, id: ToplevelId);                    // main: drop the NativeWindow, remove the layer
}
// macos/rootless/shell.rs, main thread (a thread_local RefCell, as macos/resize.rs:69 keeps SIZER)
pub(super) struct RootlessShell { space: GlobalSpace, windows: HashMap<ToplevelId, NativeWindow>,
    serials: HostSerials, focus: FocusTracker, pointer: PointerTracker, link: Arc<ShellLink>,
    layers: Arc<LayerTable>, input: Rc<ShellInput> }
struct NativeWindow { window: Retained<NSWindow>, layer: Retained<CAMetalLayer>,
    delegate: Retained<RootlessWindowDelegate>, kind: ToplevelKind, info: WindowInfo, border: u16,
    sync: GeometrySync, net: NetWmState, close: CloseTracker }
// macos/rootless/window.rs
pub(super) fn create(mtm: MainThreadMarker, style: NativeStyle, content: NSRect, scale: f64, depth: u8,
                     view: &NSView) -> (Retained<NSWindow>, Retained<CAMetalLayer>);
```

`start_rootless` (main thread): `GlobalSpace::new` from `NSScreen::screens(mtm)` frames and `S =
Metrics::of_main_screen(mtm).xpp` (D2); one `DisplayLink` + `FrameClock` whose tick hook is `pacer.tick()`, with the
`SoftwareTicker` fallback (`app.rs:85-100`); then the core thread with `run_rootless` (Task 8).

`RootlessShell::apply`, on `Mapped` (spec §4.5.1 macOS row [A]):
- the window from `NativeStyle`, `setReleasedWhenClosed(false)`, `setRestorable(false)`, `setTabbingMode(Disallowed)`;
  popups `setAnimationBehavior(None)`;
- the layer: `contentsScale = S`, `drawableSize` = backing extent, `opaque` iff depth 24 (depth 32: window not opaque,
  clear background), gravity topLeft, `magnificationFilter = kCAFilterNearest` (D2);
- `alphaValue = 0`, `setIgnoresMouseEvents(true)`, `orderFront`; `layers.insert(id, ptr)`, then post
  `FrameEvent::SetupDone { id }`.

`Unmapped` → `orderOut` only (`surface_released` frees the window); `InfoChanged(TITLE)` → `setTitle`.

`RootlessUi` (a `ShellUi`): `set_title` → a disabled "DISPLAY=:N" item in the app menu (`install_menu`,
`app.rs:419-438`), as there is no root window to title; `quit` = `MainQueueUi::quit`, factored into a shared fn;
`set_cursor`/`warp_pointer` in Task 11; `set_content_size` logs and does nothing (unreachable after 2c).

Pacing (D3): RL1's GPU path needs a `Ready` per frame after the first. `frame_wanted` → `FramePacer::want`, and the
next CVDisplayLink tick posts `Ready` on the CV thread. That is one hop and no main-queue round trip.

**eiger:** (1) the gate; (2) `cargo run --release --bin yserver -- :1 --rootless` with the Justfile environment,
then `DISPLAY=:1 xterm`, `xclock -update 1`, `xmessage hi`: each is its own window, no empty flash, xclock ticks,
closing a client closes its window; (3) the debug line in `WindowSwapchain::rebuild` (`caps.current_extent`/min/max,
composite alphas) and `vulkaninfo | grep -i maintenance1`, for R1/R2; (4) `just macos-run` unchanged.

**Gate** + eiger, commit `feat(macos): rootless windows, one NSWindow per top-level`. ~560 lines.

### Task 10: Input per window, focus

**Files:** `macos/rootless/view.rs` (`RootlessView`, ivars `{ id, input }`: `YserverView`'s mouse methods,
`view.rs:40-112`, plus `mouseEntered:`/`mouseExited:`, no live-resize hooks); `macos/rootless/delegate.rs`
(`windowDidBecomeKey:`/`windowDidResignKey:`); `macos/app.rs` (`applicationDidResignActive:` when rootless);
`macos/input.rs` (`ShellInput::{motion_local, button_local, exited_local}`).

The conversion uses `local_position` with the **global** S, not `scale::xpp(window)`, because the per-window xpp
changes across displays (D2). Each call builds `HostInputEvent::Rootless(...)` through `PointerTracker` and sends the
position before the button, in one `send_input` (as `input.rs:130-140`). Scroll keeps `Scroller` (position = the last
motion). Keys keep the app-wide monitor; X focus routes them.

Focus: `windowDidBecomeKey` → `FocusTracker::became_key` → `link.request(Activated)` + `ShellEvent::LockState`
(§4.4; replaces `focus_gained`); `windowDidResignKey` → `resigned_key`, then `exec_async(settle(keyWindow ∩ ours))`;
`applicationDidResignActive` → `app_deactivated` + `translator.release_all`, as `focus_lost` does.

`first_frame` makes a Managed window key (`makeKeyAndOrderFront`) when the app is active. Popups are borderless and
cannot become key, matching §6.2.

**eiger:** (1) the gate; (2) type in two xterms, switch by click; (3) Cmd-Tab away and back: no stuck keys (`xev`
shows the release); (4) `xev -event mouse`: Enter/Leave crossing two windows; (5) `xterm -bw 5`: a click on the
border and on the first content pixel reach the right window with the right coordinates (spec §10); (6) drag-select
out of an xterm and release outside: no stuck button.

**Gate** + eiger, commit `feat(macos): rootless pointer, keyboard and focus per window`. ~300 lines.

### Task 11: Cursor and warp

**Files:** `macos/cursor.rs` gains `set_global(mtm, image, scale, inside: bool)`. It builds at the global S, stores
`current`, and calls `set()` when `PointerTracker::inside()` is some. `RootlessView::cursorUpdate:` →
`apply_current` (exists, :118). `RootlessUi::warp_pointer` → `GlobalSpace::to_warp_point` → `CGWarpMouseCursorPosition`
+ re-associate, as `cursor.rs:198-207`, without the "inside the view" guard (spec §5: Global → native warp).

**eiger:** (1) the gate; (2) `xsetroot -cursor_name watch` changes the cursor over xclock, which inherits the root's; (3) the xterm I-beam
over xterm and the arrow over xclock; (4) a GTK app changes the cursor over links and entries; (5) `xdotool mousemove
100 100` warps to the union's top-left area; with a display above the primary, `xdotool mousemove 0 0` lands at that display's top-left (Task 4 "above").

**Gate** + eiger, commit `feat(macos): rootless cursor and pointer warp`. ~80 lines.

### Task 12: Native move, resize, close, minimize, fullscreen; X-driven configure

**Files:** `macos/rootless/delegate.rs`, `shell.rs`.
- **User → X:** `windowDidMove:`/`windowDidResize:` → `GeometrySync::native_changed(space.to_x(
  contentRectForFrameRect(frame)))` → `link.request(Moved/Resized)`, live during the drag (decision 4) [A: R4].
  Resize also sets the layer frame in a `CATransaction` with actions off (`macos/resize.rs:123-126`); the swapchain
  follows the backing through Task 3's hooks.
- **X → native:** `Configured` → `GeometrySync::configured` → `setFrame_display(frameRectForContentRect(
  space.to_native(..)), true)`. The echo is suppressed by Task 5. `Mapped` applies `content_limits`
  (`setContentMinSize`/`MaxSize`/`ResizeIncrements` in points). `InfoChanged(SIZE_HINTS)` re-applies them.
- **Placement** (§6.1): a Managed window without `USPosition|PPosition` is cascaded by AppKit
  (`cascadeTopLeftFromPoint:`) before order-in. The result goes through `native_changed` → `Moved`.
- **Close:** `windowShouldClose:` returns `false` and consults `CloseTracker`: `Request` → `CloseRequested`;
  `ConfirmKill` → an `NSAlert` naming the client (saying it does not support closing a window when
  `!info.protocols.delete_window`, and that all its windows close), then `ForceKill` (decision 12).
- **Window state back to X, whoever changed it** (Task 6 `NetStateSync`). After every native state change the shell
  reads `NativeFlags` (`isZoomed`, fullscreen from the delegate's enter/exit, `isMiniaturized`, the level it set) and
  posts what `NetStateSync::native` returns, so `_NET_WM_STATE` (and `WM_STATE` Iconic/Normal, decision 3) follows.
  Triggers: `windowDidMiniaturize:`/`Deminiaturize:`; `windowDidEnterFullScreen:`/`Exit` (green button, or our
  `toggleFullScreen:`); zoom: `windowShouldZoom:toFrame:` returns `true` and schedules a re-read on the next
  main-queue turn (AppKit posts no did-zoom notification), and every `windowDidResize:` re-reads `isZoomed` (a drag
  out of the zoomed frame unzooms; option-click on the green button zooms); level: right after our own `setLevel`
  (users have no level UI). Registry: `isZoomed` `NSWindow.rs:986`, `zoom` :990, `windowShouldZoom_toFrame` :2285,
  `NSNormalWindowLevel` :268.
- **`WmRequest` from clients** (§6.3): `Activate` → `makeKeyAndOrderFront`; `Close` → `performClose:`; `Minimize` →
  `miniaturize:`; `SetFullscreen(b)` → `toggleFullScreen:` when it differs; `SetMaximized(b)` → `zoom:` when
  `isZoomed` differs; `SetLayer(l)` → `setLevel(level_for(l))` (Below = `NSNormalWindowLevel − 1`, D13). Each reports
  back through the bullet above; none writes `_NET_WM_STATE` optimistically. At map, `info.state` fullscreen/maximized
  are applied after `first_frame` the same way.
- **`_NET_WM_MOVERESIZE`:** `BeginMove` → `performWindowDragWithEvent:` with the view's last mouse-down event, only
  while that button is still down [A]. `BeginMove { button: 0 }` (keyboard move, direction 10) and every
  `BeginResize` (directions 0-7 and keyboard 9, `wm.rs:381-405`) are logged and dropped: AppKit has no public call that starts a native resize drag. A known RL2 limitation of an
  advertised operation (RL0 lists `_NET_WM_MOVERESIZE` in `_NET_SUPPORTED`, `wm.rs:39`); the client has ungrabbed
  (EWMH §4.3), so nothing hangs, the edge drag just does nothing. Recorded in the spec (Task 14); follow-up F8.

**eiger:** (1) the gate; (2) drag xterm: `xev -event structure` streams ConfigureNotify *during* the drag (spec §10
live drag); (3) resize xterm: content reflows, the size snaps to character cells (increments); (4) `xdotool
getactivewindow windowmove 300 200` / `windowsize 400 300`: the native window follows, no bounce; (5) close xterm by
the red button (closes); (6) close a window without `WM_DELETE_WINDOW` (`tools/vng-scenarios/rootless-clients.py`
sets no `WM_PROTOCOLS`; check with `xprop`) twice: the confirm appears, Cancel keeps it, Kill closes it; (7) minimize/restore xclock: `xprop WM_STATE` goes Iconic/Normal and the window stays
mapped; (8) `wmctrl -r :ACTIVE: -b toggle,fullscreen` and the green button: `xprop _NET_WM_STATE` follows both ways;
(9) maximize: `wmctrl -r :ACTIVE: -b toggle,maximized_vert,maximized_horz` twice (zooms, then unzooms), then option-click
the green button and drag out of the zoomed frame: `_NET_WM_STATE` follows each step; (10) `wmctrl -b add,above` /
`add,below` / `remove,below`: the level changes and `_NET_WM_STATE` shows it; (11) Firefox's tab-strip drag
(`_NET_WM_MOVERESIZE` move) moves the window; an edge-resize request does nothing (the documented limitation).

**Gate** + eiger, commit `feat(macos): rootless native move, resize, close, minimize, zoom, levels and fullscreen`. ~380 lines.

### Task 13: Stacking, transients, popups on top

**Files:** `shell.rs`, `delegate.rs`.
- **Transients:** `NativeStyle.child_of = Some(p)` → `parent.addChildWindow_ordered(child, Above)` at map,
  re-parented on `InfoChanged(TRANSIENT_FOR)`; a parent's `Unmapped` detaches its children first.
- **Native → X:** on `windowDidBecomeKey` and `applicationDidBecomeActive`, `restack_requests(NSApp.orderedWindows ∩
  ours, X order)` → `Restacked { serial }`; the X order is kept from `Mapped.above`/`Restacked` events. **X →
  native:** `Restacked { from_client: true }` → `orderWindow_relativeTo(Above, other.windowNumber)` (`Below` for the
  bottom).
- **Popups:** `PopUp` level (`NSPopUpMenuWindowLevel`, 101), never key, `acceptsFirstMouse` true on their view.

**eiger:** (1) the gate; (2) `xmessage -buttons ok,cancel` from an xterm with `-transient`, or `zenity --info` if
installed: the dialog stays above its parent and moves with it [A: R5, does the child post `windowDidMove`? check
`xev` on the dialog]; (3) a GTK menu (gedit/`gtk3-demo` menus, combo box): opens at the right place, hover highlights,
a click selects, a click elsewhere in the app closes it; (4) a Qt context menu; (5) tooltips; (6) `xdotool
windowraise` on a lower window brings it forward natively; (7) Firefox if installed: menus, the URL bar drop-down, and
tab drag-move (`BeginMove`).

**Gate** + eiger, commit `feat(macos): rootless stacking, transients and popups`. ~180 lines.

### Task 14: Recipe, status, spec deltas

- `Justfile`: `macos-rootless display="1" log="info" scale="auto"`, same environment as `macos-run`, plus `--rootless`.
- `docs/status.md`: one RL2 entry (scope, D1-D13, the follow-ups, the eiger results).
- Spec deltas (one docs commit, decided by jos): §5/§9 one virtual output in RL2, per-output RandR to follow-ups
  (D1); §5 one display link in RL2 (D3); §4.5 the `surface_resizing`/`surface_rebuilt` hooks; §8.1 rectangular
  windows in RL2 (D5) and transients as child windows; §6.3 `_NET_WM_MOVERESIZE` is advertised but move-only in RL2
  (resize requests are dropped; AppKit has no public resize-drag call), `_NET_WM_STATE_BELOW` is level −1 (D13), and
  every native state change is reported with `StateChanged` (the core never writes `_NET_WM_STATE` for a request).

Commit `docs(rootless): RL2 recipe, status and spec deltas`.

## Hardware checklist for jos (eiger: M2, Mesa HEAD; air: M1)

Run from the bundle (`just macos-bundle`, `open target/yserver.app`; the display number is in the app menu) and once
with `just macos-rootless`. Tick each on both machines.

- [ ] **xterm** (×2), **xeyes** (rectangular, D5; tracks only over our windows), **xclock** (`-update 1` ticks).
- [ ] **A GTK app** (`gtk3-demo` or gedit): menus, combo boxes, dialogs, tooltips, the text cursor.
- [ ] **A Qt app** (`qtcreator`, `kate`, or `designer`): main window, context menu, file dialog.
- [ ] **Dialogs/transients:** `xmessage -center hi`, a GTK modal dialog: above the parent, move with it, close.
- [ ] **Menus and popups:** a GTK menu and a combo box. Firefox if available (menus, URL drop-down, a tab drag).
- [ ] **Drag-move:** `xev -event structure` streams ConfigureNotify during the drag.
- [ ] **Resize:** xterm reflows, and the size steps by character cells.
- [ ] **Minimize/restore:** yellow button and Dock; `xprop WM_STATE` Iconic, then Normal; stays mapped (decision 3).
- [ ] **Close:** red button on xterm closes it. On a window without `WM_DELETE_WINDOW` (`rootless-clients.py`) the
      first click does nothing and the second asks before killing (decision 12).
- [ ] **Focus:** clicks and Cmd-Tab switch windows; `xprop -root _NET_ACTIVE_WINDOW` follows; no key sticks.
- [ ] **Keyboard in two windows:** type in xterm A, click B, type: each gets its own text; Caps Lock state is kept.
- [ ] **Clipboard:** copy in xterm (`Ctrl-Shift-C` / selection + CLIPBOARD) → paste in a macOS app, and back.
- [ ] **1× ↔ 2× display** (decision 6; an external 1× display on eiger or air), once with `--scale 1` (S = 2) and
      once with `--scale auto` (S = 1): drag a window across, `xdotool windowmove` it back; X and native positions
      stay consistent (`xwininfo`), clicks land where the pixels are on both displays, and the only difference is
      sharpness (sharp where the display's backing scale = S; nearest-doubled or downsampled elsewhere, D2).
- [ ] **Fullscreen requests:** `wmctrl -r :ACTIVE: -b toggle,fullscreen`, the green button, and a client that asks
      itself (`xterm -fullscreen` or mpv `--fs`). The X window takes the screen size and `_NET_WM_STATE` follows.
- [ ] **Maximize and layers:** `wmctrl` maximize toggled twice, option-click zoom, `add,above`/`add,below`/`remove`:
      `xprop _NET_WM_STATE` follows every change, client- or user-made; a Below window stays visible (D13).
- [ ] **MOVERESIZE:** a Firefox tab-strip drag moves; an edge-resize request does nothing (known RL2 limit, D12).
- [ ] **Two displays, warp:** with a display above or left of the primary, `xdotool mousemove 0 0` lands at the
      union's top-left (Task 4).
- [ ] **Bordered window:** `xterm -bw 5`. The border is visible, and a click on it hits xterm (§10).
- [ ] **Quit:** app menu Quit and Dock Quit with clients connected ask first. With none connected they quit at once.
      SIGTERM quits without asking. Closing the last X window does **not** quit.
- [ ] **Rooted unchanged:** `just macos-run` still shows one window with the desktop.
- [ ] For the risks: menu open latency, and whether the log shows queue-idle retirements (no maintenance1).

## Decisions for jos

1. **D1 — displays: one virtual screen spanning the union, not per-output RandR. Agreed.** The root is the
   displays' bounding box at S, with one RandR output covering it (today's single-output path).
   - Why: RL0 D7's one-output core path stays valid; the pointer clamp is the bounding box, so a window lying partly
     in a dead zone between displays still takes clicks there, where a per-output CRTC constraint
     (`portable/pointer.rs:599-603`) would snap them away; a laptop alone is exact.
   - Costs: multi-monitor-aware clients see one monitor (GTK/Qt may centre a dialog or place a menu across a display
     seam, or in a dead zone of an L-shaped layout); Xinerama/RandR report one screen; a client that fullscreens by
     geometry (not `_NET_WM_STATE`) spans every display; the invisible root is allocated at the union size (6864×2160
     at S=2 is ~59 MB of VRAM). Per-output RandR is follow-up F1.
2. **D2 — scale (agreed).** S = the main display's X px per point at startup (`--scale auto` → 1; `--scale 1` → the backing
   scale, 2 on Retina), fixed for the session (decision 6). The layer has `contentsScale = S` and nearest
   magnification; the copy stays 1:1. Fallback if the filter is not honoured [A]: a per-window copy scale in
   `WindowSwapchain`, as the rooted blit has.
3. **D3 — pacing: one CVDisplayLink for all windows and for Present MSC (recommended).** It reuses the tested
   `DisplayLink` + `FrameClock`. Windows on a 120 Hz display are paced at the link's display rate. Per-screen
   `NSView` display links are follow-up F3.
4. **D4 — display changes at runtime: not in RL2.** The union is fixed at startup. A display plugged in later lies
   outside the root, so input there clamps to the root edge; restart yserver. Follow-up F2 also makes the core's
   `ScreenLayout` resize the root storage and re-sends every window's `Moved` when the root origin moves.
5. **D5 — SHAPE: rectangular windows in RL2 (agreed).** xeyes and oclock show their background outside the shape. Follow-up F4
   (transparent bounding shape + `hitTest:` for the input shape).
6. **D6 — keys: unchanged.** Every key goes to X, Cmd-Q included; quit by menu or Dock. Alternative: in rootless,
   give Cmd-Q to the app (XQuartz-like). Open.
7. **D7 — interface delta:** `RootlessHost::surface_resizing`/`surface_rebuilt` (Task 3). Needed by any layer-backed
   host. Recommended.
8. **D8 — focus between our windows:** no `Deactivated` between two X windows (Task 7). Recommended.
9. **D9 — transients as child windows** (as asked). A parent drag moves its dialogs, and their X positions must
   follow [A: R5]. If children post no `windowDidMove`, the shell re-reads children's frames on the parent's move.
10. **D10 — branch and CI (decided):** RL2 work happens on a new branch `feat/rootless-rl2`, cut from the rebased
    RL0/RL1 head, with a draft PR to master so macOS clippy/tests run in CI on every push.
11. **D11 — local macOS type-check:** `rustup target add aarch64-apple-darwin`, then `cargo clippy --target
    aarch64-apple-darwin -p yserver` on Linux. It needs no SDK to check, but the `freetype`/`fontconfig` `-sys`
    build scripts may refuse to cross-compile [A]. Worth one try; installing the target is jos's call.
12. **D12 — `WmRequest` coverage:** everything but `BeginResize` (Task 12). Every native state change, client- or
    user-initiated, reports back as `StateChanged` (Task 6, 12). `_NET_SUPPORTED` stays as RL0 announces it, with
    `_NET_WM_MOVERESIZE` documented as move-only in RL2: AppKit has no public API to start a native resize drag.
    Recommended; the resize alternative is follow-up F8.
13. **D13 — `_NET_WM_STATE_BELOW`:** AppKit defines no level below `NSNormalWindowLevel` (0, `NSWindow.rs:268`), but
    `NSWindowLevel` is a plain integer, so Below = level −1. Consequence: the window sits under every normal window of
    every app (not only ours) and above the desktop, and clicking it does not lift it above them. Alternative: keep
    level 0 and only `orderBack` among our windows (weaker: any activation raises it again). Recommended: level −1
    [A: eiger shows it stays visible above the desktop and takes clicks].

## Risks

- **R1 — maintenance1 on KosmicKrisp [A].** Without it, every unmap and every live-resize step costs one
  `vkQueueWaitIdle` on the core thread (RL1 Task 7). Menus pay it at menu speed. Task 9 measures it.
- **R2 — layer extent semantics [A].** Assumed: `currentExtent` reports the layer's `drawableSize` (which
  `surface_resizing` sets first) or `u32::MAX` (the backing extent is used). Otherwise frames are cropped or cleared
  for one frame per resize (`record_backing_copy` clamps; no crash).
- **R3 — first-frame latency per popup.** Each menu means NSWindow + layer + `vkCreateMetalSurfaceEXT` + swapchain
  before its first present (spec §11). If menus feel late, pool popup windows (follow-up F7).
- **R4 — live move notifications [A].** `windowDidMove` during a drag is assumed. If AppKit posts it only at the end,
  X lags the drag. Fallback: poll the frame on display-link ticks while the mouse is down in the title bar.
- **R5 — child windows [A].** Whether a child moved with its parent posts `windowDidMove` (D9).
- **R6 — alpha-0 order-in [A]** (spec §4.5.1): the window must be ordered in for acquires to succeed. Mouse events
  are ignored until `first_frame` (`setIgnoresMouseEvents`).
- **R7 — pointer scope during drags.** While a button is held, motion is local to the window that got the mouse-down.
  So the core's scope stays on it, and XDND between two top-levels sees the source window under the pointer.
  Follow-up F6.
- **R8 — union dead zones and root memory** (D1).
- **R9 — no motion outside our windows:** xeyes freezes over the desktop and other apps (follow-up F5, a global mouse
  monitor in root space with scope Nowhere, spec §4.4).
- **R10 — macOS code is checked only on eiger or CI.** Every macOS task carries its eiger list, and D10/D11 reduce the
  gap.
- **R11 — Present MSC from one clock:** a window on a faster display completes Present at the link's display rate.
- **R12 — `_NET_WM_MOVERESIZE` resize is advertised but dropped** (D12): clients that rely on it for edge resizing
  (CSD toolkits, Firefox) can only be resized from the native frame.
- **R13 — Below at level −1 [A]** (D13): it may hide behind other apps' windows for good, or interact oddly with
  Mission Control and Stage Manager.

## RL2 follow-ups

F1 per-output RandR (one CRTC per NSScreen, D1) · F2 runtime display changes (D4) · F3 per-screen display links
(D3) · F4 SHAPE bounding/input (D5) · F5 pointer motion outside our windows (R9) · F6 pointer scope during drags
(R7) · F7 popup window pool (R3) · F8 `_NET_WM_MOVERESIZE` resize via a host-tracked loop (on `BeginResize` with the button down, the view
follows `mouseDragged:` and sets the frame per motion until `mouseUp:`; no private API) · F9 Window menu, Dock menu listing windows,
`_NET_WM_ICON` → Dock/window icons · F10 RL1b root screenshots · F11 damage-limited GPU copy (RL1 D5) · F12 Cmd-key
policy (D6) · F13 RandR millimetres for S ≠ 1 (one virtual output reports 0 mm → 96 DPI).

## Size

| Task | Code | Tests | Runs on |
|---|---|---|---|
| 1 mode selection | 70 | 60 | Linux |
| 2a-2c Present clock, cursor, RandR refusal | 90 | 190 | lavapipe |
| 3 rebuild hooks | 35 | 60 | lavapipe |
| 4 `GlobalSpace`, local positions | 155 | 170 | Linux |
| 5 `GeometrySync` | 140 | 170 | Linux |
| 6 style, limits, state reporting | 170 | 190 | Linux |
| 7 seat/close/stacking/pacer/link | 240 | 220 | Linux |
| 8 `run_rootless` | 90 | 70 | lavapipe |
| 9 host + first windows | 560 | — | eiger |
| 10 input + focus | 300 | — | eiger |
| 11 cursor + warp | 80 | — | eiger |
| 12 native WM (incl. zoom, levels, state feedback) | 380 | — | eiger |
| 13 stacking, transients, popups | 180 | — | eiger |
| 14 recipe, status, spec | 10 | — | — |
| **Total** | **~2,500** | **~1,130** | |

Order of commits: 1 → 2a → 2b → 2c → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10 → 11 → 12 → 13 → 14.
- 1-8 are Linux-gated and independent of macOS. 4-7 are independent of each other.
- 8 needs 2a. 9 needs 3, 4, 7 and 8. 10 needs 9 and 7. 11 needs 10 (`PointerTracker::inside`). 12 needs 5, 6 and 9.
  13 needs 6, 7 and 12.
- The first user-visible state is after 9 (windows, no input), the first usable state after 10.
