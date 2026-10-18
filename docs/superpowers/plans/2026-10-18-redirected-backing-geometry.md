# Redirected backing geometry (D, C, G) — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** a redirected window's backing holds what Xorg's redirected pixmap holds, after every client draw,
every resize and every tree change. Three reviewer-measured bugs from the #260 review are in scope:
- **D:** client drawing on a bordered redirected window lands `bw` up and to the left.
- **C:** a child's newly visible border is never painted.
- **G:** a child moved by `win_gravity` into a grown parent reads stale storage.

**Architecture:** one principle, four mechanisms.
- **Principle:** while a window is redirected, its backing is the only authoritative storage for it and for every
  descendant that is not independently redirected. The leaves of those windows go stale the moment the route
  flips (`resolve_window_paint_target`, `paint_target.rs:461`, sends all their paint to the backing). Nothing may
  read a stale leaf and put the result into a backing. Leaves are read only on the two real transitions: redirect
  activation (when nothing above is redirected) and unredirect restore.
- **D:** the core names the **window**, never its backing pixmap, as the target of a draw or a read. The backend
  already turns a window into its backing plus the border inset and the ClipByChildren clip.
- **G:** a resize that reallocates the backing becomes Xorg's `compReallocPixmap` + `compCopyWindow`. The leaf
  round trip goes away, and gravity-moved children are carried from the **old** backing into the new one.
- **C:** the core computes Xorg's `borderExposed` next to every clip-list exposure and paints it (`PW_BORDER`).
  Ring paints into a **shared** backing are clipped to the window's border clip.

**Tech stack:** `yserver-core` (`clip_list.rs`, `process_request/{windows,redirect,drawing,xshm,present_ext}.rs`,
`damage_fanout.rs`, `resources/pixmap_props.rs`, `backend/trait_def.rs` + `recording.rs`), the KMS portable backend
(`crates/yserver/src/kms/render/backend/portable/{redirect,windows,paint_target}.rs`), `render_acceptance`
(lavapipe), and the vng suite (`tools/vng-scenarios`, `golden=live`).

**Status:** reviewed by codex; review corrections and jos's decisions applied (see "Decisions" at the end).

**Branch:** `fix/redirected-backing-geometry`, cut from master `54813f1e` (includes #260).

## Measurements (oracle)

Throwaway probe `target/plan-rbg/rbg-probe.c`. It is run by `tools/vng-shot.sh` in the guest, once with
`--server xorg` (21.1.24) and once with `--gpu none --binary target/debug/yserver` (54813f1e). All windows are
override-redirect, `RedirectSubwindows(root, Manual)` is set, and no WM runs. Pixels are read through
`NameWindowPixmap` + `GetImage` on the pixmap. Colours: red = border, blue = content background, green = sibling
or draw.

| Case | Point (pixmap coords) | Xorg | yserver |
|---|---|---|---|
| **D** W 100×100 bw 4, `PolyFillRect(0,0,10,10)` blue | (1,1) (3,3) | ff0000 | 0000ff |
| | (4,4) / (13,13) / (14,14) | blue / blue / white | blue / **white** / white |
| | (60,60): green child C, fill (40,40,40,40), ClipByChildren | 00ff00 | **0000ff** |
| | `GetImage(W,-1,-1)`; `CopyArea(W→pixmap)` at (9,9) | ff0000; 0000ff | **000000; ffffff** |
| **C** child (80,80) 50×50 bw 4 in W 100→200, both map orders | ring (81,105) (105,81) (135,105) (105,135) | ff0000 | **000000** |
| | content (90,90) (120,120); child Expose rects | blue; 16×16, then 34×16 + 50×34 | identical |
| **CF** same C under an *unredirected* W inside a redirected frame F | ring, 4 points | ff0000 | **ffffff** (F's bg) |
| **G** SE-gravity child (60,60) 30×30 bw 3, W 100→200 | ring (161,178) … (178,194) / content (178,178) | red / blue | **000000 / 000000** |
| | vacated (75,75); child Expose after resize | white; none | white; none |
| **GS** same, W 200→100 (child moves up-left) | ring / content | red / blue | red / blue (works: the carry lands inside OLD) |
| **U** sibling S above C's top-left ring, then `UnmapWindow(S)` | ring (11,20) (20,11) (11,11) | ff0000 | **00ff00** (S's pixels) |
| **M** S mapped above, then C mapped under it | ring (11,20) (20,11) under S | 00ff00 | **ff0000** (painted over S) |
| **N** C inside redirected W, drawn green, then `RedirectWindow(C)` | (3,3) (10,10) / (30,30) | green / blue | **505050** (stale leaf) |

Damage seen by a `ReportRawRectangles` object on W:
- **C resize, Xorg:** the copy `80,80 20x20`; W's background (4 rects); the child's border paint `80,80 58x58`; the
  child's background `84,84 50x50`; then the child's Expose events.
- **C resize, yserver:** no border paint, and the child's background at `96,80 34x16` and `80,96 50x34`. That is the
  child region shifted by `x` but not by `bw`, because `accumulate_damage_region_to_state`
  (`damage_fanout.rs:386-415`) translates by `current_win.x` and leaves out `border_width`.
- **U unmap:** Xorg reports `10,10 20x20` (ring) and `14,14 16x16`. yserver reports `10,10 16x16` only.

This is a **D2** bug of the same family as D, with the same cause, and it is in scope: Task 1.

## Root-cause model

### yserver

- **Leaf storage.** Every InputOutput window gets a leaf (`allocate_window_leaf`, `windows.rs:1616`). The leaf has
  the bordered extent, and its content sits at the allocation's `content_offset` (`storage_content_offset`,
  `paint_target.rs:719`). The ring is painted into the leaf by `paint_window_border` (`windows.rs:1030`).
  `sync_window_leaf_storage` (`:413`) reallocates a leaf on resize, and `relocate_leaf_content_in_place` (`:694`)
  moves content inside one leaf when only the border width changed. Both are **leaf-only**: they skip a window whose
  leaf has a `redirected_target` (`:2088`, `:396`).
- **Redirect backing.** The core sizes a pixmap to the bordered extent (`bordered_backing_extent`,
  core `redirect.rs:638`). The backend `backend_redirect_allocate_redirected_backing`
  (backend `redirect.rs:799-930`) then does four things:
  1. records `content_offset = bw`;
  2. `seed_backing_from_parent` (`:102`), which is `compNewPixmap`'s parent copy;
  3. `overlay_backing_inferiors` (`:225`), which copies W's and its descendants' **leaves** into the backing per
     `plan_backing_inferiors` (`:333`) → `collect_backing_inferiors` (`:363`) → `push_inferior_rects` (`:447`) +
     `push_inferior_ring` (`:492`, added by #260);
  4. flips the route (`set_redirected_target`) and paints W's own ring.
- **Routing.** `resolve_paint_target` (`paint_target.rs:429`) walks from a **window** xid to the first redirected
  ancestor-or-self. On the way it adds `x + bw` per level and builds the content clip and the ancestor bounds. A
  **non-window** xid goes to `resolve_non_window_paint_target` (`:446`): offset `(0,0)`, no clip. ClipByChildren is
  likewise keyed on a window xid: `compute_subwindow_mode_clip` returns `None` unless `windows.contains_key(host_xid)`
  (`clip.rs:539-546`).
- **Unredirect.** `backend_redirect_release_redirected_backing` (backend `redirect.rs:1036`) calls
  `sync_window_leaf_storage_to_geometry` (`windows.rs:165`, `LeafContent::Discard`) for each routed window, then
  `restore_leaves_from_backing` (`windows.rs:200`), which is the seed plan read backwards.
- **Which storage is authoritative when:**
  - before W is redirected, with nothing above it redirected: W's and its descendants' leaves;
  - while W is redirected: W's backing for the whole non-independently-redirected subtree, and those leaves are
    stale;
  - at unredirect: the backing is copied back into the leaves, which become authoritative again.
- **Resize of a redirected W** (`handle_configure_window`, core `windows.rs:1620`):
  1. The backend configures W.
  2. `apply_win_gravity` moves children in the core (`windows.rs:1892-1934`). Each moved child gets
     `backend.configure_subwindow`, which carries its pixels inside the **current (old)** backing
     (`shared_backing_move_source` / `carry_shared_backing_pixels_on_move`, `paint_target.rs:780-905`).
  3. `rotate_redirected_backing_on_resize` (core `redirect.rs:643`):
     - retain OLD, then **release** OLD, which runs the unredirect path: W's leaf is discarded, then the leaves are
       restored from OLD **at the new geometry**;
     - **allocate** NEW: parent seed, then the leaf overlay, then the route flip and the ring;
     - copy OLD content `min(old,new)` → NEW at the content inset (`:865-905`);
     - drop OLD, then damage the ring.
  4. Exposures (`TreeChange::exposed`, `windows.rs:2063-2075`) paint **backgrounds** and send Expose.

Why each bug happens:
- **D:** `host_drawable_target` (`pixmap_props.rs:286-309`) returns the **backing pixmap** xid for a redirected
  window. Every path that passes it on gets the non-window resolve, so the draw lands at offset 0 and loses
  ClipByChildren. #260 worked around it in three paths only: ClearArea (`windows.rs:148-155`), CopyArea
  (`drawing.rs:1398-1410`) and the Present paint destination (`present_ext.rs:1860-1866`).
- **C:** the core has no border exposure. `clip_list.rs` builds only clip lists, and `send_window_exposures`
  (`windows.rs:83`) paints only backgrounds. The ring in the grown strip therefore comes from one of two places:
  - the release/allocate round trip, which samples OLD beyond its extent and writes black into the leaf ring, which
    the overlay then copies (C);
  - whatever the shared backing held before (CF: F's white).

  The same gap explains U: an unmap exposes C's ring, and nobody paints it. M is the other half: backend ring paints
  (`paint_window_border`) into a **shared** backing are not clipped by higher siblings.
- **G:** the carry runs in OLD before the realloc. The child's new position lies outside OLD, so the copy is clipped
  away. The restore then reads OLD at the new position (black), and the overlay seeds that black into NEW. On a
  shrink (GS) the carry lands inside OLD and survives the round trip, which is why GS works.
- **N** (same principle): `overlay_backing_inferiors` also runs when W already has a redirected **ancestor**. Then
  W's subtree leaves are stale, and they overwrite the correct parent seed.

### Xorg (21.1.24, `../xserver`)

- **Pixmap layout.** `compAllocPixmap` (`composite/compalloc.c:600-618`) allocates
  `w + 2bw` × `h + 2bw` at `screen_x = drawable.x - bw` (`compNewPixmap`, `:532-545`). It seeds the pixmap with
  `CopyArea(parent → pixmap, IncludeInferiors)` (`:549-560`), then calls `compSetPixmap(pWin, pPixmap, bw)` (`:620`).
- **Pixmap for the subtree.** `compSetPixmapVisitWindow` (`composite/compwindow.c:122-141`) gives the pixmap to W
  and to every descendant that is not redirected. It then reruns `SetWinSize`/`SetBorderSize` and queues
  `compRepaintBorder` (`:103-117`: `borderClip − winSize`, `PW_BORDER`) for each visited window when `bw != 0`.
- **Coordinates.** fb draws at `drawable.x − screen_x` (`fb/fb.h:445`, `__fbPixOffXWin`).
  `compPositionWindow` keeps `screen_x = drawable.x − bw` (`compwindow.c:240-252`). So every draw to W, or to any
  descendant sharing W's pixmap, is offset by that window's content origin inside the pixmap. This is exactly
  `resolve_window_paint_target`'s accumulation. A redirected window's `winSize` is its own box, unclipped by its
  parent (`SetWinSize`, `dix/window.c:1713-1730`), and its clip universe is its `borderSize`
  (`mi/mivaltree.c:231-239`).
- **Resize.** `compConfigNotify` (`compwindow.c:795`) calls `compReallocPixmap` (`compalloc.c:670-712`).
  - When the bordered extent changes, it builds a **new** pixmap through `compNewPixmap` (a parent seed, nothing
    from the old pixmap) and keeps the old one as `cw->pOldPixmap`.
  - `miResizeWindow` (`mi/miwindow.c:342-620`) groups the children by `winGravity` (`:388-399`), moves them
    (`ResizeChildrenWinSize`, `dix/window.c:1845`) and copies each group's old `borderClip ∩ old winSize` to its
    new place through `CopyWindow` (`:543-575`). For a redirected W that is `compCopyWindow`
    (`compwindow.c:489-528`), which copies **from `pOldPixmap`**.
  - `miRecomputeExposures` (`miwindow.c:311-339`) sets a child's `exposed = clipList − copied` and
    `borderExposed = (borderClip − winSize) − copied`.
  - `miHandleValidateExposures` (`:211-240`) paints `borderExposed` with `PW_BORDER` **before** each window's
    `WindowExposures`. `miComputeClips` builds `borderExposed` the same way for every validated window
    (`mi/mivaltree.c:362-380`), so map, unmap, restack and shape changes paint newly visible borders too.
- **Unredirect.** `compSetParentPixmap` + `compRestoreWindow` (`compalloc.c:641-661`, `:232-256`) copy the window's
  pixmap back into the **parent's** pixmap.

## Global Constraints

- **Xorg is the oracle.** Every expected pixel comes from a vng `--server xorg` run of the committed probe (Task 0),
  except where it follows from protocol arithmetic: the content origin is `bw`, which Xorg confirms in row D.
- **`bw == 0` is identity.** No new backend call, no new damage and no new GPU work for unbordered windows. The vng
  goldens `expose-order` and `damage-after-map` use only unbordered windows and must stay byte-identical.
- **No leaf reads into a backing** except on redirect activation with no redirected ancestor (`overlay_…`) and on
  real unredirect (`restore_…`). Add a `debug_assert!` with no side effects that the overlay root has no redirected
  ancestor.
- **No env switches and no test-only forks in production bodies** (project rule).
- **Gate per task:**
  1. `cargo +nightly fmt` first;
  2. `env -u RUSTC_WRAPPER cargo build --bin yserver`;
  3. `cargo test -p yserver-core --lib` and `cargo test -p yserver --lib`;
  4. `VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test -p yserver
     --test render_acceptance -- --ignored`;
  5. `cargo clippy --all-targets -- -D warnings` (also with `--features xdmcp`);
  6. `tools/check-layout.sh`;
  7. the vng `redirected-backing` scenario (both legs).

  Every command runs under `env -u RUSTC_WRAPPER`; before each commit the full set is
  `cargo test --workspace --all-targets --locked` and the same with lavapipe and `-- --ignored`.
- **Tests:**
  - New acceptance tests go in a new `crates/yserver/tests/render_acceptance/redirected_geometry.rs`, registered in
    `main.rs`. `resize.rs` is 1491 lines already.
  - Every test names its mutation, and the mutation is run once.
  - Red first, against the Xorg value. Never a hypothesis-derived value.
- **Commits:** one per task, made after that task's automated gate passes. The HW smoke runs once, at the end
  (Task 6), on the whole branch. Trailer
  `Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>`. Never a session URL.

## Review Focus

1. **The Present completion fence** must still attach to the backing after D: `completion_dst_host_xid` keeps
   backing identity (Task 1, `present_completion_keeps_backing_identity`).
2. **NameWindowPixmap aliases and GLX TFP exports** across a realloc that no longer goes through release+allocate
   (Task 2, `realloc_retargets_aliases_and_glx_exports`).
3. **A real unredirect** (`mpv` `_NET_WM_BYPASS_COMPOSITOR`) still restores leaves (the existing
   `unredirect_restores_the_window_leaf_from_the_backing` stays green). Only the rotate stops calling the restore.
4. **Ordering inside a resize:** realloc, then the NW content copy, then the gravity carries from OLD, then OLD
   dropped, then border exposures and background exposures. Test `resize_call_order_matches_xorg` (Task 2).
5. **Border damage on W itself:** the rotate already damages W's ring. yserver keeps ONE report; Xorg reports it
   twice (`borderExposed` in HandleExposures, then the `compRepaintBorder` work proc). That difference is measured
   and documented, not matched: the new border exposure must not add a second report for W's own ring.

---

### Task 0: Oracle probe and vng scenario (test only, red)

**Files:**
- Create: `tools/vng-scenarios/redirected-backing-probe.c`, from `target/plan-rbg/rbg-probe.c`, cleaned up: one
  `key=value` line per fact and no XIDs on fact lines, in the style of `tools/border-damage-probe.c`. Cases D, C (two
  map orders), CF, G, GS, U, M and N; the D sub-cases PutImage, ImageText8, PolyText8, CopyPlane, ShmPutImage and
  ShmGetImage; and Damage `RawRectangles` on W for C and U.
- Create: `tools/vng-scenarios/redirected-backing.sh` with a `# golden: probe.log` line, in the style of
  `damage-after-map.sh`. The probe writes the raw damage rectangles to a separate `damage-raw.log`, which is
  **not** a golden file; `probe.log` holds only pixels and damage **coverage**, so `golden=live` diffs exactly what
  gates.
- Modify: `tools/vng-scenarios/suite.list`. Add `redirected-backing guest=redirected-backing.sh timeout=300
  golden=live`, **commented out** until Task 5 turns it green, so nightly CI does not go red.

- [ ] **Step 1:** write the probe. `probe.log` damage lines print the **union coverage** per phase; the raw rects go
  to `damage-raw.log` only. Xorg reports `extents ∩ clip` per fill (`damagePolyFillRect`,
  `miext/damage/damage.c:1194`) and yserver reports the region, so raw rects are informational and coverage is the
  gate. Raw-rect granularity is out of scope.
- [ ] **Step 2:** run both legs; the Xorg leg must reproduce the Measurements table. Save the Xorg `probe.log`
  values into the test constants of Tasks 1-4.
- [ ] **Step 3:** commit `test(vng): redirected-backing pixel/damage probe against Xorg (not yet in the suite)`.

### Task 1: D: windows are drawn and read through the window (and D2: damage inset)

**Files:**
- Modify: `crates/yserver-core/src/resources/pixmap_props.rs:277-326`. `host_drawable_target` returns
  `Window { host_xid: window.host_xid, depth: window.depth }` whether or not the window is redirected. Delete the
  "typed lie" branch and its comment.
- Add: `ResourceTable::present_completion_host(id) -> Option<u32>`, which returns the backing pixmap for a redirected
  window and otherwise the window's host xid. It is used **only** at `present_ext.rs:1950` and `:2341`
  (`completion_dst_host_xid`), so fencing stays byte-identical.
- Simplify the three #260 workarounds, which are now redundant:
  - `windows.rs:148-155` goes back to `host_drawable_target`;
  - the CopyArea override `drawing.rs:1398-1410`;
  - `paint_dst_host_xid` at `present_ext.rs:1866`.

  Behaviour is identical; this is optional and a separate hunk in the same commit.
- Modify: `crates/yserver-core/src/core_loop/damage_fanout.rs:385-410` (D2). The ancestor step translates by
  `x + border_width` (the child's content origin in the parent's content space, `dix/window.c` `drawable.x =
  parent.x + x + bw`) before clipping to the parent's content.
- Modify: `crates/yserver-core/src/core_loop/process_request/redirect.rs:32` doc and
  `resources/tests/window.rs:353` (`host_drawable_target_redirected_window_returns_backing_xid`, which flips to
  returning the window).

**Paths fixed by the one change.** Each resolves a drawable with `host_drawable_target` and hands the xid to a
backend op that calls `resolve_paint_target`:
- PutImage `drawing.rs:90`
- ImageText8/16 `:327`/`:363`
- PolyText8/16 `:624`/`:668`
- PolyPoint/Line/Segment `:702`/`:751`/`:791`
- PolyRectangle/Arc, FillPoly `:825`/`:859`/`:895`
- PolyFillRectangle/Arc `:934`/`:974`
- GetImage `:495`, which goes to `backend_draw_get_image`, `portable/draw.rs:2575-2600`. That function already adds
  `target.offset()` and is border-inclusive, as Xorg's `DoGetImage` is.
- CopyPlane src/dst `:1823-1824` and its exposures `:1881`
- MIT-SHM PutImage `xshm.rs:400` and GetImage `:562`

**Paths already right or out of scope:**
- RENDER resolves the window at CreatePicture (`render.rs:185-193`), and the other RENDER ops use the picture.
- DRI3 is pixmap-only (`dri3.rs:154/264/312/389`).
- GLX has no window draw target: `glx.rs:552-597` stores only the xid, and GL presents through Present.
- WarpPointer (`focus_pointer.rs:620`) is ignored by KMS.
- The root (`reset.rs:466`) is never redirected.

- [ ] **Step 1: tests.**
  - Core, `RecordingBackend`, in `process_request/tests/drawing.rs`:
    - `poly_fill_rect_on_a_redirected_window_names_the_window`: `FillRectangle.host_xid == window host`.
      Mutation: restore the backing branch.
    - Same for `PutImage` and `CopyPlane` (`RecordedCall::{PutImage,CopyPlane}`).
    - `present_completion_keeps_backing_identity`: the completion xid is the backing.
  - Core, `damage_ext.rs`:
    - `a_bordered_childs_damage_reaches_the_parent_at_its_content_origin`: child (80,80) bw 4, region (0,0,10,10).
      The parent's report is `84,84 10x10`. Mutation: drop `+ bw`.
  - Acceptance, `redirected_geometry.rs`:
    - `client_draws_land_in_a_bordered_redirected_windows_content`, one sub-case per op. W 100×100 bw 4, red
      border, white bg; a blue 10×10 at (0,0); green child C (50,50) 20×20. Assert the backing at (1,1) and (3,3) is
      `ff0000`, (4,4) and (13,13) are `0000ff`, and (14,14) is `ffffff`. The ClipByChildren sub-case asserts (60,60)
      is `00ff00`.
    - `get_image_and_copy_area_read_a_redirected_window_through_its_content`: through the core handler (opcode 73),
      reply read from the client's `outbound` (add `ProtoFixture::take_reply`). Expect (0,0) and (9,9) blue,
      (10,10) white, (-1,-1) red.
- [ ] **Step 2:** run them; they fail with the yserver column values above.
- [ ] **Step 3:** implement.
- [ ] **Step 4:** run the gate; D rows match Xorg in the vng probe. Also run the xts5 `Xlib9` IncludeInferiors cases
  touched by #133 step 3 (jos: yes). If xts cannot run locally, note it and skip; jos runs xts on bee.
- [ ] **Step 5:** commit `fix(composite): draw on and read a redirected window through the window, not its backing
  pixmap`.

### Task 2: G: a resize reallocates like `compReallocPixmap` and carries gravity children from the old backing

**Files:**
- Modify: `crates/yserver-core/src/backend/trait_def.rs` (next to `allocate_redirected_backing`, ~L1436). Add
  `fn reallocate_redirected_backing(&mut self, origin, host_window: WindowHandle, old: PixmapHandle, width, height,
  depth) -> io::Result<PixmapHandle>`. The default is an error, as for `allocate_…`. Add a `RecordedCall` in
  `recording.rs`.
- Modify: backend `portable/redirect.rs`, adding `backend_redirect_reallocate_redirected_backing` next to `:799`:
  - `create_pixmap` NEW and `set_content_offset(bw)`;
  - `seed_backing_from_parent` (`compNewPixmap`), and **no** `overlay_backing_inferiors`;
  - move `host_window_to_backing[W]` OLD→NEW and `set_redirected_target(W, NEW)`;
  - `set_scene_participating(OLD, false)`;
  - drop OLD's Reason-1 hold (`alias_registry.decref`, freeing nothing while the rotate retain holds it);
  - register NEW's hold, `wake_for_damage`, and paint W's ring.

  The function does **not** call `sync_window_leaf_storage_to_geometry` or `restore_leaves_from_backing`. Wire it
  in `trait_impl.rs` next to `:695`.
- Modify: `crates/yserver-core/src/core_loop/process_request/redirect.rs:643-960`. In the realloc branch, replace
  `release_redirected_backing` + `allocate_redirected_backing` with `reallocate_redirected_backing`. Keep:
  - the rotate retain;
  - the alias and GLX retarget (`:785-835`);
  - the NW content copy (`:865-905`);
  - the drop, the scene participation and the ring damage.
- Modify: the gravity-carry sequence in `handle_configure_window` (`windows.rs:1892-1947`):
  1. snapshot the gravity moves (`apply_win_gravity` already mutates core state; keep its return value);
  2. run `rotate_redirected_backing_on_resize` **first**;
  3. issue the children's backend moves through a new `Backend::move_subwindows_for_gravity(origin, parent_host,
     moves: &[(u32, i16, i16)], carry_from: Option<PixmapHandle>)`. `carry_from` is `Some(OLD)` only when the
     rotate reallocated; the rotate returns that, still retained. Release OLD after the moves.
  4. send GravityNotify as today, after the moves.
- Modify: backend `paint_target.rs:780-905`.
  - `shared_backing_move_source` takes an optional source backing. When it is given, the source target is the
    window's **old** offset re-based into OLD: the same walk, with the owner's term taken from OLD's
    `content_offset`.
  - `carry_shared_backing_pixels_on_move` accepts `source.backing != target.backing` only when the source is
    `carry_from`.

  The pieces stay `old borderClip ∩ new borderClip` (`shared_backing_move_pieces`). That is Xorg's
  `gravitate[g] ∩ oldRegion` copied into the new clip, so the exposure diff that follows covers exactly what the
  copy did not. Children are carried bottom-to-top by `stack_rank`, so a higher sibling wins an overlap at the
  destination.
- **Failure path.** OLD stays retained until both the NW content copy and every gravity carry from it have
  finished; only then is it released. If the retain or the reallocation fails, the old route stays valid (W still
  targets OLD, OLD keeps its Reason-1 hold, and the carries fall back to `carry_from = None`). Nothing ever copies
  from a freed source.
- Unchanged: the non-realloc paths. A pure move, and a resize of an **unredirected** W inside a redirected ancestor,
  keep `carry_from = None` and the in-place carry. Resizing W there does not reallocate the ancestor's backing.

- [ ] **Step 1: tests.**
  - Acceptance:
    - `gravity_moves_a_child_into_the_grown_area_with_its_pixels` (G): ring red at 4 points, content blue at
      (178,178), vacated (75,75) white. Mutation: `carry_from = None`.
    - `gravity_moves_a_child_up_left_on_shrink` (GS, a guard; green today).
    - `realloc_does_not_round_trip_through_leaves`: draw blue into child C's content through the backing, then
      corrupt C's leaf with a privileged fill, then resize W. C's content in NEW is still blue. Mutation: put the
      overlay back in `reallocate`.
  - Core: `resize_call_order_matches_xorg` (`RecordingBackend`): `ConfigureSubwindow(W)`, `ReallocateRedirected`,
    `CopyArea(OLD→NEW)`, `MoveSubwindowsForGravity{carry_from: OLD}`, `DropBackingStorage(OLD)`, then the exposure
    fills. Mutation: gravity before the rotate.
  - Core: `realloc_retargets_aliases_and_glx_exports`: adapt the existing rotate alias tests to the new call.
- [ ] **Step 2:** run them; G fails as measured (`000000`).
- [ ] **Step 3:** implement.
- [ ] **Step 4:** run the gate. The unredirect restore tests in `render_acceptance/redirect.rs:97,185` stay green.
  Run the existing border-damage A/B with `tools/border-damage-probe.c`; its output must not move away from Xorg.
- [ ] **Step 5:** commit `fix(composite): reallocate a resized redirected backing from the parent and carry gravity
  children from the old one`.

### Task 3: C: paint newly exposed borders (`borderExposed` / `PW_BORDER`)

**Files:**
- Modify: `crates/yserver-core/src/core_loop/clip_list.rs`.
  - Add `border_clip` beside each clip list. A window's border clip is its `borderSize` (`border_size_in_parent`,
    `:103`), translated into its content space (negative coordinates) and intersected with the same universe
    `child_universe_with` (`:207`) cuts it from, before the `win_size` intersect. A redirected window's border clip
    is its own `borderSize` (`mivaltree.c:233-239`).
  - Return `(clip, border)` per window from `clip_lists_of` (`:356`), `walk` (`:279`), `clip_lists_under` (`:303`)
    and `subtree_clip_lists` (`:269`).
  - `newly_exposed` (`:439`) returns `(window, exposed, border_exposed)`, with
    `border_exposed = (border_after − border_before) − win_size`. It is empty unless `border_width > 0`.
  - `whole` still applies only to content (`miwindow.c:466-472`). The border of the `whole` window stays a diff,
    because Xorg uses `borderVisible` there (`miwindow.c:418-429`).
- Modify: `windows.rs`. At every exposure site (`:509`, `:638`, `:2063-2075`, `:2566`, `:2863`,
  `shape_xfixes.rs:240`, `composite_overlay.rs:161`), paint each window's `border_exposed` **before** its
  `send_window_exposures`, as `miHandleValidateExposures` orders it, through a new
  `paint_window_border_region(state, backend, origin, window, rects)`. It:
  - calls `backend.paint_window_border_rects(origin, host, rects)`;
  - then calls `accumulate_damage_region_to_state(state, window, rects)`;
  - does nothing at `bw == 0` or when the region is empty.
- Modify: `trait_def.rs` and `recording.rs`. Add `fn paint_window_border_rects(&mut self, origin, host_xid: u32,
  rects: &[RegionRect]) -> io::Result<()>`; the default is a no-op.
- Modify: backend `portable/windows.rs`. Add `backend_windows_paint_window_border_rects`. It works like
  `paint_window_border` (`:1030`), but intersects `border_ring_rects(outer, inner)` with `rects` translated by
  `target.offset()`, then does the same solid or tiled fill (`paint_border_ring_tiled`, `:1289`).
- Map: `send_map_exposures` (`windows.rs:187`) keeps the realize-time ring paint and its whole-ring damage. Task 4
  clips that paint.

- [ ] **Step 1: tests.**
  - Core, pure arithmetic from `mivaltree.c:362-380`, cross-checked against the Xorg damage `80,80 58x58` extents:
    - `border_exposed_of_a_child_crossing_the_grown_edge`: child (80,80) 50×50 bw 4, W 100→200. In child content
      space the result is `[(16,-4,38,4), (50,0,4,16), (-4,16,4,34), (50,16,4,34), (-4,50,58,4)]`. Mutation: return
      the whole ring.
    - `border_exposed_when_an_occluding_sibling_unmaps` (U): S (0,0) 30×30 over C (10,10) bw 4. The result is
      `[(-4,-4,20,4), (-4,0,4,16)]`.
    - `unbordered_windows_expose_no_border` (identity).
  - Core, `expose_probe.rs`: the C resize with a Damage object on W. Its **coverage** equals Xorg's (border union
    `80,80 58x58` region, then content). Expose events are unchanged.
  - Acceptance: C (both map orders), CF and U: ring `ff0000` at the measured points. Mutation: skip
    `paint_window_border_region`.
- [ ] **Step 2:** run them; C and CF fail with `000000` and `ffffff`, U with `00ff00`.
- [ ] **Step 3:** implement.
- [ ] **Step 4:** run the gate. Also run the vng `expose-order` and `damage-after-map` goldens (unbordered, so they
  must not move) and the border-damage A/B (Review Focus 5).
- [ ] **Step 5:** commit `fix(expose): paint the newly exposed part of a window's border (PW_BORDER)`.

### Task 4: M: ring paints into a shared backing respect higher siblings and the parent's content

**Files:**
- Modify: backend `portable/windows.rs:1030-1150` (`paint_window_border`) and the new `…_rects` from Task 3. When
  `target.backing_id()` is not the window's own leaf or own backing (the window paints into an ancestor's backing):
  - clip the ring to the **parent's and ancestors'** content bounds, since a child's border may not reach its
    parent's border (`mivaltree.c:386`). Not `target.content_bounds()`: that includes the child's own content
    bounds (`paint_target.rs:485`), and the ring lies outside them, so it would clip away the ring itself;
  - then subtract `copy_area_shared_backing_occluders(host_xid, &target)` (`paint_target.rs:906`), which is mapped
    higher siblings at every level.

  This is Xorg's `borderClip` (`miComputeClips`). Leaf targets are unchanged.

- [ ] **Step 1: tests.**
  - Acceptance `a_ring_painted_under_a_higher_sibling_leaves_the_sibling` (M): (11,20) and (20,11) are `00ff00`.
    Mutation: drop the occluder subtract.
  - `an_unobscured_part_of_the_ring_stays_painted` (same M layout): a point on C's ring that S does not cover is
    `ff0000`. Mutation: clip the ring to `target.content_bounds()`.
  - `a_childs_ring_stops_at_its_bordered_parents_content`: W bw 3, child crossing W's right edge; W's ring column
    keeps W's border pixel. Expected value from a Xorg probe run, added to the Task 0 probe as case **MB**. This
    divergence is **not measured yet**; it is inferred from the code, so measure it first and drop the test if
    Xorg agrees with yserver.
- [ ] **Step 2:** run; M fails (`ff0000`).
- [ ] **Step 3:** implement.
- [ ] **Step 4:** run the gate.
- [ ] **Step 5:** commit `fix(composite): a border painted into a shared backing stays under higher siblings`.

### Task 5: N, suite entry, status

**Files:**
- Modify: backend `portable/redirect.rs:860-870`. Call `overlay_backing_inferiors` only when W has no redirected
  ancestor. Under one, `seed_backing_from_parent` already reads the authoritative ancestor backing, which is
  `compNewPixmap`'s IncludeInferiors copy.
- Modify: `tools/vng-scenarios/suite.list` (uncomment the Task 0 entry; add it to the `QUICK` filter in
  `.github/workflows/vng-suite.yml` if its measured runtime is short) and `docs/status.md` (one entry: what was
  measured, the four fixes, and the remaining items below).

- [ ] **Step 1:** acceptance `nested_redirect_seeds_from_the_ancestor_backing` (N): (3,3) and (10,10) `00ff00`,
  (30,30) `0000ff`. Mutation: always overlay.
- [ ] **Step 2:** red, implement, gate. The whole vng suite is green, `redirected-backing` included (`golden=live`).
- [ ] **Step 3:** commit `fix(composite): a nested redirect seeds from its ancestor's backing, not stale leaves`, then
  `test(vng): redirected-backing joins the suite`.

### Task 6: Hardware smoke (jos observes, once, on the whole branch)

The user watches. Send the per-step observation guide first.

| Box / desktop | Exercise | Watch for |
|---|---|---|
| bee, awesome + picom (glx, then xrender) | bordered frames; resize tiles; titlebar redraws | frame borders intact; titlebar text not shifted by `bw` (D); no black rings on resize (C) |
| bee, xcompmgr + fvwm | window-shade (South gravity, `windows.rs:1895`); resize | the shaded client reveals with pixels (G) |
| bee, MATE + marco compositing | panel tray (XEmbed, nested redirect, N); drag panel; resize caja | tray icons survive the panel grow; no stale strips |
| bee, Plasma/KWin | resize; mpv fullscreen + `_NET_WM_BYPASS_COMPOSITOR` (unredirect restore); direct scanout | windows do not blank on bypass; direct scanout still engages (`m2` logs) |
| silence, Cinnamon/muffin | restacks; resize | no regression of the unflip/restack symptoms |
| any, mpv Vulkan + picom | Present into a redirected window; resize while playing | no stalls; no `fallback_target` mismatches |

Logs: no new `warn`, no `copy_area dropped`, no `SHORT-OF-LEAF`. Per-resize GPU submits go **down**, because the
restore and overlay passes are gone; quote the `record_paint_submit` delta.

## Risks and scope

- **Direct scanout / Present.**
  - Task 1 changes only the xid that core draw ops name. The backend resolves both to the same backing id, so
    `direct_frame_references_host_drawable` (`kms/scanout.rs:329`), which compares the backing id and the paint xid,
    is unaffected.
  - Task 2 changes which backing is current mid-configure. The existing `request_direct_unflip` in
    `configure_subwindow` (`windows.rs:2034`) still runs first for W.
- **The DRI3 imports themselves.** Untouched: they are pixmap-only.
- **COW.** The COW is never redirected and has `bw = 0`, so it is identity in every task. `composite_overlay.rs:161`
  gains the border tuple and an empty border region.
- **Unredirect restore.** `release_redirected_backing` is unchanged; only the rotate stops using it. The restore
  reads the **current** backing, so after Task 2 it sees NEW, which is complete. Known remaining gap, unmeasured and
  out of scope: on a nested unredirect, Xorg's `compRestoreWindow` writes the **parent's** pixmap, while yserver
  writes leaves that are stale under the ancestor.
- **Damage granularity.** Measured, out of scope: Xorg reports `extents ∩ clip` per fill and yserver reports the
  region. The vng gate compares coverage.
- **ForgetGravity.** Existing divergence, kept: yserver still copies W's own old content NW (`redirect.rs:865-905`),
  while Xorg copies nothing for W itself under ForgetGravity and exposes it whole. Both repaint backgrounds; they
  differ only for `background None` (the "Resize WIPES background-None windows" note).
- **UnmapGravity** children are not unmapped on a parent resize: `win_gravity_delta` returns `(0,0)` for gravity 0,
  `resources/window.rs:51`, against Xorg `miwindow.c:391-399`. Seen in code, not measured, out of scope.
- **Order of commits:** 0 → 1 → 2 → 3 → 4 → 5.
  - D is independent and the highest user impact (awesome + picom).
  - G before C, so that C's tests see the parent seed and not stale black. Either alone leaves the rows of the other
    red.
  - M and N are small, and each can be dropped without touching the rest.

## Decisions (jos, after the codex review)

1. **N and M** are fixed on this branch.
2. **Border damage on a resized redirected W:** keep ONE report. Xorg reports it twice; document that measured
   difference instead of matching it.
3. **Commit cadence:** automated gate per task, commit after each gate; one HW smoke at the end.
4. **xts:** yes to the `Xlib9` IncludeInferiors cases, if xts runs here; otherwise skip, jos runs it on bee.
5. **Raw-rect damage granularity:** out of scope.
6. **The vng scenario** joins the quick tier if its measured runtime is short.
