# Rootless Core — Design

Date: 2026-10-19. Status: **revision r3** after codex review 3 (r1: loop ordering §4.1, one coordinate convention
§4.0, host serials and owned CPU frames §4.2/§4.4/§4.5; r2: host readiness states §4.5.1, canonical CPU image
§4.5.2, decisions 10–12 recorded; r3: `Ready` after a failed frame, damage since the last delivered frame; after the
RL0 plan review: a close request never kills (§6.4, §10), `next_wakeup` moves to RL1 (§9, §10), a fresh
`ToplevelId` per mapped lifetime (§4.2), one host output until RL2 (§5)). Branch
`plan/rootless-core`, cut from master `cb8297d0`.

Builds on: the macOS port plans (`plans/2026-10-14-macos-m0-m1.md`, `-m2-input.md`, `-m3-window.md`), the window
storage lifecycle spec (`2026-09-23-window-storage-lifecycle-design.md`, root-child boundary :37-40, :249-254), the
composite redirect port (`composite_redirects.rs`, `process_request/redirect.rs`), and the phase 3 render/scene
split (`plans/2026-10-13-phase3-render-crate.md`, `-phase3b-scene-split.md`).

Tags: **[C]** = checked in yserver code at `cb8297d0` (file:line). **[F]** = checked in SetZero's fork
(`github.com/ferrix-os/yserver`, branch `rootless-series` @ `8772cd65` unless marked **FR** =
`ferrix-rebase-2026-10-18` @ `7341fcf3`; read-only clone in `target/rootless-research/ferrix-yserver`, never built or
run). **[XW]** = Xwayland 24.1.9 (`target/rootless-research/xwayland/hw/xwayland`). **[XQ]** = Xorg 21.1.24
`../xserver/miext/rootless` and `hw/xquartz`. **[XWin]** = Xorg 21.1.24 `../xserver/hw/xwin`. **[A]** = assumption
about a native platform, to verify on hardware before the milestone that depends on it merges.

## Goal

Code name **RL**. One shared layer that turns every top-level X window into a native window, so each rootless backend
(macOS now, Wayland from SetZero later, Windows in the future) only does what is genuinely native: create and show
a native window, put pixels in it, map its coordinates, and relay what the native window manager does.

The shared layer owns:

- which X windows are top-levels and popups, and a lifecycle event stream for them (map, unmap, configure, restack,
  shape, info change, destroy);
- server-owned redirection of every root child, so each top-level has one backing that holds its whole subtree;
- per-top-level damage and a frame handoff (GPU copy, dmabuf, or CPU bytes);
- ICCCM/EWMH parsing into a neutral `WindowInfo`, and a built-in window-management policy (no X WM runs);
- input mapping: (native window, local position) → root position, enter/leave → crossings, native activation →
  X focus.

Success: the macOS backend gains rootless mode with no window-tree, ICCCM or focus logic of its own, and the
interface demonstrably fits SetZero's Wayland code (mapping table in §8) and a Windows HWND backend (§8.3) without
changes to the core module.

## Decisions (user, via review 1)

1. **macOS default mode:** rootless for the app bundle; a root window (rooted) for `just macos-run` (§7).
2. **Built-in WM;** an external X WM is refused in rootless mode (§6.5).
3. **Minimize keeps the X window mapped** and marks it iconic (§6.3).
4. **Live moves:** native moves reach X during the drag, coalesced per loop iteration (§5, §6.4).
5. **Windows rendering path** (Vulkan swapchain per HWND vs DXGI vs CPU) is postponed to RL4 and decided by
   measurement on yoga (§8.3).
6. **macOS scale:** ONE global scale in v1. This is a documented limitation; RL2 has a test requirement for moving
   windows between displays of different scales (§5, §10).
7. **SetZero** joins the **interface review** now; their port proceeds alongside RL2 once RL0 has settled (§9).
8. **Root screenshots** (`GetImage` on the root, `xwd -root`) compose on demand, in their own milestone RL1b (§9).
9. **Removing the dead nested code** (`host_x11/`, `nested.rs`) is a separate cleanup and does not block RL0.
10. **X borders on top-levels stay visible** (drawn as backing pixels, never zeroed; §4.0).
11. **Root screenshots** fill the area outside every top-level with the root background, including the root's
    configured background pixmap where it has one (RL1b, §9).
12. **Force-kill only with explicit user confirmation after a repeated close request.** An unanswered or unsupported
    `_NET_WM_PING` alone never kills a client, since killing it may close several windows (§6.4).

## Non-goals

- A Windows backend. Windows is a **design target**: every API below is checked against it (§8.3), but no Windows
  code lands in this work.
- A Wayland backend in this tree. SetZero upstreams theirs later, onto this interface.
- Running a third-party X window manager or X compositing manager in rootless mode (§6.5).
- Runtime switching between rooted and rootless (XQuartz can toggle; we pick at startup, §7).
- Client-side decorations drawn by the server (libdecor-like) for Wayland compositors without server-side
  decorations.
- Clipboard/selection bridging (macOS already has its internal client, `surface/clipboard/mod.rs`; Wayland has its
  own in the fork). Rootless only must not break it.
- GLX/DRI3 on macOS/Windows; Present beyond Copy completions (§11).
- Any `YSERVER_*` environment switch.

## 1. yserver today (facts the design rests on)

### 1.1 Backend and platform

- `trait Backend` (`yserver-core/src/backend/trait_def.rs:631`) is the core↔server-renderer seam [C]. Three
  impls: `RecordingBackend` (`backend/recording.rs:831`), the dead `HostX11Backend` (`host_x11/trait_impl.rs:20`),
  and `impl<P: Platform> Backend for XBackend<P>` (`yserver/src/kms/render/backend/trait_impl.rs:17`).
- **macOS is not a separate backend.** It is `XBackend<SurfacePlatform>` (`surface/platform.rs:834`, constructor
  `surface_backend` :192); KMS is `XBackend<PlatformBackend>` (`backend/kms/state.rs:823`). The OS seam is
  `trait Platform: OutputSink` (`kms/render/backend/seam.rs:28`, ~100 hooks) [C]. So the rootless layer plugs in
  **below `Backend`, at the platform**, not as a new `Backend` impl.
- Backend selection is by cfg, not by flag: `#[cfg(kms)] run` (`lib.rs:310-311`), macOS `macos::run`
  (`lib.rs:298-301`, `macos/run.rs:12-17`), else `surface::server::run_headless` (`lib.rs:304-307`) [C].
- Window-tree hooks exist on `Backend` but are the nested-X11 shape: `create_subwindow` (:1289), `map_subwindow`
  (:1306), `configure_subwindow` (:1331, restack via its `sibling`/`stack_mode`), `register_top_level` (:1367),
  `sync_top_level_order(&ServerState)` (:1132), `on_window_became_top_level` (:1119),
  `on_window_property_changed` (:1081) [C]. XBackend deliberately does not use the last two
  (`portable/windows.rs:1869-1879`).
- `host_x11/` and `nested.rs` are a dead runtime path (`nested::run` has no caller; `ynest` removed in `993b76f7`),
  but their **types** are live in 51 files (`HostPointerEvent`, `HostKeyEvent`, `HostXidMap`, …) [C]. RL does not
  reuse the nested model (one host window per X window); it is a separate cleanup.

### 1.2 macOS today

- One `NSWindow` + `CAMetalLayer` is the whole root (`macos/app.rs:219-288`, `macos/window.rs:23-109`); the core
  runs on its own thread (`surface/server.rs:37`, :88) [C].
- Frames: the scene composes into X-sized offscreen slots, then `present_pending` copies or NEAREST-blits a slot
  into a swapchain image and calls `vkQueuePresentKHR` (`surface/swapchain.rs:1-10`, :325). Pacing by a
  CVDisplayLink `FrameClock` (`macos/display_link.rs`, `surface/clock.rs`) [C].
- Input: NSEvent → `surface/input/translate.rs` → `Message::HostInput` → `backend.on_host_input`
  (`core_loop/run.rs:4301`). Pointer position is `x_position` (`surface/input/pointer.rs:28`): view points × scale,
  y flipped, **root space only because the view is the root** [C].
- Core → shell: `trait ShellUi: Send + Sync` (`surface/ui.rs:29`): `set_title`, `quit`, `set_cursor`,
  `warp_pointer`, `set_content_size`, executed on the main queue (`app.rs:317-385`) [C]. The cursor is an NSCursor
  from the X image, chosen in M2 because it "works unchanged for rootless" (`plans/…-m2-input.md:181-186`).
- The M1 plan already reserved the shape of this work: "Rootless (M4) adds surfaces whose root rects are the
  top-levels. Nothing above the sink assumes one swapchain or the whole root" (`plans/…-m0-m1.md:357-361`).

### 1.3 Render, scene, storage

- `yserver-render` holds the Vulkan renderer; the scene is `yserver-render/src/kms/render/scene*`.
  `SceneCompositor` (`scene.rs:568`) composes `SceneInput.top_level_order` onto `OutputLayout { root_rect, mode,
  transform }` (`scene.rs:107-114`) [C]. An output whose `root_rect` equals a top-level's rect would still compose
  the root and overlapping windows under it, so **the scene is the wrong tool for per-top-level images**.
- Every viewable window owns a leaf image (`allocate_window_leaf`, `portable/windows.rs:1722`). Under redirect,
  painting resolves to the redirected ancestor's backing (`resolve_paint_target`, `portable/paint_target.rs:405-446`;
  `store.redirected_target`, `store.rs:1406`, :1436) [C]. **A redirected top-level's backing is the one image of
  its whole subtree** — exactly what a native window needs.
- Render-side damage: `DrawableStore::damage` (`store.rs:1246`) accumulates `presentation_damage` for
  scene-participating drawables; `peek_presentation_damage` / `ack_presentation_damage` (:1352, :1373) are an
  epoch snapshot/ack with a single consumer [C].
- Pixel access: synchronous `Engine::get_image` (`engine/put_get.rs:215`); dmabuf export
  `promote_drawable_exportable` (`engine/export.rs:27`), not wired on `SurfacePlatform` (seam defaults
  `seam.rs:408-551`) [C].

### 1.4 Composite redirect

- `composite_redirects.rs` ports Xorg `compalloc.c` bookkeeping; `redirect_window` refuses a second Manual
  redirect with `ManualTaken` (:28, :119-134); `redirect_subwindows` (:158); `redirect_new_subwindow` (:207) gives
  new children of a subwindow-redirected parent their redirect at creation [C].
- `sync_redirect_backing` (`process_request/redirect.rs:295-317`) → `activate_redirect_backing_for` (:47) →
  `backend.allocate_redirected_backing` (trait :1436). Manual: the window leaves the scene, the backing stays
  participating (:155-173) [C].
- `SERVER_OWNER = ClientId(0)` (`resources/mod.rs:25`); the screensaver already creates windows as the server
  (`saver_dpms.rs:303-310`). There is no server-owned redirect today [C].

### 1.5 Input, focus, WM requests

- `HostInputEvent` (`core_loop/message.rs:328`) positions are root-space [C]. XBackend handles it in
  `backend_devices_on_host_input` (`portable/devices.rs:408`) → `process_pointer_absolute`
  (`portable/pointer.rs:553`) → hit test `ServerState::root_pointer_target_at` (`server.rs:3143`) → crossings via
  `crossings::normal_mode_crossings` (`crossings.rs:90`).
- Focus: `apply_core_input_focus` (`process_request/focus_pointer.rs:153`, `pub(super)`, needs a client and a
  sequence) [C].
- MapWindow and ConfigureWindow redirect to SubstructureRedirect holders on the parent (`windows.rs:2390-2416`,
  :1798-1821, `subscribers_by_id` `core_loop/fanout.rs:131`). Without a holder, clients map and configure directly.
  `handle_send_event` (`selection.rs:359`) does plain delivery; no server-side `_NET_WM_*` handling exists [C].
- No typed ICCCM/EWMH parser exists anywhere in the server [C].

## 2. Prior art

### 2.1 XQuartz: `miext/rootless` (frames)

The server owns one native **frame** per top-level, created at Realize (`rootlessWindow.c:375-479`) through
`RootlessFrameProcsRec` (`rootless.h:290-314`: Create/Destroy/Move/Resize/Restack/Reshape/UnmapFrame,
Start/StopDrawing, UpdateRegion, DamageRects, optional SwitchWindow/DoReorderWindow, CopyBytes/CopyWindow) [XQ].

- Pixels: `StartDrawing` returns a pointer into the native frame's memory and the window pixmap becomes a scratch
  header over it (`rootlessCommon.c:160-238`); every GC op is wrapped to start drawing and damage
  (`rootlessGC.c:451-575`). Damage is flushed by a 10 ms timer in the block handler (`rootlessScreen.c:713-773`).
- The root is invisible: `HUGE_ROOT`, a custom ValidateTree and MarkOverlapped so top-levels never clip each other
  or the root (`rootlessCommon.h:190-214`, `rootlessValTree.c:459-529`, `rootlessScreen.c:550-620`).
- Real global coordinates: frames at X position + `rootlessGlobalOffset` (`rootlessWindow.c:413-415`;
  `xprScreen.c:462-463`). Native moves come back as a fake `ConfigureWindow` with a loop guard
  (`rootlessWindow.c:98-140`). The server restacks frames itself (`RestackFrame`, :539-588).
- WM: a separate X client, quartz-wm, through the AppleWM extension (`applewm.c:622-675`).

### 2.2 Xwayland: composite redirect + buffers

The server redirects each top-level Manual as `serverClient` (`xwayland-window.c:1517`, undone :1687) and gives it
a `wl_surface` with no role; the compositor's X WM assigns the role through a surface-serial association
(`:750-796`) [XW].

- Gate: rootless surfaces exist only for windows already Manual-redirected by the WM (`:1470-1472`); "top-level" =
  root child or child of a WM frame (`:688-700`, :1866-1889).
- Pixels: per-window `DamageCreate` (`:328-345`) → block handler `xwl_screen_post_damage`
  (`xwayland-screen.c:432-468`) → attach the backing as a `wl_buffer` (glamor dmabuf or shm,
  `xwayland-pixmap.c:66-76`), throttled by frame callbacks; a small buffer swapchain with per-buffer damage copies
  (`xwayland-window-buffers.c:396-477`).
- The root has no storage: `ROOT_CLIP_INPUT_ONLY`, 0×0 screen pixmap (`xwayland-screen.c:994-999`).
- Positions: Xwayland never tells the compositor a position (no `set_position` anywhere); the compositor must also be
  the X WM (`man/Xwayland.man:144-147`). Input inverts by `drawable.x + sx` (`xwayland-input.c:546-553`, :657-691);
  a pointer outside every surface hit-tests to root so a LeaveNotify fires (`:3182-3251`). Keyboard enter only
  replays keys; X focus is the WM's job (`:1213-1236`).

### 2.3 XWin multiwindow (Windows prior art)

One HWND per mapped root child, override-redirect included (`winmultiwindowwindow.c:684`, :481-610) [XWin].

- Pixels: every pixmap is a DIB section (`:967-1102`); the internal WM sets `CompositeRedirectSubwindows(root,
  Automatic)` (`winmultiwindowwm.c:1162-1195`), and `WM_PAINT` `BitBlt`s the backing DIB
  (`winshadgdi.c:812-869`), triggered by **root** damage extents (`:282-311`, :525-529). No
  `UpdateLayeredWindow`; ARGB only via blur-behind hacks (`winmultiwindowwndproc.c:325-408`).
- Coordinates: X root = Windows virtual screen (`±SM_X/YVIRTUALSCREEN` everywhere, e.g.
  `winmultiwindowwindow.c:212,500`); X geometry = client rect, frame added with `AdjustWindowRectEx`. No per-monitor
  DPI awareness.
- Windows owns Z-order; X is re-derived from it (`winReorderWindowsMultiWindow`, `:748-800`) with static
  re-entrancy guards (`fRestacking`, `winInRaiseWindow`). Native moves → `winAdjustXWindow` → `ConfigureWindow`
  (`:870-962`), only after `WM_EXITSIZEMOVE` (`winmultiwindowwndproc.c:961-966`, :1098-1136).
- WM: two threads with their own xcb connections (`winmultiwindowwm.c:1392-1454`), mapping
  `_NET_WM_NAME`/`WM_HINTS`/`_MOTIF_WM_HINTS`/`_NET_WM_WINDOW_TYPE`/`WM_NORMAL_HINTS` to Win32 styles
  (`winApplyHints`, :1723-1959), `WM_CLOSE` → `WM_DELETE_WINDOW` or KillClient (:887-899), `WM_ACTIVATE` →
  `SetInputFocus` + `WM_TAKE_FOCUS` (:901-936). It never takes SubstructureRedirect (`:1654-1696`).
- Lessons (all from source, none measured): the WM thread touches server structures without a lock
  (`:870-876`, :1967-1985) and makes cross-thread `SendMessage` calls that block on the server thread; one Win32
  message per wakeup with a 100 µs busy-poll (`winwakeup.c:46-53`, `winblock.c:46-62`); a `GetCursorPos` polling
  timer to track the pointer outside X windows (`winwndproc.c:862-897`); `_NET_WM_STATE_FULLSCREEN` mapped to
  maximize.

### 2.4 SetZero's fork

SetZero already factored a host-neutral core: `yserver_core::rootless` (`rootless/mod.rs:1-31`,
`topology.rs`, `popups.rs`, `seat.rs`) with a `HostWindows` trait (`mod.rs:109`), `ToplevelSpec` (:81),
`PopupPlacement` (:100), `RootlessRenderer` (:52), and a Wayland host behind feature `wayland` and binary
`ywayland` (`crates/yserver/Cargo.toml`, `src/bin/ywayland.rs:33`) [F]. Its module doc names macOS and Windows as
later hosts (`mod.rs:5-7`). What it does:

- Redirects all root children Manual as `SERVER_OWNER` (`process_request/for_server.rs:24`) and announces itself
  as an EWMH WM without SubstructureRedirect (`topology.rs:371-383`, `for_server.rs:284`).
- **Diffs** the root's children every loop iteration (`Topology::sync`, `topology.rs:194-336`); top-level = viewable,
  InputOutput, not override-redirect, not the COW (`:534`). Override-redirect → `xdg_popup` with a parent guess
  (`popups.rs:256-310`), `grab=false`.
- Keeps X positions (Xwayland model, `mod.rs:28-31`); root = first `wl_output` only (`toolkit.rs:348-361`);
  `PointerScope {Anywhere, Nowhere, Within(top)}` restricts the hit test to the entered top-level
  (`server.rs:370`, :3198).
- Pixels: synchronous full-window Vulkan readback into `wl_shm` on the core thread (`backend/rootless.rs:65-139`,
  `engine.get_image_with`), a per-row hash to find one damage band (`topology.rs:490-528`), frame-callback paced.
- Focus: keyboard enter → `set_input_focus_for_server` (`seat.rs:174-200`). Compositor resize → X ConfigureWindow
  (`carry_out`, `topology.rs:387`). `_NET_WM_MOVERESIZE` → `xdg_toplevel.move/resize` (`wayland.rs:167-182`).

What upstream would not take as is: a 1752-line hand-written `Backend` delegation wrapper guarded by a test that
string-parses `trait_def.rs` (`rootless.rs:210-235`); the window sync piggybacked on `poll_deferred_input`
(`rootless.rs:161-173`); synchronous readback per frame; core handlers driven by synthesized little-endian request
bodies (`for_server.rs:127-140`, :183-204) and server XIDs from a linear scan (:244-261); a 5 ms poll loop for
clipboard pipes (`wayland.rs:203-210`); ignored xdg_toplevel states and immediate `ack_configure`
(toolkit `client.rs:2303-2352`); single output, no HiDPI; unrelated fixes bundled into the series (key
`event_x/y`, grab child, writer-fd polling, nonblocking accept, resize-expose paint). FR adds env switches
`YSERVER_BACKEND` and `YSERVER_POINTER_LOCK` and `cfg` fields inside `KmsBackend` (`kms/state.rs:1043-1049`).

## 3. Which model fits yserver

| | XQuartz frames | Xwayland redirect | XWin | **RL** |
|---|---|---|---|---|
| Per-top-level image | native frame memory, drawn in place | Composite backing | Composite backing (DIB) | **Composite backing (VkImage)** |
| Who redirects | n/a | server, Manual (after WM) | internal WM, Automatic | **server, Manual, at startup** |
| Damage source | GC wrappers | DamageExt per window | root damage | **render-side backing damage** |
| Positions | real, server moves frames | opaque, compositor mirrors X | real | **per-backend policy (§5)** |
| Z-order source | X | compositor | Windows | **native, mirrored into X** |
| WM | external quartz-wm | compositor's X WM | internal X client threads | **built-in policy in core (§6)** |

**Decision 1: the Xwayland model (server-owned Manual redirect of every root child), with a built-in WM policy.**

- yserver already has what the XQuartz model would have to rebuild: GPU storage per drawable and a redirect path
  where a top-level's backing holds its whole subtree (§1.3). Drawing straight into native memory would bypass the
  Vulkan renderer entirely and reintroduce a CPU fb path; it does not fit.
- Manual (not Automatic as XWin does) keeps top-levels out of any root scene: rootless has no root to compose
  (M2 plan: "rootless has no full-screen scene", `plans/…-m2-input.md:176`).
- Unlike Xwayland, no X WM runs, so the server must own the WM policy itself (XWin's lesson: do it inside the
  server, not as threads with their own connections and unlocked access to server state).
- SetZero independently reached the same model (§2.4), which keeps their upstreaming cheap.

## 4. The proposed split

```
yserver-core::rootless            (no Vulkan, no OS; unit-tested on ServerState + RecordingBackend)
  Rootless   — mode state in ServerState: redirect setup, topology model, reconcile → events,
               WindowInfo parser, WM policy, seat (input mapping, focus), host requests → X actions
  RootlessEvent / WindowInfo / Geometry / HostRequest / RootlessInput   (plain data, Send)

yserver::rootless                  (render-aware; per Platform)
  Presenter  — per top-level: backing drawable, damage peek/ack, frame handoff (GPU copy | dmabuf | CPU)
  trait RootlessHost                — implemented by each native backend (macOS, Wayland, Windows)
```

### 4.0 The coordinate convention (the only definition; every formula below follows it)

Facts [C]:
- A window's `x, y` locate its **outer** upper-left corner (border included) relative to its parent, and
  `window_absolute_position` returns the **content** origin, `border_width` further in, as Xorg's `drawable.x`
  (`resources/tree.rs:334-352`, `dix/window.c:888`).
- A redirect backing is allocated at the bordered extent `(w + 2bw) × (h + 2bw)` and holds the window at its outer
  origin, content at `(bw, bw)` (`process_request/redirect.rs:101-111`, Xorg `composite/compalloc.c:600-606`). The content
  offset is recorded per allocation, not inferred (`storage_content_offset`, `portable/paint_target.rs:715-725`).
- SHAPE regions are relative to the content origin (`drawable.x`) and may extend into the border
  (`../xserver/dix/window.c:1735-1742`).

Definitions, for a top-level `T` (a root child, so its `x, y` are already root coordinates) with border `bw`:

| Name | Origin | Extent | In root coordinates |
|---|---|---|---|
| **outer rect** `O(T)` | `(T.x, T.y)` | `(T.width + 2bw, T.height + 2bw)` | `O.origin = window_absolute_position(T) − (bw, bw)` |
| **backing pixels** | backing (0,0) = outer corner | = outer extent | backing `(bx, by)` = root `O.origin + (bx, by)` |
| **native content** (the drawable area of the native window, inside any native decoration) | = outer corner | = outer extent (macOS: extent / S points) | native content `(0,0)` = backing `(0,0)` |
| **pointer-local** (what a host sends) | native content (0,0) | native content, **in backing pixels** | root = `O.origin + local` |

So the native window shows **the whole backing, X border included**. Everything that crosses the host↔core
boundary (positions, sizes, pointer-local, damage, shapes) is in **X pixels**; a host converts its native units at
its own edge (macOS: points × S, as `x_position` already does, `surface/input/pointer.rs:28`; S per §5), so the core
never scales. Consequences used everywhere below:

- `Geometry { x, y }` is the outer corner; `width, height` are the X **content** size and `border` is `bw`; the native
  content size is `(width + 2·border, height + 2·border)`.
- Damage rects, `Reshaped` shapes and popup placement are all in **backing pixels**: shapes are translated by
  `(+bw, +bw)` from their X (content-origin) coordinates before they reach a host.
- A host-driven size `(W, H)` of native content becomes X content `(W − 2bw, H − 2bw)`; a host-driven position
  `(X, Y)` of native content becomes `T.x = X, T.y = Y`.
- Popup placement: `at = O(popup).origin − O(parent).origin` (backing pixels of the parent).
- Top-level borders stay visible (decision 10); we do not zero them as XWin's WM does
  (`winmultiwindowwm.c:1227-1241`): with no reparenting WM a client's border is part of what it drew.

### 4.1 Ownership and loop integration

- `ServerState.rootless: Option<Rootless>` [new]. `Some` iff the server started in rootless mode. Request handlers
  that need the mode (SendEvent to root, SubstructureRedirect on root, §6) read it there; this is where SetZero's
  `server_wm_requests` / `pointer_scope` fields went (`server.rs:1089`, :1245) [F].
- Startup: `Rootless::start(state, backend)` calls `composite_redirects.redirect_subwindows(ROOT,
  RedirectRecord { mode: Manual, owner: SERVER_OWNER })`, then `sync_redirect_backing` for existing children.
  New children get theirs at creation through `redirect_new_subwindow` (`composite_redirects.rs:207`).
  `forget_client` must never drop `SERVER_OWNER` records (verify, `composite_redirects.rs:246`).
- **Reconcile once per loop iteration, in `run_iteration_tail`, immediately before `maybe_composite`.** The real
  loop order [C]:
  1. loop head: `poll_timeout` is computed, including `backend.next_wakeup()` (`core_loop/run.rs:2208-2253`);
  2. `backend.before_block()` (:2260), then `poll` (:2268) and dispatch of fds, client requests and `Message`s;
  3. `settle_client_output` (:2934), then `run_iteration_tail` (:2943; body :3078-3109): damage-notify flush,
     `poll_deferred_input`, XFIXES cursor notify, `drain_present_completions`, **`maybe_composite`** (:3104),
     `arm_present_idle_vblanks`; then `settle_client_output` again (:2944-2950).

  `before_block` runs at the next loop head, after this iteration's compose and after the next `poll_timeout` is
  already fixed, so reconciling there (r0 of this spec) would hand a newly mapped window to the presenter only after another, unrelated wakeup.
  Instead the tail gains one call, after `drain_present_completions` and before `maybe_composite`:
  `rootless::reconcile_and_dispatch(state, backend)` → `Rootless::reconcile(&mut self, &ServerState) ->
  Vec<RootlessEvent>` diffs the root's children against the last snapshot (map state, geometry, border, order,
  shape, override-redirect, dirty properties, host acks), then `backend.on_rootless_events(state, &events)` [new
  `Backend` method, default no-op] hands them to the platform, and the same iteration's `maybe_composite` runs the
  presenter (§4.6), so a window mapped in this iteration gets its first frame in this iteration. Host requests are
  `Message`s dispatched in step 2, so they are applied before the reconcile that acknowledges them. Client output
  the reconcile produces (WM_STATE PropertyNotify, synthetic ConfigureNotify) is flushed by the second
  `settle_client_output`. Not in an input hook either (fixes `rootless.rs:161` [F]).
- **Wakeups.** Pending rootless work must reach the next `poll_timeout` through `Backend::next_wakeup`
  (`trait_def.rs:1025-1031`; the surface platform's `next_wakeup`, `surface/platform.rs:857-875`, is where it joins):
  - a top-level with unacknowledged damage in readiness `ReadyFirst`/`Ready` (§4.5.1) → `now` (normally it was
    already presented this iteration, so this is only the "host became ready during the tail" case);
  - damage while the host is not ready → **no deadline**: host readiness (Wayland frame callback, display-link tick,
    `WM_PAINT`/DWM) and setup completion arrive as `FrameEvent::Ready`/`SetupDone` on the platform link, which wakes `poll` as
    `Message::PlatformEvent` (§4.6);
  - a CPU readback in flight → its completion: an fd where the platform has one (a sync-file registered via
    `poll_fds`, as KMS does for `BackendFdKind::ScanoutRenderCompletion`, `trait_def.rs:96`), else the existing 1 ms
    GPU-poll deadline convention used for polled Present batches (`portable/compose.rs:214-222`), only while a
    readback is in flight;
  - nothing pending → `None`. Reconcile itself never needs a wakeup: every topology change is caused by a request,
    a disconnect or a `Message`, each of which already ends in a tail.
- **Why diff and not per-request hooks:** one place sees every path (reparent in/out of root, destroy-with-children,
  client disconnect teardown, RandR root resize); a map + configure in one iteration coalesces into one `Mapped`
  with final geometry; it is what SetZero's `Topology::sync` already does. Cost is O(top-levels) per iteration with
  no property hashing: properties use a dirty set fed by the existing `on_window_property_changed` call sites
  (`props.rs:1102`, :1237, :1273), routed to `Rootless` in the core instead of the backend. Shape is compared as
  the stored rect lists (rare, small).
- Host → core goes through the existing `CoreSender` (`trait_def.rs:1010`): input as `Message::HostInput` with new
  local-coordinate variants (§4.4), WM-side actions as a new `Message::Rootless(HostRequest)`, executed on the core
  thread by `Rootless::host_request(state, backend, req)`. This is the same channel macOS input uses today
  (`surface/link.rs`).

### 4.2 Events

```rust
/// Names ONE mapped lifetime of a top-level: allocated fresh at every `Mapped` (a remap gets a
/// new id) and retired at the `Unmapped` that ends it. Never an XID: an XID can be reused by a
/// new window while an old event for it is still in a host's queue. The core drops every host
/// request and input whose id is not live, so a close, move, ack or pointer event queued for a
/// destroyed native window never reaches the window's next mapped lifetime. 64 bits: the
/// counter survives server resets and never wraps in practice (it wraps, never panics).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ToplevelId(NonZeroU64);

pub enum ToplevelKind {
    /// Managed top-level: a native window with native decorations and management.
    Managed,
    /// override-redirect: menu, tooltip, drop-down, DnD icon. Not decorated, never activated
    /// by a click, placed exactly where X put it, relative to `parent` where the host needs it.
    Popup { parent: Option<ToplevelId> },
}

/// §4.0: `x, y` = outer corner in root coordinates; `width, height` = X content size;
/// native content = (width + 2·border) × (height + 2·border) backing pixels.
pub struct Geometry { pub x: i32, pub y: i32, pub width: u16, pub height: u16, pub border: u16 }

/// Monotonic per host, assigned by the host to every geometry/stacking HostRequest (§4.4).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct HostSerial(u64);

/// Which geometry fields a NON-host source (client ConfigureWindow, size-hint constraint,
/// RandR root change) set after the last acknowledged host request.
pub struct GeometryFields(u8);   // X | Y | WIDTH | HEIGHT | BORDER

pub enum RootlessEvent {
    /// The window became viewable. Carries everything a host needs before showing it (Wayland
    /// must set title/app_id/parent/limits before the first commit, toolkit `client.rs:687-748` [F]).
    /// The host creates its native window now, runs its setup handshake, and makes it visible to
    /// the user only once the first frame is delivered (§4.5.1; no empty flash, cf.
    /// `_XWAYLAND_ALLOW_COMMITS`, `xwayland-window.c:128-191`).
    /// `above` = the nearest lower top-level that is not restacked in the same batch, so events
    /// apply in order.
    Mapped { id: ToplevelId, window: ResourceId, kind: ToplevelKind, geometry: Geometry,
             depth: u8, info: WindowInfo, above: Option<ToplevelId> },
    /// Destroy the native window: the window was unmapped, destroyed, reparented away from the
    /// root, or its client left. Retires `id`; nothing more is ever reported for it.
    Unmapped { id: ToplevelId },
    /// Final geometry after this iteration. `acked` = highest host serial for this window applied
    /// since the last event (also when the request changed nothing, e.g. clamped by size hints).
    /// `client_fields` = fields the host must apply natively (§4.2.1).
    Configured { id: ToplevelId, geometry: Geometry, acked: Option<HostSerial>,
                 client_fields: GeometryFields },
    /// X stacking changed: `id` now sits directly above `above` (None = bottom of ours).
    /// `acked` as above; `from_client` = a non-host restack was applied after the acked request.
    Restacked { id: ToplevelId, above: Option<ToplevelId>, acked: Option<HostSerial>,
                from_client: bool },
    /// Bounding and input shapes in backing pixels (§4.0; None = rectangular).
    Reshaped { id: ToplevelId, bounding: Option<Vec<Rect>>, input: Option<Vec<Rect>> },
    /// `info` re-parsed after a property change, or `transient_for` re-resolved because the
    /// transient parent mapped or went away.
    InfoChanged { id: ToplevelId, info: WindowInfo, changed: InfoFields },
    /// A popup's placement parent (`ToplevelKind::Popup { parent }`) changed: its
    /// `WM_TRANSIENT_FOR` changed or resolved, or the parent went away (re-chosen by the §10
    /// order). Separate from `InfoChanged`: `transient_for` is the client's ICCCM relationship,
    /// this is the core's placement choice, which a Wayland host can only apply by re-creating
    /// the xdg_popup; a Managed window's transient parent travels in `InfoChanged` only.
    ParentChanged { id: ToplevelId, parent: Option<ToplevelId> },
    /// A client asked the WM for something (§6.3).
    WmRequest { id: ToplevelId, request: WmRequest },
    /// Follows `Unmapped` in the same batch when a MAPPED top-level stopped existing as one
    /// (destroyed, reparented away, owner gone). A window destroyed after its unmap gets nothing:
    /// its id is no longer live.
    Destroyed { id: ToplevelId },
    /// Root size or native output layout changed (§5); hosts re-place their windows.
    ScreenChanged { root: (u16, u16) },
}
```

#### 4.2.1 Acknowledgment and coalescing rules

The core is the single serialization point: host requests and client requests are applied in the order the core
dispatches them, and the last writer of a field wins. Reconcile then reports the **final** state per window, once
per iteration, so the rules are about not losing information in that coalescing:

1. Per top-level the core keeps `acked: Option<HostSerial>` (highest applied host serial not yet reported) and
   `client_fields` (fields set by a non-host source **after** that serial was applied; a later host request clears
   the fields it sets). Both reset after the event is emitted.
2. A host request is acknowledged even when it changes nothing (refused by size hints, same value): reconcile emits
   `Configured`/`Restacked` whenever `acked` is set, not only when the geometry differs.
3. Within one iteration: host move (serial 7) → client resize → host move (serial 8) emits one `Configured` with
   `acked = 8`, `client_fields = WIDTH|HEIGHT`. Client move after host serial 8 gives `client_fields ⊇ X|Y` even
   though the host also wrote X/Y earlier in the iteration: the client's value is final and must be applied.
4. Host rule: apply natively exactly the fields in `client_fields`, from `geometry`, even when the host has newer
   requests in flight (serial > `acked`); fields not in `client_fields` already match what the host asked for up
   to `acked`, and anything the host sent later will be acknowledged by a later event. A host that sees
   `acked < its latest serial` and `client_fields` empty does nothing.
5. Size-hint clamping of a host resize is a non-host change of that field (`client_fields ⊇ WIDTH|HEIGHT`), so the
   native window snaps to the clamped size.
6. Restacks follow the same rules with `from_client` in place of `client_fields`.
7. An event with neither a geometry change, an ack, nor client fields is not emitted.

This replaces r0's `ChangeOrigin { Client, Host }`, which could hide a client configure coalesced with a host move.

The cursor is not an event: it stays global, through the existing `ShellUi::set_cursor` path (macOS) and the
render side's `get_active_cursor_image` (`trait_def.rs:2980`).

### 4.3 WindowInfo

Parsed in core from `resources.window_property` (`resources/pixmap_props.rs:16`) with the window's byte order taken
from how the core stores property data (SetZero decodes as little-endian unconditionally, `topology.rs:564`, :589
[F]; the plan must check `properties.rs:6-10` and decode correctly for big-endian clients).

```rust
pub struct WindowInfo {
    pub title: String,                 // _NET_WM_NAME (UTF8_STRING), else WM_NAME (Latin-1/COMPOUND_TEXT best effort)
    pub class: Option<(String, String)>,   // WM_CLASS instance, class → app_id / NSApp grouping / AppUserModelID
    pub transient_for: Option<ToplevelId>, // WM_TRANSIENT_FOR, tracked after map (XWin does not, :223-254)
    pub window_type: WindowType,       // _NET_WM_WINDOW_TYPE first known atom; default Normal / Dialog if transient
    pub size_hints: SizeHints,         // WM_NORMAL_HINTS: min, max, base, inc, aspect, gravity, USPosition/PPosition
    pub input_hint: Option<bool>,      // WM_HINTS.input
    pub urgency: bool,                 // WM_HINTS.XUrgencyHint
    pub protocols: Protocols,          // WM_PROTOCOLS: DELETE_WINDOW, TAKE_FOCUS, _NET_WM_PING
    pub decorations: Decorations,      // _MOTIF_WM_HINTS: Full | None | Border (title/border subset)
    pub state: NetWmState,             // _NET_WM_STATE as set by the client before map (fullscreen, maximized,
                                       //   above, below, skip_taskbar, modal, hidden)
    pub pid: Option<u32>,              // _NET_WM_PID
    pub icon: Option<IconRef>,         // _NET_WM_ICON present (host fetches pixels on demand)
}
```

`InfoFields` is a bitset so a host can skip work (a title change must not re-style a Win32 window).

### 4.4 Input injection

The host only knows "this native window, this local position". The root position depends on the top-level's X
origin, which only the core thread knows race-free, so the conversion happens there (Xwayland does the same,
`xwayland-input.c:546-553`):

```rust
// new HostInputEvent variants, sent through Message::HostInput (core_loop/message.rs:328)
PointerEnter  { origin, toplevel: ToplevelId, local: (f64, f64), time }
PointerMotionLocal { origin, toplevel: ToplevelId, local: (f64, f64), time }
PointerLeave  { origin, time }                        // pointer is over no X window
// buttons, scroll, keys: the existing variants, unchanged

// Message::Rootless(HostRequest)
pub enum HostRequest {
    Activated { id: ToplevelId },          // native focus/key window gained
    Deactivated,                           // focus left all X windows (other app)
    /// Native content origin in root coordinates (§4.0); Global model only (§5).
    Moved { id: ToplevelId, x: i32, y: i32, serial: HostSerial },
    /// Native content size in X pixels, border included (§4.0); core subtracts 2·bw.
    Resized { id: ToplevelId, width: u16, height: u16, serial: HostSerial },
    Restacked { id: ToplevelId, above: Option<ToplevelId>, serial: HostSerial },
    StateChanged { id: ToplevelId, state: NetWmState },               // native fullscreen/max/minimize
    CloseRequested { id: ToplevelId },
    ForceKill { id: ToplevelId },                                     // user confirmed (§6.4)
    ScreenLayout { outputs: Vec<HostOutput> },                        // §5, RandR
    PopupDismissed { id: ToplevelId },                                // host closed a popup
}
```

Core semantics:

- `PointerEnter/MotionLocal`: `local` is pointer-local (§4.0: origin = native content (0,0) = backing (0,0) = outer
  corner), so root = `window_absolute_position(T)` (`resources/tree.rs:337`, the content origin) − `(bw, bw)` +
  `local`. A click on the X border hits the top-level itself, as in Xorg. The core sets the **pointer scope** to
  that top-level, so the hit test (`root_pointer_target_at`, `server.rs:3143`) only finds windows in its subtree;
  X stacking can disagree with the native one (Opaque model) or lag it (Global model), and the native system is
  right about which window is under the pointer. This is SetZero's `PointerScope` (`server.rs:370`, :3198 [F]) and Xwayland's `xwl_xy_to_window`
  (`xwayland-input.c:3182-3251`).
- `PointerLeave`: scope Nowhere; the hit test returns root so the client gets LeaveNotify (Xwayland :3239-3248),
  held buttons are released, and no X motion is generated until the next enter. This replaces XWin's
  `GetCursorPos` polling (`winwndproc.c:862-897`): hosts that know the global pointer outside our windows (macOS
  `mouseMoved` global monitor, Windows) may still send `PointerMotion` in root space with scope Nowhere so xeyes
  works; Wayland cannot.
- Keyboard: `Activated` is the only path into X focus (§6.2). On `Deactivated` the core releases all pressed keys
  (XWin `winKeybdReleaseKeys`, `winkeybd.c:464-483`) and sets focus None; on `Activated` the host must also send its
  current lock-modifier state (the existing `ShellEvent::LockState`, `surface/link.rs:17`).
- Feedback guard: `Moved/Resized/Restacked` carry a `HostSerial`; the core applies them as `ConfigureWindow` from
  `SERVER_OWNER` and acknowledges them through `acked`/`client_fields` (§4.2.1), so the host never re-applies its own
  move yet never misses a client change coalesced with it (XQuartz `no_configure_window`, `rootlessWindow.c:98-140`;
  XWin static `fRestacking`). No static flags.

Internal X actions use **typed core entry points**, not synthesized wire bodies (`for_server.rs:127-140` [F]):
`configure_window_as_server(state, backend, window, ConfigureValues)`, `set_input_focus_as_server`,
`send_client_message_as_server`, `kill_client_as_server`. Each splits the existing handler into parse + apply (the
pattern SetZero already used for focus, `focus_pointer.rs:182` [F]). Server-owned XIDs (the `_NET_SUPPORTING_WM_CHECK`
window) come from the server's existing resource allocator, not a scan (`for_server.rs:244-261` [F]).

### 4.5 Buffer handoff

The presenter lives in the `yserver` crate (it needs `DrawableStore` and the engine). Per tracked top-level it knows
the backing drawable (`Window.redirected_backing`, `resources/window.rs:1021`). With no scene consuming backing
damage in rootless mode, the presenter is the single consumer of `peek/ack_presentation_damage` (`store.rs:1352`,
:1373). A host declares how it takes pixels:

```rust
pub enum HandoffKind {
    /// The host's native surface is a Vulkan surface on the same VkDevice (macOS CAMetalLayer via
    /// KosmicKrisp, Windows VK_KHR_win32_surface). The presenter records a damage-limited copy from
    /// the backing into the acquired swapchain image and presents. No CPU copy.
    VulkanSurface,
    /// The host wants a dmabuf of the backing (Wayland linux-dmabuf; Linux only).
    Dmabuf,
    /// The host wants bytes (Wayland wl_shm, Windows UpdateLayeredWindow for depth-32 or shaped windows).
    Cpu,
}

pub trait RootlessHost {
    fn handoff(&self, id: ToplevelId, depth: u8) -> HandoffKind;   // per window: Windows picks Cpu for ARGB

    /// VulkanSurface: create the VkSurfaceKHR for this native window (host-side, on its UI thread);
    /// the presenter owns the swapchain (surface/swapchain.rs, one per top-level).
    fn create_surface(&mut self, id: ToplevelId, vk: &VkContext) -> io::Result<vk::SurfaceKHR>;

    // Readiness is not polled: the presenter tracks it from FrameEvents (§4.5.1).

    /// Dmabuf / Cpu delivery. Ownership passes to the host (it may forward the frame to its UI
    /// thread); it reports the outcome with a `FrameEvent` (below). Frames of one window are
    /// submitted in increasing `seq`, and the host must process and report them in that order.
    fn submit_dmabuf(&mut self, frame: DmabufFrame);
    fn submit_cpu(&mut self, frame: CpuFrame);

    /// The first frame for `id` was delivered (GPU path: presented): make the native window
    /// visible to the user now (§4.5.1).
    fn first_frame(&mut self, id: ToplevelId);

    /// Lifecycle (§4.2), on the core thread; hosts with a UI thread post them there.
    fn on_events(&mut self, events: &[RootlessEvent]);
}
```

- **Δ RL1:** the trait also has `frame_wanted(id)` (a Busy window has damage: arm the pacing signal, D7) and
  `surface_released(id)` (the presenter destroyed the window's last swapchain and its `VkSurfaceKHR`: the host may
  free the native window now). RL1's GPU copy is full-extent (D5); `Dmabuf` is answered with CPU frames until RL3
  (D4). The headless CLI came with RL1 Task 9 (`--headless --rootless`, a recording host), not RL2 (D2, D3).
- **GPU path (macOS, Windows):** reuses `SurfaceOutput`'s swapchain/present logic (`surface/swapchain.rs:166`,
  `present_pending` :325, out-of-date recreation :617), with the backing as the "slot". Depth 24 → opaque swapchain;
  depth 32 → `PRE_MULTIPLIED` composite alpha when the surface offers it (X ARGB visuals are premultiplied).

Frames handed to a host are **owned** and `Send`, so a UI thread can hold them past the call:

```rust
pub struct FrameSeq(u64);                 // per top-level, monotonic

pub struct CpuFrame {
    pub id: ToplevelId,
    pub seq: FrameSeq,
    pub backing_gen: u64,                 // backing allocation the bytes came from (§4.5, resize rule)
    pub width: u16, pub height: u16,      // backing extent = native content (§4.0)
    pub stride: u32,
    pub depth: u8,                        // 24: alpha byte undefined; 32: premultiplied ARGB
    pub pixels: Vec<u8>,                  // B,G,R,A, full extent; every byte = canonical image at seq (§4.5.2)
    pub damage: Vec<Rect>,                // backing pixels; the host may limit its upload to these
}
pub struct DmabufFrame { pub id: ToplevelId, pub seq: FrameSeq, pub backing_gen: u64,
                         pub fds: Vec<OwnedFd>, /* planes, modifier, format */ pub damage: Vec<Rect> }

/// Host → presenter, over the platform's own link (the `ShellEvent` channel, `surface/link.rs:17-34`,
/// drained in `Platform::on_platform_event`, `surface/platform.rs:879`), not through the core.
pub enum FrameEvent {
    SetupDone { id: ToplevelId },                                   // handshake done: ready for the FIRST frame
    Ready { id: ToplevelId },                                       // can take another frame
    Delivered { id: ToplevelId, seq: FrameSeq },                    // shown / committed
    Dropped { id: ToplevelId, seq: FrameSeq },                      // could not be shown
    Released { id: ToplevelId, seq: FrameSeq, pixels: Vec<u8> },    // bytes back for reuse (CPU only)
}
```

- **CPU path:** **asynchronous** readback. The presenter peeks the backing's damage snapshot
  (`peek_presentation_damage`, `store.rs:1352`), records a copy of the damaged rects into the window's staging
  buffer, submits, and continues; at a later tail, once the fence has signalled, it applies the staging rects to the
  window's canonical image and builds an owned `CpuFrame` from it (§4.5.2), then calls `submit_cpu`. Never a fence wait on
  the core thread (SetZero's per-frame synchronous `get_image_with`, `backend/rootless.rs:65-139` [F]). Damage comes
  from the store (exact rects), not a row hash (`topology.rs:490-528` [F]). At most one readback in flight per
  window; damage arriving meanwhile stays in the store for the next one.
- **When damage is acknowledged.** The store's ack is epoch-based: `ack_presentation_damage(snap)` drops only rects
  added at or before the snapshot's epoch and keeps later ones (`store.rs:1364-1397`). The presenter therefore holds
  the snapshot until the host reports `FrameEvent::Delivered` for that frame and acks then. On `Dropped`, a failed
  submit, or a swapchain acquire/present failure (GPU path), it does not ack: the damage is still in the store and the
  next frame re-covers it. The GPU path acks after a successful `vkQueuePresentKHR` of the copy.
- **Resize / reallocation cancels.** A backing reallocation (`reallocate_redirected_backing`, `redirect.rs:705`)
  returns a new `PixmapHandle` for a new pixmap (`kms/render/backend/trait_impl.rs:713-731`); `backing_gen`
  identifies that allocation. A readback whose `backing_gen` is stale on completion is discarded without delivery,
  its snapshot (of the old pixmap) is never acked, and the window is marked **fully damaged** at the
  new extent; a host receiving a `CpuFrame`/`DmabufFrame` whose extent no longer matches its native content (it
  resized in between) reports `Dropped`, which leads to the same full repaint. The GPU path recreates the swapchain
  at the new extent (`recreate_swapchain`, `surface/swapchain.rs:617`) and sends a full frame.
- A newly mapped window's first frame is always full-extent, whatever the store's damage says.
- **Dmabuf:** `promote_drawable_exportable` (`engine/export.rs:27`) on the backing; Linux-only; a later step for
  Wayland, not needed for macOS.

#### 4.5.1 Host readiness states

The presenter keeps one state per top-level and submits a frame only in `ReadyFirst` or `Ready`:

```
Setup ──SetupDone──▶ ReadyFirst ──first frame submitted──▶ Busy ──Ready──▶ Ready ──frame submitted──▶ Busy …
                                                             └──Delivered(first) ⇒ host.first_frame(id): visible
```

- **A new surface is eligible for its first frame as soon as its host-side setup handshake is done** (`SetupDone`),
  never gated on a frame callback or display-link tick: those only exist for a surface that already has content.
  Damage present at `SetupDone` (or none: the first frame is always full-extent, §4.5) is submitted in the same
  tail.
- `Ready` (frame callback, display-link tick, `WM_PAINT`/DWM) moves `Busy → Ready` for every later frame.
  `Delivered`/`Dropped` do not by themselves make the window ready; a host whose pacing is "one frame per
  delivery" sends `Ready` with it.
- **Liveness after a failure:** a host **must send `Ready` as soon as it can retry** after it reports `Dropped`
  (or after a submit fails on its side), because the pacing signal it would otherwise wait for may never come: a
  Wayland frame dropped before its commit requested no `wl_surface.frame` callback. This explicitly includes a
  failed **first** frame (the state goes back to `ReadyFirst`, the window stays invisible, and the retry is again
  full-extent) and a frame dropped because its extent no longer matches a resized surface (the retry carries the new
  `backing_gen`, full extent, §4.5). The damage of a dropped frame is never acked (§4.5 ack rule), so it is in the
  retry. A host that cannot retry yet (for example, a Wayland `configure` it has not acked) sends `Ready` once it can.
- **Visibility rule, consistent with readiness:** the window becomes visible to the user only after the first
  frame is delivered. Where the native system needs a surface to be mapped for a present to succeed, the host maps it
  *invisibly* during setup; the user-visible step is `first_frame`.
- A window that unmaps before `SetupDone` never gets a frame; a resize before `SetupDone` only changes the extent of
  the eventual first frame.

Per host:

| | setup handshake → `SetupDone` | first frame → visible (`first_frame`) | later `Ready` |
|---|---|---|---|
| Wayland (xdg-shell) | create `wl_surface` + `xdg_surface` + role, set title/app_id/parent/limits, **empty initial commit**, receive `configure`, `ack_configure` → `SetupDone` (no buffer may be attached before this; xdg-shell, `xdg_surface` "initial commit" rules) | the first buffer attach + commit maps the surface: `Delivered` = that commit was made | `wl_surface.frame` callback requested with each commit |
| Popup (xdg_popup) | same handshake with the positioner (`at`, §4.0) | same | same |
| macOS | `NSWindow` + `CAMetalLayer` created on the main queue and ordered in with `alphaValue = 0` [A] (a present to an ordered-out window may fail to acquire, `surface/swapchain.rs:8-10`), `VkSurfaceKHR` + swapchain created → `SetupDone` | first `vkQueuePresentKHR` succeeds → `alphaValue = 1` | display link of the window's screen |
| Windows | HWND created on the UI thread, shown **cloaked** (`DWMWA_CLOAK`) [A], surface/layered setup done → `SetupDone` | uncloak (or first `UpdateLayeredWindow` for the CPU path, which is itself the show) | `WM_PAINT`/DWM composition tick |
| Headless recording host (RL1) | immediate | immediate | immediate |

A later Wayland `configure` (compositor resize) is the host's to `ack_configure` with the commit that carries the next
buffer; it does not return the surface to `Setup`. The host reports the new size as `HostRequest::Resized`.

#### 4.5.2 CPU frames: canonical image and buffer reuse

Invariant: **every byte of a delivered `CpuFrame.pixels` equals the window's canonical image as of that frame's
`seq`.**

- Per top-level the presenter keeps a **canonical image**: a full-extent CPU copy of the backing at the current
  `backing_gen`. Each completed readback writes its damaged rects into it. A new `backing_gen` (resize) or a new
  window starts with a full-extent readback before anything is delivered.
- Frame buffers come from a small per-window pool (bound 3, like Xwayland's window buffers,
  `xwayland-window-buffers.c:137-158`, :396-477). Each pool buffer records the `seq` whose content it holds, and the
  presenter keeps the damage of the last pool-size frames. To fill a buffer for frame `n`, it copies from the
  canonical image the union of the damage of frames `k+1..=n`, where `k` is the buffer's recorded `seq`. A buffer
  whose content is unknown (fresh allocation, `k` older than the kept history, other `backing_gen`) gets a
  full-extent copy. `Released` returns a buffer with its `seq`; a buffer that is never released is never reused.
- `CpuFrame.damage` is everything that changed **since the last frame the host reported `Delivered`**: the union
  of the damage of frames `d+1..=n`, where `d` is the last delivered `seq`, so the damage of every dropped frame in
  between is included. If the kept history does not reach back to `d`, or nothing was delivered yet at this
  `backing_gen`, it is the full extent. This is for the host's own upload/`damage_buffer`; the pool-buffer fill
  above uses the buffer's own `k`, independently.
- **Order:** frames of one window are submitted in increasing `seq`, at most one readback is in flight, and the
  host processes and reports them in order; a `Delivered(n)` acks every held store snapshot up to `n`
  (§4.5, ack rule).

### 4.6 Where `RootlessHost` attaches

**Δ RL1 (as built):** no `rootless_host()` accessor. The presenter is a field of the surface platform's
`SurfaceBackendState` (rootless is surface-only, §7) and needs the store, engine, GPU and host at once; one seam
hook, `Platform::on_rootless_events` (no-op default), applies the batch to the presenter before the host sees it.
The presenter runs at the end of the surface `maybe_composite`, after the scene. The text below is the r3 design.

`Platform` gets one optional accessor: `fn rootless_host(&mut self) -> Option<&mut dyn RootlessHost>` (default
`None`, `seam.rs:28`). `XBackend<P>` implements `on_rootless_events` by forwarding to it and runs the presenter in
`maybe_composite` (trait :1043) **instead of** the scene when rootless; outputs given to the scene are empty, so it
stays inert. No Backend delegation wrapper; KMS and rooted macOS are untouched (`rootless_host` returns `None`).

`FrameEvent`s are posted on the platform link and wake the core as `Message::PlatformEvent`, which dispatches
`backend.on_platform_event` during step 2 of the iteration (`core_loop/run.rs:2793`), i.e. before the tail's
`maybe_composite`: a `Ready` or `Delivered` that arrives while the core is blocked is acted on in the same
iteration it wakes.

## 5. Coordinate model

```rust
pub enum CoordinatePolicy {
    /// Native positions are global and settable (macOS, Windows). X root = bounding box of the native
    /// desktop in X pixels; a top-level's outer corner *is* its native content origin (§4.0); native moves
    /// are applied to X, X moves are applied natively.
    Global { origin: (i32, i32), scale: Scale },
    /// Native positions are hidden (Wayland). X positions are kept as clients/WM set them and never sent to
    /// the host; popups are placed relative to their parent's X position.
    Opaque,
}
```

| | macOS | Wayland | Windows |
|---|---|---|---|
| Policy | Global | Opaque | Global |
| Root size | union of `NSScreen.frame` × S | bounding box of outputs (xdg_output logical) | virtual screen `SM_C[XY]VIRTUALSCREEN` |
| Root (0,0) | top-left of the union; Cocoa y flipped against the primary screen's height | first output's origin | `SM_[XY]VIRTUALSCREEN` (XWin, `winmultiwindowwindow.c:212`) |
| X pixel = | S × point, S = M3's global scale choice (1 or 2) [A] | logical pixel × buffer scale (v1: 1) | physical pixel, per-monitor-v2 DPI awareness [A] |
| X position → native | outer corner → content-rect origin (`setFrameOrigin` of `frameRectForContentRect:`) | not sent; popups: `at = O(popup) − O(parent)` (§4.0) | outer rect = client rect → `SetWindowPos` of `AdjustWindowRectExForDpi(client rect)` |
| Native move → X | `windowDidMove` → `Moved` (live during the drag, decision 4) | n/a | `WM_WINDOWPOSCHANGED` → `Moved` (live, coalesced per iteration; XWin waits for `WM_EXITSIZEMOVE`) |
| RandR outputs | one per NSScreen, CRTC at its X origin | one per wl_output | one per `HMONITOR` |

Rules for every policy:

- The native **content** rect is the X **outer** rect (§4.0); native decorations are outside it. Hosts with a frame
  keep their insets to themselves: `_NET_FRAME_EXTENTS` is 0,0,0,0, since no frame surrounds the window in X.
- RandR describes real native outputs; configuration requests (`SetCrtcConfig`, `SetScreenSize`) fail with a real
  error status rather than pretending to succeed (the M3 lesson, `plans/…-m3-window.md` "Client-driven resize is a
  silent lie") [C]. No RandR providers in rootless (Wine divides by zero otherwise, SetZero `kms/randr.rs` [F]).
- `ScreenLayout` from the host updates root size and RandR (`handle_host_container_resize`, `core_loop/run.rs:3554`,
  generalised) and emits `ScreenChanged`. Until the per-output RandR model lands (RL2), the core accepts exactly
  **one** host output and rejects any other count (no change, no event): the existing resize path sets the root and
  the first RandR output together, which is only true with one output.
- **Mixed scale (macOS, Windows).** One X pixel size for the whole root. macOS (decision 6): ONE global S in v1,
  fixed at startup (M3 `--scale`); a window on a screen of a different backing scale is scaled by the system or by
  our NEAREST blit (`surface/swapchain.rs` `set_copy_scale` :489). Documented limitation: X content is sharp only on
  screens whose backing scale equals S; RL2 tests a window moved between a 1× and a 2× display (§10). Windows: physical pixels are consistent under per-monitor-v2, so X apps simply
  look smaller on high-DPI monitors; `WM_DPICHANGED`'s suggested rect is ignored (keep the physical size) [A].
- Warp (`warp_pointer`, trait :3008): Global → native warp (`CGWarpMouseCursorPosition`, `SetCursorPos`) after
  conversion; Opaque → only inside the focused top-level via pointer constraints (Xwayland rule,
  `xwayland-screen.c:288-330`), else ignored.

## 6. Window management

### 6.1 Who answers MapRequest/ConfigureRequest

Nobody, by design. The server **does not** select SubstructureRedirect on the root, so MapWindow and ConfigureWindow
apply directly (`windows.rs:2390-2416`, :1798-1821 find no holder) and arrive as `Mapped`/`Configured` events.
This is what XWin (`winmultiwindowwm.c:1654-1696`) and SetZero (`topology.rs:371-383` [F]) do; a synchronous
WM round trip through a native window manager would stall clients on every configure.

The built-in policy (`yserver-core::rootless::wm`) then does what a WM must do, as `SERVER_OWNER`:

- announces EWMH: `_NET_SUPPORTING_WM_CHECK` (server-owned InputOnly window), `_NET_WM_NAME` = "yserver",
  `_NET_SUPPORTED` = exactly what §6.3 handles; `_NET_ACTIVE_WINDOW`, `_NET_CLIENT_LIST(_STACKING)` on the root;
- sets `WM_STATE` Normal on map and Withdrawn on unmap (Wine waits for it, SetZero `windows.rs:2295` [F]);
- sends a synthetic ConfigureNotify after a host-driven move (ICCCM 4.1.5; Java, XWin `:1294-1316`);
- `_NET_FRAME_EXTENTS` = CARDINAL[4] zeros (native decorations sit outside the X window), set at map and in
  answer to `_NET_REQUEST_FRAME_EXTENTS`, which EWMH allows before map (GTK waits for it);
- **placement** of a newly mapped Managed window without `USPosition|PPosition` (Global only): the host decides
  (`Mapped` carries the hints; macOS cascades, Windows `CW_USEDEFAULT` like XWin `:538-544`) and replies with
  `Moved`. Transients without position are centred on their parent by the core before `Mapped`.

### 6.2 Focus

- Native activation is the only source: `Activated { id }` → if `WM_HINTS.input != False`, `SetInputFocus(window,
  PointerRoot)`; if `WM_TAKE_FOCUS` is in `WM_PROTOCOLS`, send it (ICCCM 4.1.7, XWin `:901-936`); update
  `_NET_ACTIVE_WINDOW`.
- Popups are never activated (macOS non-activating panel, Windows `WS_EX_NOACTIVATE`/`MA_NOACTIVATE` like XWin
  `:1210-1222`, Wayland xdg_popup). Their keyboard input goes where X focus already is (the X client grabs).
- A client `SetInputFocus` on another of its windows stays X-only; `_NET_ACTIVE_WINDOW` from a client is a
  `WmRequest::Activate` (host may refuse: Windows foreground lock, Wayland needs xdg-activation).

### 6.3 Client requests to the WM

`handle_send_event` (`selection.rs:359`) on the root, in rootless mode, additionally feeds these to
`Rootless` (delivery to other root listeners is unchanged):

| X request | `WmRequest` | macOS | Wayland | Windows |
|---|---|---|---|---|
| `_NET_WM_STATE` add/remove/toggle FULLSCREEN | `SetFullscreen(bool)` | `toggleFullScreen:` | `set/unset_fullscreen` | borderless monitor-sized (not maximize, unlike XWin `:1723+`) |
| `… MAXIMIZED_VERT/HORZ` | `SetMaximized(bool)` | `zoom:` | `set/unset_maximized` | `ShowWindow(SW_MAXIMIZE)` |
| `… ABOVE / BELOW` | `SetLayer` | window level | ignored | `HWND_TOPMOST/BOTTOM` |
| `… HIDDEN`, `WM_CHANGE_STATE` Iconic | `Minimize` | `miniaturize:` | `set_minimized` | `SW_MINIMIZE` |
| `_NET_WM_MOVERESIZE` | `BeginMove/BeginResize { edges, button }` | `performWindowDragWithEvent:` [A] | `xdg_toplevel.move/resize` with last serial | `WM_NCLBUTTONDOWN` HTCAPTION / HT* edge |
| `_NET_ACTIVE_WINDOW` | `Activate` | `makeKeyAndOrderFront:` | xdg-activation (if any) | `SetForegroundWindow` (may be refused) |
| `_NET_CLOSE_WINDOW` | `Close` | `performClose:` | n/a → core closes | `WM_CLOSE` |

Minimizing does **not** X-unmap the window (decision 3): the core sets `WM_STATE` Iconic and `_NET_WM_STATE_HIDDEN` and keeps it
mapped (as compositing WMs do), so `Unmapped` keeps meaning "destroy the native window". The native state comes back
through `HostRequest::StateChanged` and is written to `_NET_WM_STATE`.

### 6.4 Native-driven actions

- `CloseRequested` → `WM_DELETE_WINDOW` if listed in `WM_PROTOCOLS`; otherwise the core does nothing. A close
  request never kills, whether or not the client supports `WM_DELETE_WINDOW` (unlike XWin `:887-899` and SetZero
  `for_server.rs:157` [F], which KillClient). **Force-kill (decision 12):** only on a *repeated* close request for
  the same window while the client is still alive, and only after the host shows an explicit confirmation naming
  the client (killing it may close several windows); the host then sends `HostRequest::ForceKill { id }`, the only
  request that kills. A host can see from `WindowInfo.protocols` that a window lacks `WM_DELETE_WINDOW` and word the
  confirmation accordingly. `_NET_WM_PING` may inform the dialog text, but an unanswered or unsupported ping never
  kills by itself.
- `Resized` → ConfigureWindow (size), respecting `WM_NORMAL_HINTS` (min/max/inc); the host should also enforce them
  natively (macOS `contentMinSize`/`contentResizeIncrements`, Windows `WM_GETMINMAXINFO`/`WM_SIZING`, Wayland
  `set_min/max_size`). Until the client repaints, the native window shows the old content at NorthWest.
- `Moved` (Global): applied live during a native drag (decision 4), one `ConfigureWindow` per iteration with the
  latest position (§4.2.1 rule 3 coalesces the rest).
- `Restacked` (Global): native z-order is authoritative among our windows; the core restacks X to match and
  acknowledges it (`Restacked { acked, from_client: false }`). X-initiated restacks (`XRaiseWindow`) arrive with
  `from_client: true`; macOS applies `orderWindow:relativeTo:`, Windows `SetWindowPos(hWndInsertAfter)` among our
  HWNDs, Wayland ignores it.

### 6.5 A real X WM or compositor in rootless mode

Not supported (decision 2). The core owns `WM_S0` (ICCCM 2.8) and answers `ChangeWindowAttributes(root,
SubstructureRedirect)` from any client with BadAccess — what a WM sees when another WM runs, so twm/xfwm4 exit with
their usual message. A compositing manager's `RedirectSubwindows(root, Manual)` gets BadAccess through `ManualTaken`
(`composite_damage.rs:141-171`), exactly as under Xwayland. Window-level Manual redirects of non-top-levels and
Automatic redirects keep working.

## 7. Mode selection

- Rooted vs rootless is chosen **at startup**, per platform: `--rootless` / `--rooted` (via `launch::parse_args`).
- **macOS (decision 1):** both offered. The app bundle (M3 `just macos-bundle`) defaults to rootless (what an
  XQuartz user expects); `just macos-run` and `--headless` default to rooted; `--rooted` keeps the M1–M3 single
  window for running a full desktop (twm, xfce) inside one window.
- **Wayland (SetZero):** rootless only; rootful Xwayland-style is out of scope.
- **Windows:** both are natural (XWin has `-multiwindow` and windowed); rootless first.
- **KMS:** never rootless; `rootless_host()` is `None`.
- **Headless** (`run_headless`): gains `--rootless` with a recording host (§10), for tests only.

## 8. How each backend plugs in

### 8.1 macOS (first consumer, M4)

- `SurfacePlatform` gets a `Mode { Rooted, Rootless(MacRootlessHost) }`. In rootless mode `output_layouts()`
  (`surface/platform.rs:841`) returns none for the scene; RandR comes from NSScreens.
- Native window: one `NSWindow` (Managed: Titled|Closable|Miniaturizable|Resizable unless `_MOTIF_WM_HINTS` says
  none; Popup: borderless `NSPanel`, non-activating, level above its parent or `NSPopUpMenuWindowLevel`) + its own
  `CAMetalLayer`, created on the main queue from `on_events` (generalising `macos/window.rs:23-109`). `ShellUi`
  grows per-window methods or is replaced by the event forwarder; `set_cursor` stays global.
- Pixels: `HandoffKind::VulkanSurface`; one swapchain per top-level via `vkCreateMetalSurfaceEXT`; paced by the
  display link of the screen the window is on.
- Input: `macos/input.rs` sends `PointerEnter/MotionLocal` with the event's window and its content-view
  `locationInWindow`, flipped and converted points × S to X pixels at the host edge (§4.0; the conversion
  `x_position` does today, `surface/input/pointer.rs:28`, minus its root clamp); `windowDidBecomeKey` → `Activated` (replacing today's `focus_gained`, `app.rs:183-196`).
- Native WM: `windowDidMove`/`windowDidResize`/`windowDidMiniaturize`/`windowShouldClose:` → `HostRequest`s; app
  activation brings all our windows forward (XQuartz `xp_window_bring_all_to_front`, `xprEvent.c:75-81`).
- Shape: transparent pixels outside the bounding shape (layer non-opaque) and `hitTest:` honouring the input shape
  [A]; both arrive in backing pixels (§4.0) and the host converts them to view points (÷ S).
- Bordered top-levels (`xterm -bw 5`, no WM to strip the border): the NSWindow content view is the outer rect, the
  border is drawn as backing pixels, clicks on it go to the top-level (§4.0; test in §10).

### 8.2 SetZero's Wayland backend → this interface

| Their code [F] | Here |
|---|---|
| `rootless/topology.rs` `Topology::sync` diff (:194-336), `is_toplevel` (:534) | `Rootless::reconcile` + `RootlessEvent`; same predicate |
| `HostWindows` trait (`mod.rs:109`): `create_toplevel`, `create_popup`, `reposition_popup`, `destroy`, `set_title`, … | `RootlessHost::on_events` (`Mapped`/`Configured`/`InfoChanged`/`Unmapped`) |
| `ToplevelSpec { title, class, size, parent, limits }` (`mod.rs:81`) | `Mapped { info: WindowInfo, geometry, kind }` |
| `PopupPlacement` + parent guess (`popups.rs:256-310`) | `ToplevelKind::Popup { parent }` (guess moves into core); host computes `at = O(popup) − O(parent)` (§4.0; their content-origin vs outer-corner mix is the probable border offset) |
| `popup_dismissed` (`popups.rs:77`) | `HostRequest::PopupDismissed` |
| `RootlessRenderer::read_image_into` / `image_version` (`mod.rs:52`) | presenter, `HandoffKind::Cpu` (async, owned `CpuFrame`) or `Dmabuf`; frame callbacks → `FrameEvent::Ready/Delivered` |
| `seat.rs` `pointer_enter/motion/leave`, `keyboard_enter/leave` | `HostInputEvent::PointerEnter/MotionLocal/Leave`, `HostRequest::Activated/Deactivated` |
| `PointerScope` (`server.rs:370`) | core pointer scope (§4.4) |
| `HostRequest::{Resize, Close}` + `carry_out` (`topology.rs:22`, :387) | `HostRequest::Resized { serial }`/`CloseRequested`; acks per §4.2.1 |
| `ServerWmRequest::MoveResize` (`server.rs:1089`), `selection.rs:470-499` | `WmRequest::BeginMove/BeginResize` |
| `for_server.rs` wire-body helpers | typed `*_as_server` entry points |
| `RootlessBackend<H>` + `delegate.rs` (1752 lines) | `Platform::rootless_host()`; no wrapper |
| `poll_deferred_input` as tick, `on_host_display_ready` (`trait_def.rs:898`) | reconcile in `run_iteration_tail` before `maybe_composite` (§4.1); keep their `BackendFdKind::HostDisplay` fd hook (generic, useful) |
| `set_host_output` (first output) | `HostRequest::ScreenLayout` (one output until RL2's per-output RandR, then all) |
| `cursor_key` polling | existing displayed-cursor change (`take_displayed_cursor_change`, trait :2968) |
| headless render node DRI3 (`render_node.rs` `open_headless`) | orthogonal; separate PR |

Their Wayland transport, toolkit, keymap and clipboard stay theirs.

### 8.3 Windows (design target)

- Native window: an HWND per `Mapped`, created on a **dedicated UI thread** that owns all HWNDs and pumps messages
  (Win32 requires the creating thread to pump). Core → UI thread only via `PostMessage` (never `SendMessage`, which
  blocks cross-thread; XWin's WM thread does exactly that, `winmultiwindowwm.c:1901-1904`). UI thread → core via
  `CoreSender`. The core thread never touches an HWND except through `vkQueuePresentKHR`.
- Styles from `WindowInfo` (the same table as XWin `winApplyHints`, done in the host from parsed data): Managed →
  `WS_OVERLAPPEDWINDOW` (or `WS_POPUP` for `Decorations::None`); transient → owner HWND; Popup →
  `WS_POPUP`, `WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`, owned by its parent; `skip_taskbar` → toolwindow;
  `WM_CLASS` → AppUserModelID for taskbar grouping.
- Coordinates: Global, physical pixels, process per-monitor-v2 DPI aware [A]; content rect ↔ window rect via
  `AdjustWindowRectExForDpi`; root origin = virtual-screen origin. `WM_WINDOWPOSCHANGED` → `Moved/Resized`
  (live, with serials), `WM_ACTIVATE` → `Activated`, `WM_CLOSE` → `CloseRequested`, z-order changes →
  `HostRequest::Restacked { serial }` (no static guards; the §4.2.1 acks do it). The UI thread owns the
  `HostSerial` counter and applies only `client_fields` from `Configured`, so a `WM_WINDOWPOSCHANGING` it caused
  itself is never echoed back.
- Pixels (decision 5: the choice is made in RL4 by measurement on yoga): candidates are `VulkanSurface` via
  `VK_KHR_win32_surface` (no per-pixel alpha on an HWND swapchain [A]), DXGI/DirectComposition, and `Cpu` →
  `UpdateLayeredWindow` (premultiplied BGRA, matches X ARGB; the owned `CpuFrame` goes straight to the UI thread).
  `SetWindowRgn` covers 1-bit shapes on opaque windows (XWin `winmultiwindowshape.c:82-199`). The interface does
  not depend on the outcome: `HandoffKind` is per window.
- Input: `WM_MOUSEMOVE` client coords → `PointerMotionLocal` (scaled by nothing: X pixels = physical), first move
  after leave → `PointerEnter`; `TrackMouseEvent(TME_LEAVE)` per HWND → `PointerLeave`; outside-window tracking via
  `WM_INPUT` raw mouse or a low-rate global hook rather than XWin's polling timer. Keys: scancodes → evdev codes
  in the host; keymap/layout mapping is a separate Windows work item.
- Hardware targets: x86_64 (silence, bee) and arm64 (yoga: Snapdragon X Elite, Qualcomm Vulkan; Mesa Dozen
  fallback) [A].
- What Windows asks of the core that macOS/Wayland do not: nothing new beyond the interface — Global policy,
  per-window `HandoffKind`, `Restacked` from the host, serial acks, owned frames for a UI thread. That is the check
  this section exists for.

## 9. Migration and milestones

Each milestone is a branch and a PR (squash unless it holds distinct changes).

- **RL0 — core module, no native backend.** `yserver-core::rootless`: server-owned Manual redirect, reconcile,
  `RootlessEvent`, `WindowInfo` parser, WM policy (EWMH announce, WM_STATE, client-message mapping, `WM_S0`,
  SubstructureRedirect refusal), pointer scope, local-coordinate input, `HostRequest` handling, typed
  `*_as_server` helpers, serial acks. `Backend::on_rootless_events` + the `run_iteration_tail` call before
  `maybe_composite`. No `next_wakeup` contribution: RL0 has no damage, readiness or readback to wake for, and every
  topology change already ends in a tail (§4.1). Unit tests (§10).
- **RL1 — presenter + headless recording host.** `yserver::rootless` presenter with GPU-copy and async CPU paths;
  the `next_wakeup` contribution (§4.1 "Wakeups"); `Platform::rootless_host`; `run_headless --rootless` with a recording host that logs events and dumps per-top-level
  frames; vng scenario.
- **RL1b — root reads (decision 8).** `GetImage`/`CopyArea` from the root in rootless mode compose on demand: the
  mapped top-levels' backings in X stacking order over the root background (decision 11: the root's background
  pixel, or its configured background pixmap tiled from the root origin where it has one, `background_pixmap`,
  `resources/window.rs:115-124`) into a private image, then read
  (generalising `SceneCompositor::root_readback`, `scene/root_readback.rs:58`). No standing root scene. Gate:
  `xwd -root` and x11vnc show the windows at their X positions.
- **RL2 — macOS rootless (M4).** `MacRootlessHost`, per-window CAMetalLayer swapchains, Global coordinates,
  activation, native move/resize/minimize/close, popups, `--rootless`/`--rooted`. Verified on eiger/air/m4.
- **Interface review with SetZero (now, decision 7).** Before RL0 code moves, SetZero reviews §4 (events, acks,
  input, frame handoff) against their compositor. Their port (RL3) proceeds **alongside RL2** once RL0 has settled.
- **RL3 — Wayland (SetZero).** They port `ywayland` onto `RootlessHost`. What we tell SetZero (jos sends it; nothing
  is posted by us):
  1. the shared layer lands upstream first, shaped after their `rootless` module (credit), with the differences in
     §8.2 and why (no delegation wrapper, reconcile before compose, async readback with owned frames, serial acks,
     typed server actions, all outputs);
  2. please send the unrelated fixes as separate PRs now (key `event_x/y` relative, grab child in grabbed pointer
     events, nonblocking accepted sockets, writer-fd polling, depth-24 alpha word copy, resize-expose paint — check
     the last against master's `3cb576bf`/`e1578d97`/`6a0eba68`), and the headless-render-node DRI3 work;
  3. join the interface review now, especially §4.0–§4.5 against their compositor (hyprix) needs;
  4. env switches (`YSERVER_BACKEND`, `YSERVER_POINTER_LOCK`) cannot go upstream; the binary or a CLI flag selects.
- **RL4 — Windows.** Later; starts with the rendering-path measurement on yoga (decision 5).
- **Separate cleanup (decision 9):** removing `host_x11/` and `nested.rs` (moving their live types) does not block
  RL0.

## 10. Testing

**Unit (RL0), on `ServerState` + `RecordingBackend` with a recording `RootlessHost`.** Each names the mutation that
must make it fail:

- Startup redirects root children Manual as `SERVER_OWNER`; a client created later is redirected at creation.
  Mutation: redirect only at map.
- Create + map + configure in one iteration → one `Mapped` with final geometry; unmap → `Unmapped`; destroy of a
  mapped window → `Unmapped` then `Destroyed`; client disconnect → same. Mutation: emit per request.
- Reparent a top-level under a non-root window → `Unmapped`/`Destroyed`; back to root → `Mapped`.
- Override-redirect → `Popup { parent }` with the parent order: transient-for, pointer top-level, containing
  top-level, topmost.
- `ToplevelId` is not reused when an XID is reused, and a remap gets a fresh id; host requests and input carrying
  the id of an earlier mapped lifetime (close, move, ack, pointer) are dropped. Mutation: use the XID, or keep the id
  across an unmap.
- `ScreenLayout` with one output resizes the root and that RandR output; with two it is rejected (until RL2).
- `WindowInfo`: `_NET_WM_NAME` over `WM_NAME`; `WM_NORMAL_HINTS` min/max/inc from a big-endian and a
  little-endian client; `_MOTIF_WM_HINTS` no-decor; `_NET_WM_WINDOW_TYPE` dialog. Test vectors are taken from real
  client captures or the ICCCM/EWMH/Motif struct layouts, not invented.
- `PointerMotionLocal` on a top-level with `bw = 5`: local (0,0) → root = outer corner, hit = the top-level
  (border); local (5,5) → content origin, event coordinates (0,0) in the window. Same for a bordered popup (the
  offset SetZero likely has, [F] `seat.rs:273-317`). Mutation: use `window_absolute_position` without `− bw`.
- `Reshaped` rects are the X shape translated by `(+bw, +bw)`; `Resized { W, H }` on a bordered window yields X
  content `(W − 2bw, H − 2bw)`. Mutation: drop either translation.
- Pointer scope: two overlapping top-levels with X order opposite to the native one; motion entered on the lower
  one hits it. Mutation: unscoped hit test.
- `PointerLeave` → LeaveNotify to the client, buttons released.
- `Activated` with `input=False` + `WM_TAKE_FOCUS` → no SetInputFocus, a TAKE_FOCUS ClientMessage;
  `Deactivated` → focus None, keys released.
- Acks (§4.2.1): `Moved(7)` alone → `Configured { acked: 7, client_fields: ∅ }`, synthetic ConfigureNotify sent;
  `Moved(7)` + client resize in one iteration → `acked: 7, client_fields: WIDTH|HEIGHT`; `Moved(7)`, client move,
  `Moved(8)` → `acked: 8, client_fields: ∅`; `Moved(7)` then client move → `client_fields ⊇ X|Y`; a host resize
  clamped by `WM_NORMAL_HINTS` → `client_fields ⊇ WIDTH|HEIGHT`; a no-op host request is still acked. Mutation:
  drop client fields when an ack is present (r0's `ChangeOrigin` behaviour).
- Loop order: extend `run_iteration_tail_drains_present_work_before_compositing` (`core_loop/run/tests.rs:3851`) —
  `on_rootless_events` is recorded after `drain_present_completions` and before `maybe_composite`; a window mapped
  by a request in this iteration is in that iteration's events. Mutation: reconcile in `before_block`.
- `_NET_WM_STATE` fullscreen / `_NET_WM_MOVERESIZE` / `WM_CHANGE_STATE` on root → the right `WmRequest`; also
  delivered to other root listeners.
- `CloseRequested` → `WM_DELETE_WINDOW` when listed; without it, nothing (no KillClient). `ForceKill` → KillClient.
  Mutation: KillClient on a `CloseRequested` without `WM_DELETE_WINDOW`.
- A client selecting SubstructureRedirect on root gets BadAccess; `RedirectSubwindows(root, Manual)` gets BadAccess.

**Presenter (RL1), lavapipe:** `next_wakeup`: unacked damage + host ready → now; host not ready → `None`;
readback in flight → its deadline only while in flight. Damage on a child of a top-level arrives as that top-level's damage in backing
pixels; the CPU path never waits on a fence on the core thread (assert via the engine's wait counter); the
store snapshot is acked only on `Delivered` (a `Dropped` frame's damage is in the next frame); a resize while a
readback is in flight discards it (stale `backing_gen`) and the next frame is full-extent at the new size; a
`CpuFrame` outlives the `submit_cpu` call (the recording host keeps it on another thread). Mutations: ack at
submit; deliver a stale-generation frame.
Readiness (§4.5.1): a new window with a recording host that withholds `SetupDone` gets no frame; `SetupDone`
alone (no `Ready`, no frame callback) yields the first, full-extent frame in the same tail, and `first_frame` fires
only on its `Delivered`; later frames wait for `Ready`. Mutation: gate the first frame on `Ready`.
Liveness (r3): (a) the host drops the **first** frame and sends `Ready` → the state returns to `ReadyFirst`, a
full-extent retry is submitted, the window stays invisible until that retry's `Delivered`; (b) the host drops a
frame because the surface was resized meanwhile and sends `Ready` → the retry is full-extent at the new
`backing_gen`. In both, the dropped frame's store damage is not acked and the delivered retry's
`CpuFrame.damage` covers it. Mutations: leave the window `Busy` after `Dropped` + `Ready`; ack on `Dropped`;
compute `damage` from the previous submitted (not delivered) frame. Canonical image
(§4.5.2): with three pool buffers released out of order and damage in disjoint rects per frame, every delivered
`CpuFrame` equals a full readback of the backing at its `seq`; a buffer older than the kept history gets a full
copy; frames reach the host in `seq` order. Mutations: copy only the current frame's damage into a recycled buffer;
reorder submission.
**Close (decision 12):** a single `CloseRequested`, or any number of unanswered pings, never kills; only
`ForceKill` does.

**vng (RL1):** a `rootless-headless` scenario under `tools/vng-scenarios/`: `run_headless --rootless`, start xterm,
xeyes (shaped) and an override-redirect menu (`xmessage` / a Tk menu); assert the recorded event log and that each
dumped top-level image matches `xwd -id` of that window. Self-asserting like the existing suite.

**Hardware (RL2):** eiger/air/m4: xterm, xeyes, gimp/inkscape (dialogs, transients, menus), a Tk app, Firefox; move,
resize, minimize, fullscreen, close, Cmd-Tab. Required cases:
- **bordered top-level:** `xterm -bw 5`; the border is visible, a click on it and on the content's first pixel
  reach the right window with the right coordinates (`xev`);
- **mixed scale (decision 6):** a window moved between a 1× and a 2× display, by drag and by `xdotool windowmove`:
  position stays consistent, input lands where the pixels are, the documented limitation (sharp only at S) is the
  only visible difference;
- live drag: the X position follows during the drag (`xev` ConfigureNotify stream).

## 11. Risks

- **Per-window swapchains on KosmicKrisp.** Menus and tooltips create and destroy swapchains at menu speed; creation
  latency could make menus late. Mitigation to measure: pool native popups, or use the CPU path for small
  short-lived popups.
- **Memory.** Each viewable window keeps a leaf image *and* each top-level a backing (§1.3). Whether redirected
  subtrees still allocate leaves must be measured before RL2 (the storage spec's "one image per top-level" half is
  not implemented).
- **Present timing.** Copy-only completions per native window need an MSC source per window (display link per
  screen on macOS, DWM on Windows, timer on Wayland as FR's `wayland/clock.rs` [F]). Present pacing work so far is
  per CRTC.
- **Damage completeness.** The presenter trusts render-side backing damage; a draw path that writes the backing
  without `store.damage` shows as a stale native window. The same gap already breaks the rooted scene, so it is a
  bug either way, but rootless makes it per-window visible.
- **Feedback loops** between native and X geometry/stacking (§4.2.1 acks); Windows `WM_WINDOWPOSCHANGING` storms.
- **Live drags** issue one ConfigureWindow per iteration during a drag; clients that repaint on every ConfigureNotify
  may lag the native window (decision 4 accepts this).
- **Apps that expect a WM** (`_NET_WM_PING`, frame extents before map, `_NET_WM_SYNC_REQUEST` during resize) may
  misbehave; Java needs the synthetic ConfigureNotify.
- **No X WM/compositor in rootless** excludes users who want picom shadows on macOS — acceptable, as Xwayland.
- **Windows-specific:** foreground-lock rules refuse `SetForegroundWindow`; HWND swapchains have no alpha; mixed-DPI
  makes X apps tiny on high-DPI monitors; the Qualcomm Vulkan driver on yoga is unproven for many small swapchains
  (Dozen fallback lacks features); Win32 keyboard layouts → XKB is a large separate item; Windows has no
  `SO_PEERCRED`-style client identity for `_NET_WM_PID` checks.

## Open questions for jos

None at r2: the r1 questions are answered as decisions 10–12.
