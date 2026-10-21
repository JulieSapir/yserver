# Cursor Sprite State P2a Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL at implementation time: use superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking. The implementer is a small model: where this plan gives an interface, a table or a mutation, follow it exactly; where it names a reference file and line range, read that range first and port it, stripping what is listed.

**Goal:** Land the foundations of the atomic cursor path with no change to what the server does: small shared cursor test fixtures, the cursor buffer operations, the sprite pool, the capability negotiation and driver facts, the topology/eligibility discovery that decides the static cursor state path, and the scene-crate seam with Legacy implementations.

**Architecture:** Everything here is additive and unread by production behavior. The sprite pool and buffer operations are new modules with recording test doubles; `Device::open` delegates to a testable post-open function but its cursor step (hotspot capability, driver name) is not added until P2b Task 5, so production devices have no negotiated caps and decide `Legacy/no-device-caps`; the path is decided once at plane construction, logged, and nothing branches on `Atomic` until P2b; the scene seam gets Legacy implementations only. Master's legacy cursor path, atomic position path, software fallback and every existing test are untouched.

**Tech Stack:** Rust workspace; locked `drm` 0.15.0 / `drm-ffi` 0.9.1 (dumb buffers, `add_planar_framebuffer`, client capabilities); P1's `AtomicEventFixture` and recording atomic executor; lavapipe for ignored scene tests (only P2b needs Vulkan).

**Spec:** [docs/superpowers/specs/2026-10-19-cursor-sprite-state-design.md](../specs/2026-10-19-cursor-sprite-state-design.md). Read it before executing any task. Continuation: [P2b](2026-10-19-cursor-sprite-state-b.md).

## Global Constraints

- Everything on the legacy path is unchanged when the device does not qualify; master's `set_cursor2`/`move_cursor` executors, hide-recovery, transient-fallback and software-cursor policy keep their tests.
- Devices with no universal cursor plane (Apple DCP / Asahi) keep the optimistic legacy probe and the permanent software-cursor latch.
- Preserve Linux glibc, Linux musl and FreeBSD ioctl request typing; the hotspot capability uses the `as _` request cast pattern, never `libc::Ioctl`.
- No new `YSERVER_*` behavior environment variables and no `cfg(test)` branches inside production function bodies (whole test-only items, methods and fields are fine when nothing in production reads them).
- Respect `tools/check-layout.sh` (5,000 code / 6,000 test lines; `yserver-render` free of drm/gbm/input/udev/`yserver-core`; `backend/portable` free of KMS names). Do not edit the script.
- Do not pull forward any P3-P5 behavior and none of the four maintainer findings (spec Non-goals).
- P2a changes `yserver-render` only in Task 6 (the scene seam, with Legacy behavior identical to master).
- The path is decided once per `CursorPlane` from static facts and is never recomputed by a refresh; the assignment and `properties_by_crtc` are immutable for the plane's lifetime (spec R3, R24).

This document is planning only. Its preparation authorizes writes only to `docs/superpowers/specs/` and `docs/superpowers/plans/` of `/home/ariel_santangelo/Projects/yserver-cursor-sprites`, with no git writes, cargo runs, device access, hardware or subagents. Tasks below describe future implementation; their source writes and commits require a later implementation instruction. Do not merge as part of this plan; the repository's confirmation rule applies to any later merge.

Branch `feat/cursor-sprite-state` on P1 `ba875e8b`. Read-only porting source: `/home/ariel_santangelo/Projects/yserver-cursor` at `9023da14` (call it *the reference*). Reference line numbers below refer to that HEAD. Never copy whole reference files: each task lists what to port and what to strip.

## Review Focus

1. A paravirtualized or unknown-name driver must never be treated as physical, and a physical driver must never be demoted: Task 4 `paravirtualized_names_are_exact_and_unknown_needs_hotspot` and Task 5 decision-table row 4.
2. A buffer with a user or reservation must never be rewritten or freed, including at pool drop: Task 3 `sprite_pool_never_rewrites_a_buffer_with_users_or_reservations` and `sprite_pool_drop_forgets_buffers_with_live_users`.
3. A smaller second cursor on a padded pitch must not show pixels of the first: Task 3 `sprite_pool_writes_top_left_image_and_clears_pitch_padding`.
4. A half-failed allocation must leak nothing and keep the wanted image retryable: Task 3 `sprite_pool_allocation_failure_rolls_back_in_reverse_order`; and the same version re-staged on another output must reuse its slot: Task 3 `sprite_pool_restaging_a_held_identity_reuses_its_slot`.
5. Deciding the path must not change behavior of any device, and a topology refresh must never flip it: Task 5 `installing_a_complete_plane_computes_atomic_but_changes_no_behavior`, `topology_refresh_never_changes_the_path_or_the_assignment`, and the unchanged master suite.

## Verification rules for all tasks

- **A (end state):** every stateful test ends with the relevant ledger empty: sprite pool users and reservations zero, `CursorBufferLedger::end_state() == (0, 0, 0)` after the pool is released, no master cursor in-flight record, no pending position. Do not clear maps to obtain this; settle through the API under test.
- **B (production drivers):** Tasks 2-5 test pure boundary algorithms (buffer operations, pool, negotiation order, decision table) and Task 6 the scene crate's pure functions directly; they say so and do not claim Backend coverage. Their Backend-driven re-proofs are in P2b. Task 1 and Task 5's backend-level tests advance only through `Backend` trait entries.
- **C (hardware):** none in P2a. The cap request and path log are exercised on hardware in P2b Task 6.
- **F (repetition):** every new test runs 5 times under exactly 16 busy loops (none here waits on GPU work). Start one Bash session, save the 16 busy-loop PIDs in an array, `trap` EXIT/INT/TERM to kill and reap only those PIDs, run `cargo test --workspace --locked --features xdmcp <name> -- --exact --nocapture` five times with the lavapipe environment sourced, and kill the loops before reporting. Use `--list` first so a filter cannot select zero tests. Each mutation named below is applied once, must fail the named assertion (not a build error or timeout), and is reverted before the next case and before the gate.
- **Skips:** every fixture skip prints exactly one line naming its reason, from the deciding constructor only. Unexpected setup errors fail.
- **Mutation protocol:** mutate by line, confirm the run compiled (a compile error is not a failure to count), then restore. Record test name, repetitions, skips and result in the task report.
- One commit per task after its gate. Keep every commit buildable; add or update consumers and tests in the same task as an interface change. Any `#[allow(dead_code, reason = "...")]` names the P2b task that consumes the item; P2b removes it.

Per-task gate (Step "gate" of every task). Every test command is prefixed by `source /home/ariel_santangelo/Projects/claude-scratch/atomic-cursor-split/sdd-p1/lvp-env.sh &&` (without it tests would use the real NVIDIA/AMD drivers), and no test is ever run without it:

```sh
cargo +nightly fmt --all
source /home/ariel_santangelo/Projects/claude-scratch/atomic-cursor-split/sdd-p1/lvp-env.sh && cargo clippy --workspace --all-targets --locked --features xdmcp -- -D warnings
source /home/ariel_santangelo/Projects/claude-scratch/atomic-cursor-split/sdd-p1/lvp-env.sh && cargo test --workspace --locked --features xdmcp
source /home/ariel_santangelo/Projects/claude-scratch/atomic-cursor-split/sdd-p1/lvp-env.sh && cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
tools/check-layout.sh
```

## Production caller audit (performed before choosing the tests)

Read-only `rg -n -g '*.rs' -g '!**/tests/**' -g '!**/for_tests.rs'` on the P1 tree. Re-run if the base moves. A reference-only caller is not a reason to port a path.

| Path a test relies on | Non-test caller verified | Driver used by planned tests |
| --- | --- | --- |
| `CursorPlane::new` (discovery, dimensions) | `platform/cursor.rs:289` `initialize_cursor_plane_for_device`, passed as the factory at `platform/connectors.rs:432,612` and `platform/init.rs` | Task 5 decision-table tests call the pure decision directly; the install test drives `install_cursor_plane_for_device`, the function production calls |
| Scene `upload`/`show` sink calls | `scene/cursor.rs:544` `apply_cursor_transition_on_retire` (from `scene/flip.rs:218`, `scene/cursor.rs:112`), direct hooks `backend/kms/scanout.rs:431-470` | Task 6 render-crate pure tests; Backend re-proof in P2b |
| `install_cursor_plane_for_device` | `platform/cursor.rs:289` (above) | Task 5 backend-level test via a test installer that calls this same function |
| Cursor topology refresh | `platform/connectors.rs:286,942,1160` call `refresh_cursor_topology_for_devices(_with)` (`platform/cursor.rs:1374`) | Task 5 refresh test beside master's existing refresh tests in `platform/tests.rs` (they already call it directly) |
| `Device::open` post-open sequence | `kms/outputs.rs:308`, `platform/drm.rs:181` (the candidate probe; it also negotiates, which is harmless) call `Device::open`, which calls `configure_kms_device` | Task 4 tests call `configure_kms_device` through a recording `DeviceControl`; the cursor step is added in P2b Task 5 |
| Dumb buffer / `AddFB2` calls | None on master's atomic path (the legacy path passes a dumb handle to `set_cursor2` and never calls `AddFB2` for the cursor) | Task 2 tests use the recording buffer I/O only; the real I/O is exercised on hardware in P2b Task 6 |
| Master cursor-only lane and event retirement | `backend/kms/scanout.rs:2522,2631` call `submit_atomic_cursor_move_for_output`; `:2201,2229,2290` retire | Task 1 and Task 5 backend tests, through `Backend::warp_pointer_root`, `maybe_composite`, `on_page_flip_ready` |

## File and interface map

`Y` means `crates/yserver/src/`. No `yserver-render` file changes in P2a.

| Responsibility | Files |
| --- | --- |
| Small cursor test fixtures | `Y/kms/render/backend/tests/cursor_sprites/{mod,support}.rs`; register in `Y/kms/render/backend/tests/mod.rs` |
| Dumb buffer / AddFB2 operations, ledger, recording I/O | `Y/kms/cursor_plane/ops.rs`, `Y/kms/cursor_plane/ops/recording.rs` (test only) |
| Sprite pool | `Y/kms/cursor_plane/sprites.rs`, `Y/kms/cursor_plane/sprites/tests.rs` |
| Capability negotiation, driver facts, open sequence | `Y/drm/client_caps.rs`, `Y/drm/device.rs`, `Y/drm/mod.rs` |
| Property discovery, static path decision, plane integration | `Y/kms/cursor_plane/topology.rs`, `Y/kms/cursor_plane.rs`, `Y/kms/render/platform/cursor.rs`, `Y/kms/render/platform.rs`, `Y/kms/render/platform/tests.rs` |
| Scene seam (Legacy behavior) | `R/scene.rs`, `R/scene/{sink,cursor,flip}.rs`, `R/scene/tests/{mod,cursor}.rs`, `Y/surface/platform.rs`, `Y/kms/render/platform/{scene_sink,cursor}.rs`, `Y/kms/render/backend/kms/scanout.rs` |

### Task 1: Small shared cursor fixtures

**Files:**

- Create: `crates/yserver/src/kms/render/backend/tests/cursor_sprites/mod.rs`.
- Create/Test: `crates/yserver/src/kms/render/backend/tests/cursor_sprites/support.rs`.
- Modify: `crates/yserver/src/kms/render/backend/tests/mod.rs` (add `mod cursor_sprites;` next to `mod commit_identity;`).

**Interfaces:**

- Consumes: P1's `commit_identity::support::{AtomicEventFixture, RecordingSceneFixture, accepted_event_id}` (`pub(crate)` items in `backend/tests/commit_identity/support.rs:25,265`; make the `commit_identity` module `pub(super)` visible to siblings if it is not, with the smallest visibility change), `RecordingSceneFixture::{new, install_direct_present}` for the grouped-cookie case, master's `Backend::{create_pixmap, put_image, create_cursor, render_create_picture, render_create_cursor, define_cursor, warp_pointer_root, maybe_composite, on_page_flip_ready, before_block}`, `RecordingAtomicCommitExecutor::records()`, `AtomicCommitRecord { kind, flags, properties, events, user_data, device_fd, result }`.
- Produces (all `pub(crate)` in `support.rs`, nothing else):
  - `create_argb_cursor(backend: &mut KmsBackend, extent: (u16, u16), pixel: [u8; 4], hotspot: (u16, u16)) -> CursorHandle`: ARGB cursor of `extent` filled with `pixel` through pixmap, `put_image`, `render_create_picture`, `render_create_cursor`. Port the helper at reference `backend/tests/atomic_cursor_only/round_b_producers.rs:4-31`, generalizing the hard-coded 4x4 to `extent`.
  - `create_bitmap_cursor(backend: &mut KmsBackend, visible: bool) -> CursorHandle`: 1x1 bitmap cursor through `create_pixmap`, `put_image`, `create_cursor`. `visible == true` paints the source bit (as master's `install_visible_cursor_via_backend`, `backend/tests/atomic_cursor.rs:251-271`); `false` leaves source and mask transparent (reference `round_b_producers.rs:124-` `transparent_bitmap_cursor`).
  - `define_root_cursor(backend: &mut KmsBackend, cursor: CursorHandle)`: `Backend::define_cursor` on `backend.core.window_id`.
  - `accepted_live_records(recorder: &RecordingAtomicCommitExecutor) -> Vec<AtomicCommitRecord>`: successful, non-`TEST_ONLY` records only (the inventory's name, kept so the P3-P5 plans need no rename).
  - `record_property(record: &AtomicCommitRecord, property: property::Handle) -> Option<u64>`: the value written to `property` on any object of the record.
  - `CompletionKey { device_fd: RawFd, crtc: crtc::Handle, commit_id: u64 }` (`Hash + Eq + Copy`) and `retire_pending_output(events: &mut AtomicEventFixture, delivered: &mut HashSet<CompletionKey>, output_idx: usize, frame: u32) -> u64`: finds accepted event-producing records (`accepted_event_id`) for that output's device fd whose target event is that output's CRTC and whose `CompletionKey` is not in `delivered`; asserts there is exactly one (the one-commit-in-flight invariant); calls `events.queue_commit_completion(device_fd, crtc, id, frame, Duration)` with that exact ID and dispatches `Backend::on_page_flip_ready(&mut events.backend, &mut events.state, device_fd)`; inserts `(device_fd, crtc, id)` in `delivered`; returns the ID. A grouped record can therefore be delivered once per target CRTC, and another device's same numeric ID remains independent. Panics with the message `no accepted unretired commit for output` when none exists and `more than one accepted unretired commit for output` when several do.
  - `assert_no_cursor_resources(backend: &KmsBackend)`: asserts, for every device, `!backend.platform.any_cursor_commit_in_flight()`, no pending position (`for_tests_atomic_cursor_pending_position` is `None` for every output), no `cursor_retry_waits_for_flip` (use `PlatformBackend::cursor_retry_waits_for_flip(output_idx)`). Failure messages start with `cursor commit still in flight`, `cursor position still pending`, `cursor retry still waiting`. Tasks 2 and 4 and P2b add the sprite-pool and ledger assertions to this same function as their types appear.
  - `finish_fixture(events: &mut AtomicEventFixture, delivered: &mut HashSet<CompletionKey>)`: for every output, retires remaining accepted unretired commits one at a time with `retire_pending_output` until none remain (bounded to 8 iterations per output, then panics with `fixture did not settle`), calls `Backend::before_block` 4 times, then `events.assert_quiescent()` and `assert_no_cursor_resources(&events.backend)`. It never clears state to pass.
- Produces: `cursor_sprites::mod.rs` declares `mod support;` and re-exports nothing else; later tasks add `mod pool;`, `mod topology;`, etc. as they add files.

- [ ] **Step 1: Write the fixture contract tests first.** The basic helper and equal-device-ID cases are headless; the grouped two-CRTC completion case uses the existing ignored GPU scene fixture.

| Test | Drives and asserts | Mutation that must fail it |
| --- | --- | --- |
| `p2_fixture_cursor_helpers_control_the_effective_cursor` | `KmsBackend::for_tests()`; define a visible bitmap cursor on the root: `!effective_cursor_hidden()`; define an invisible one: `effective_cursor_hidden()`; `create_argb_cursor` with extent (6, 3), hotspot (1, 2): the stored cursor record has `width == 6`, `height == 3`, `hot_x == 1`, `hot_y == 2`. | `define_root_cursor` defines on a child window instead of the root; or `create_argb_cursor` ignores `extent` and builds 4x4. |
| `p2_fixture_retire_pending_output_delivers_exactly_the_accepted_id` | `AtomicEventFixture::new(1)`; seed one visible atomic position with `for_tests_seed_atomic_cursor_position` (a test seed is allowed as setup; it is what master's `atomic_cursor.rs` tests use); `Backend::warp_pointer_root` then `Backend::maybe_composite` produce one accepted `CursorOnly` record; `retire_pending_output` returns that record's `accepted_event_id`, after which `!any_cursor_commit_in_flight()`, `for_tests_pending_page_flip_entry_count() == 0`, and a second call panics with `no accepted unretired commit for output` (use `std::panic::catch_unwind`). | Deliver `id + 1` instead of the accepted ID (P1 filters it as unmatched, the pending entry remains, the in-flight check fails). |
| `p2_fixture_retire_pending_output_qualifies_equal_ids_by_device` | `AtomicEventFixture::new(2)` creates two devices with the same CRTC handle; seed a visible atomic position on each output, call `Backend::warp_pointer_root` and `Backend::maybe_composite`, and assert each device accepted a `CursorOnly` record with ID 1. Call `retire_pending_output` for output 0, then output 1; both return 1, each output has no cursor commit in flight, and `delivered` contains both `(fd, crtc, 1)` keys. | Track `id` alone: the second call finds no accepted unretired commit for output, failing the second-output assertion. |
| `p2_fixture_retire_pending_output_accepts_a_group_cookie_per_crtc` (`#[ignore]`, GPU scene) | `RecordingSceneFixture::new(2)`; `install_direct_present`; call `Backend::try_present_direct` eight times (first seven reject during existing probation, eighth accepts); identify the accepted grouped record with two target CRTCs and one `accepted_event_id`. Call `retire_pending_output` for output 0 then output 1 without changing the cookie; both return the same ID, both CRTC event queues and in-flight records settle, and `delivered` contains `(fd, crtc0, id)` and `(fd, crtc1, id)`. | Track `(fd, id)` only: the second CRTC is suppressed and its assertion reports an outstanding grouped commit. |
| `p2_fixture_accepted_live_records_exclude_test_only_and_failed_commits` | Script the recorder (`script_result`) so one cursor-only attempt fails with `EINVAL`, and inject one `TEST_ONLY` record through `AtomicCommitSink::test_only` (`drm/atomic.rs:744`); `accepted_live_records` returns only the successful non-`TEST_ONLY` record; `record_property` returns `Some(x)` for `CRTC_X` and `None` for an absent property. | `accepted_live_records` stops filtering `TEST_ONLY`, or `record_property` matches by object instead of property. |
| `p2_fixture_finish_rejects_an_unretired_cursor_commit` (`#[should_panic(expected = "cursor commit still in flight")]`) | As `p2_fixture_retire_pending_output_delivers_exactly_the_accepted_id`, but call `assert_no_cursor_resources` while the accepted `CursorOnly` commit is unretired. | `assert_no_cursor_resources` skips the in-flight check. |

- [ ] **Step 2: Run the new tests red**, then implement the helpers (one file, `support.rs`). Red must show the missing helper or failed assertion, not a setup error. Add whole-function `#[cfg(test)]` is unnecessary: the whole `tests` tree is already test-only.
- [ ] **Step 3: Run the mutation and repetition protocol.** The headless tests are 5x under 16 busy loops; `p2_fixture_retire_pending_output_accepts_a_group_cookie_per_crtc` is GPU-waiting, ignored and repeated 20x when Vulkan is available.
- [ ] **Step 4: Run the per-task gate; commit only after every command passes.** Subject: `test(kms): share small cursor fixtures`.

### Task 2: Cursor buffer operations, ledger and recording I/O

**Files:**

- Create: `crates/yserver/src/kms/cursor_plane/ops.rs`.
- Create/Test: `crates/yserver/src/kms/cursor_plane/ops/recording.rs` (declared `#[cfg(test)] pub(crate) mod recording;` at the bottom of `ops.rs`).
- Modify: `crates/yserver/src/kms/cursor_plane.rs` (add `pub(crate) mod ops;` only).

**Interfaces:**

- Produces in `ops.rs` (port reference `kms/cursor_plane/ops.rs`): `CursorFramebufferDescriptor` (private constructor `argb8888(width, height)`), `CursorDumbBuffer`, `CursorMapping`, `trait CursorBufferIo` with exactly the reference methods `create_dumb`, `map_dumb`, `add_framebuffer`, `write_image`, `remove_framebuffer`, `unmap_dumb`, `destroy_dumb` (lines 87-112), `DrmCursorBufferIo` (133-255, using `device.create_dumb_buffer`, `map_dumb_buffer`, `add_planar_framebuffer`, `destroy_framebuffer`, `destroy_dumb_buffer`; keep `ensure_device`), `CursorBuffer { allocate, release, fb_id, ptr, write_image, clear, dump_to_ppm, for_tests_bytes }` (298-310, 686-927). `CursorBuffer::allocate` is the shared production algorithm: acquire dumb handle, mapping, framebuffer in that order; roll back only what was acquired, in reverse order, on failure; return the original error plus any cleanup error text; start fully cleared. `write_image` validates `0 < image <= capacity` and `len == w*h*4` (both `InvalidInput`), zeroes the whole `pitch * capacity_height` mapping, then copies rows top-left (reference `write_mapped_image`, 852-907). `Drop for CursorBuffer` releases through `release_resources`, warning on cleanup error.
- Produces in `ops/recording.rs` (port reference 312-684, moved out of the production file): `CursorBufferRecord::{Create, Map, AddFb, RemoveFb, Unmap, Destroy}` (each with `device_fd`), `CursorBufferLedger` (`records()`, `end_state() -> (live_dumb, live_mappings, live_framebuffers)`, `fail_after(operation, nth, raw_os_error)`, `failures_taken()`, `mapped_bytes(device_fd, handle)`), `CursorBufferOperation`, `RecordingCursorBufferIo::{new(pitch_alignment) -> (Self, Rc<CursorBufferLedger>), with_shared_ledger(ledger, pitch_alignment)}` over in-memory `Box<[u8]>` mappings with pitch rounded up to `pitch_alignment`. Strip the reference's `#[allow(dead_code, reason = "consumed by the A1 atomic cursor backend fixture")]` attributes.
- Dead code: `#[allow(dead_code, reason = "consumed by P2a Task 3 sprite pool")]` only on items clippy flags.

- [ ] **Step 1: Write the buffer tests first** (stateless boundary tests with `Device::for_tests()` and `RecordingCursorBufferIo::new(256)`; port the reference's own tests, lines 929-1005, e.g. `cursor_allocation_preserves_map_and_destroy_errors`, and add):

| Test | Drives and asserts | Mutation that must fail it |
| --- | --- | --- |
| `cursor_buffer_allocation_order_and_full_geometry` | `CursorBuffer::allocate(70x64)`: ledger records are exactly `Create(70x64), Map, AddFb(ARGB8888 fourcc, 70x64)`; `end_state() == (1, 1, 1)`; `release()` appends `RemoveFb, Unmap, Destroy` and `end_state() == (0, 0, 0)`. | Allocate smaller geometry; reorder map and AddFb. |
| `cursor_buffer_failure_rolls_back_only_what_was_acquired_in_reverse_order` | `fail_after` create / map / addfb in turn: the call returns the injected errno in its text, `end_state() == (0, 0, 0)`, rollback records are in reverse acquisition order (no `Unmap` after a map failure, `Unmap` then `Destroy` after an AddFb failure); a failing cleanup keeps both texts. | Omit `unmap` after AddFb failure; omit `destroy` after a map failure. |
| `cursor_buffer_write_places_top_left_and_clears_padding` | pitch 512 for a 70-pixel row: a 4x4 image then a 2x2 image of another color on the same buffer: bytes only at rows `< h`, columns `< w` at stride 512, every other byte zero, no pixel of the first image after the second write. | Skip the full-mapping clear; copy rows with stride `w*4`. |
| `cursor_buffer_rejects_oversize_and_wrong_length` | 71x64, 70x65, zero-width, and a byte slice one byte short each return `InvalidInput` with the mapping untouched. | Remove the capacity check; check `>= len`. |

- [ ] **Step 2: Run red, port `ops.rs` and the recording module, run green.**
- [ ] **Step 3: Layout.** `ops.rs` stays far below 5,000 lines with the recording module split out (reference total 1,005). Verify `add_planar_framebuffer` and `destroy_framebuffer` exist in the installed `drm` 0.15.0 source before use.
- [ ] **Step 4: Mutation and repetition protocol** (5x under 16 busy loops).
- [ ] **Step 5: Run the per-task gate; commit only after every command passes.** Subject: `feat(kms): add cursor sprite buffer operations and recording ledger`.

### Task 3: Sprite pool

**Files:**

- Create: `crates/yserver/src/kms/cursor_plane/sprites.rs`.
- Create/Test: `crates/yserver/src/kms/cursor_plane/sprites/tests.rs` (`#[cfg(test)] mod tests;` in `sprites.rs`).
- Modify: `crates/yserver/src/kms/cursor_plane.rs` (add `pub(crate) mod sprites;` only).

**Interfaces:**

- Produces (port reference `kms/cursor_plane/sprites.rs`, strip as listed):
  - `SpriteBufferLease` (`Copy`): `version() -> u64`, `matches(version, hot_x, hot_y) -> bool`. Omit the reference's `sprite_key()` (P5 qualification).
  - `enum SpriteStage { Ready, Deferred }` (the scene crate's `CursorUploadOutcome` arrives in Task 6; P2b maps one to the other).
  - `SpritePool::new(width: u32, height: u32) -> Self`; `set_active_crtc_count(&mut self, count: usize)` (maximum `max(2, count + 1)`; it only bounds growth and never shrinks or releases buffers already held, so a smaller count leaves the pool as is); `prepare(&mut self, device: Rc<Device>, io: Rc<dyn CursorBufferIo>, version: u64, hot_x: u16, hot_y: u16, image_width: u32, image_height: u32, bgra_bytes: &[u8]) -> io::Result<SpriteStage>`; `candidate_lease(&self) -> Option<SpriteBufferLease>`; `framebuffer_for_lease(&self, lease) -> Option<u32>`; `reserve`, `can_accept_reserved`, `accept_reserved`, `reject_reserved`, `release_user`, `cancel_candidate` (all `-> bool`); `dump_sprite_to_ppm`; `has_available_buffer`; `wanted_version`. Test-only (`#[cfg(test)]`): `user_count(lease)`, `all_users_zero()`, `prepared_user_count()`, `for_tests_sprite_bytes(lease)`, `candidate_version()`.
  - **Strip:** `quarantined`, `disarm`, `recover_after_all_off` (P5). **Replace** the reference `Drop`: forget (`std::mem::forget`) exactly the buffers with `users != 0 || prepared_users != 0`; release the rest in order through `CursorBuffer::release` (spec R15).
  - Invariants: `prepare` records the newest wanted image before anything can fail; the first call allocates both initial buffers atomically (second fails -> first released, error returned; `WouldBlock` allocation or write -> `Ok(Deferred)`); **identity lookup covers every slot** whose recorded `(version, hotspot)` equals the request (not only the candidate): such a slot becomes the candidate with no write and no allocation; otherwise a different identity writes the first slot with `users == 0 && prepared_users == 0`, growing up to the maximum, else `Deferred`; a successful write sets that slot's version/hotspot, publishes the candidate and never touches a slot with users or reservations; `lease_matches` compares the slot's recorded version and hotspot so a stale lease cannot act on a rewritten slot.
- Dead code: `#[allow(dead_code, reason = "consumed by P2b Task 2 sprite staging")]` only where clippy flags.

- [ ] **Step 1: Write the pool tests before the pool** (pure boundary tests of the allocation and accounting algorithm; `Device::for_tests()`, `RecordingCursorBufferIo::new(256)`, capacity 70x64):

| Test | Drives and asserts | Mutation that must fail it |
| --- | --- | --- |
| `sprite_pool_allocates_two_full_capacity_argb_buffers` | First `prepare` of a 4x4 image: ledger shows exactly two `Create` (70x64), two `Map`, two `AddFb` (ARGB8888); `end_state() == (2, 2, 2)`; dropping the pool with no users gives `(0, 0, 0)`. | Allocate sprite-sized buffers; skip `AddFb`. |
| `sprite_pool_writes_top_left_image_and_clears_pitch_padding` | Stage a 4x4 opaque image, then a 2x2 of another color into the other free slot, then (after `cancel_candidate`) a 3x3 into the first: bytes only inside the image at stride 512; the smaller later image leaves no pixel of an earlier one. | Skip the clear; use stride `w*4`. |
| `sprite_pool_allocation_failure_rolls_back_in_reverse_order` | `fail_after` create/map/addfb for buffer 1 and for buffer 2: `end_state() == (0, 0, 0)` after each, rollback in reverse order, error text carries the errno; `wanted_version() == Some(v)` and a later `prepare` succeeds. A `WouldBlock` write returns `Ok(Deferred)` and keeps the previous candidate. | Publish a candidate before the write succeeds; leak the first buffer when the second fails. |
| `sprite_pool_hotspot_only_change_stages_a_distinct_buffer` | Same version and pixels with hotspot (1,1) then (2,2): two leases, different framebuffers, `matches(v,1,1)` false for the second; staging (2,2) again returns `Ready`, same candidate, no new ledger record. | Dedup by version only. |
| `sprite_pool_restaging_a_held_identity_reuses_its_slot` | Output A stages version 5; output B stages version 6 (candidate = 6); a late staging of version 5 finds the slot that still holds 5 (it has a user or not), makes it the candidate, and writes nothing (ledger unchanged, no `Deferred` even when the pool is otherwise full). | Compare only against the candidate. |
| `sprite_pool_pressure_defers_and_keeps_only_the_newest_wanted_image` | `set_active_crtc_count(1)` (max 2): A and B staged, reserved and accepted (users). C then D: both `Deferred`; `wanted_version()` is D's, never C's; no write; A and B bytes unchanged. `release_user(A)` then `prepare(D)` -> `Ready` in A's slot. With count 3 it grows to 4, not beyond; then `set_active_crtc_count(1)` leaves the 4 held buffers untouched. | Overwrite a slot with users; keep the oldest wanted; grow past `count + 1`; release buffers on shrink. |
| `sprite_pool_never_rewrites_a_buffer_with_users_or_reservations` | One lease reserved+accepted by two CRTCs: `user_count == 2`; a `prepare` needing a slot cannot take it; one `release_user` leaves 1, the second frees it; a third returns `false` and does not underflow; `reserve` of a stale lease (slot rewritten) returns `false`. | Release on accept; underflow; skip the lease/slot check. |
| `sprite_pool_reservation_rejection_restores_the_prior_owner` | A applied (user 1), B staged: `reserve(B)` -> `prepared_user_count() == 1`; `reject_reserved(B)` -> 0, B free, A's user still 1; afterwards `can_accept_reserved(B)` and `accept_reserved(B)` are false. | `reject_reserved` also decrements users; accept without a reservation. |
| `sprite_pool_drop_forgets_buffers_with_live_users` | A (user) and B (free): dropping releases B only (`RemoveFb`/`Unmap`/`Destroy` for B; A's records end at `AddFb`; `end_state() == (1, 1, 1)`). A pool with only a reservation also forgets that buffer. | Release every buffer on drop. |
| `sprite_pool_rejects_oversize_and_wrong_length_images` | 71x64, 70x65, zero-width and a short slice return `InvalidInput`; ledger and `wanted_version()` unchanged. | Remove the capacity check; check `>= len`. |

- [ ] **Step 2: Run red; port the pool.**
- [ ] **Step 3: Mutation and repetition protocol** (5x under 16 busy loops).
- [ ] **Step 4: Run the per-task gate; commit only after every command passes.** Subject: `feat(kms): add the cursor sprite pool`.

### Task 4: Capability negotiation, driver facts and the testable open sequence

**Files:**

- Create/Test: `crates/yserver/src/drm/client_caps.rs` (tests in the same file's `#[cfg(test)] mod tests`, ABI test included).
- Modify: `crates/yserver/src/drm/mod.rs` (`pub mod client_caps;` and `pub use client_caps::CursorDeviceCaps;`).
- Modify: `crates/yserver/src/drm/device.rs` (fields, accessor, the post-open function, test constructor; `Device::open` calls the post-open function but the cursor step is **not** part of it yet).

**Interfaces:**

- Produces in `client_caps.rs` (port reference `drm/client_caps.rs` lines 1-275, with these deliberate differences):
  - `DeviceClientCapability::{UniversalPlanes, Atomic, CursorPlaneHotspot}`.
  - `CursorDeviceCaps { hotspot_cap_accepted: bool, driver_name: Option<String> }` with `new(..)`, `cursor_plane_hotspot_cap_accepted()`, `driver_name() -> Option<&str>`, `is_paravirtualized(&self) -> bool` (name exactly one of `virtio_gpu`, `vmwgfx`, `qxl`, `vboxvideo`; case- and suffix-sensitive; `None` is false) and `requires_hotspot_support(&self) -> bool` (`is_paravirtualized()` or `driver_name.is_none()`, i.e. the name query failed; spec R1). The reference puts the name list in `topology.rs` and makes the name query fatal: do not copy either. "No negotiation happened" is `Device::cursor_caps() == None`, not a `CursorDeviceCaps` value.
  - `trait DeviceControl: Send + Sync { fn set_client_capability(&self, &Device, DeviceClientCapability) -> io::Result<()>; fn driver_name(&self, &Device) -> io::Result<String>; fn crtc_in_vblank_event(&self, &Device) -> io::Result<u64>; }` (`crtc_in_vblank_event` wraps `get_driver_capability(DriverCapability::CRTCInVBlankEvent)` so P1's check is testable), `DrmDeviceControl` (production; the hotspot request through the 16-byte `DRM_IOCTL_SET_CLIENT_CAP` wrapper with value 6, request typed with `nix::request_code_write!` and cast `as _`, reference lines 79-123).
  - `negotiate_cursor_caps(device: &Device, control: &dyn DeviceControl) -> CursorDeviceCaps`: requests `CursorPlaneHotspot` (failure: debug, cached `false`), then queries the driver name (failure: warn, `None`). It never errors. It does **not** request UniversalPlanes/Atomic (the open sequence does).
  - `#[cfg(test)]`: `DeviceControlCall::{SetClientCapability(DeviceClientCapability), DriverName, CrtcInVBlankEvent}`, `RecordingDeviceControl::new(capability_results, driver_name_result, crtc_event_result)`, `calls()`, `recording_control(driver_name)`.
  - The ioctl ABI test `set_client_cap_ioctl_matches_target_abi`, ported verbatim from reference lines 245-275 (copy the `cfg` ladders exactly).
- Produces in `device.rs`: fields `device_control: Arc<dyn DeviceControl + Send + Sync>` (default `Arc::new(DrmDeviceControl)`) and `cursor_caps: Option<CursorDeviceCaps>` (`None` in every constructor); `pub fn cursor_caps(&self) -> Option<&CursorDeviceCaps>`; `fn configure_kms_device(&mut self, control: &dyn DeviceControl) -> io::Result<()>`: **replaces `enable_atomic_capabilities`** (keep its two warning texts verbatim, so UniversalPlanes then Atomic are requested once each through `control`) and then P1's `require_crtc_in_vblank_event` using `control.crtc_in_vblank_event`. `Device::open` calls it with `DrmDeviceControl` after `acquire_master_lock`. `#[cfg(test)] pub(crate) fn for_tests_with_control(control: Arc<dyn DeviceControl + Send + Sync>) -> io::Result<Self>`: opens `/dev/null`, runs `configure_kms_device(control)`. P2b Task 5 adds the cursor step (`negotiate_cursor_caps` and caching) between the Atomic request and the CRTC-event check.
- Observable behavior in P2a: none (the same two capability requests and the same check, in the same order, once each).

- [ ] **Step 1: Write the capability tests** (pure boundary tests, using `/dev/null` devices; no hardware):

| Test | Drives and asserts | Mutation that must fail it |
| --- | --- | --- |
| `open_sequence_requests_universal_atomic_then_checks_crtc_events_once` | `Device::for_tests_with_control(recording_control("amdgpu"))`: `calls()` is exactly `[Set(UniversalPlanes), Set(Atomic), CrtcInVBlankEvent]`, no cursor step, `cursor_caps()` is `None`. A scripted `Err`/`0` CRTC-event result fails construction with the device named (P1 contract). | Request a capability twice; swap Atomic and the CRTC check; keep `enable_atomic_capabilities` alongside the new function. |
| `negotiate_cursor_caps_orders_hotspot_then_driver_name` | `negotiate_cursor_caps` with a recording control: calls `[Set(CursorPlaneHotspot), DriverName]`; accepted -> `hotspot_cap_accepted`, name cached. | Query the name first. |
| `physical_driver_hotspot_rejection_is_non_fatal_and_cached_false` | Hotspot scripted `Err(EOPNOTSUPP)`: no panic/error, `hotspot_cap_accepted == false`, `requires_hotspot_support() == false` for `amdgpu`. | Treat rejection as fatal; cache `true` on failure. |
| `unknown_name_query_failure_requires_hotspot_support` | Name query `Err(EIO)`: `driver_name() == None`, `requires_hotspot_support() == true`, `is_paravirtualized() == false`. | Return an error on name failure; default an unknown name to physical. |
| `paravirtualized_names_are_exact_and_unknown_needs_hotspot` | Table: `virtio_gpu`, `vmwgfx`, `qxl`, `vboxvideo` -> paravirtualized (true); `msm`, `xe`, `i915`, `amdgpu`, `nouveau`, `nvidia-drm`, `apple`, `Virtio_gpu`, `virtio_gpu2`, `qxl-drm` -> false; `None` -> `requires_hotspot_support` true but `is_paravirtualized` false. | Prefix or case-insensitive match; treat every unknown name as paravirtualized. |
| `render_node_and_inherited_fd_devices_have_no_caps` | `Device::open_render_node("/dev/null")` and `Device::from_inherited_kms_fd(..)` report `cursor_caps() == None` and request nothing. | Make either constructor negotiate. |
| `set_client_cap_ioctl_matches_target_abi` | Ported ABI assertion. | Change the `_IOW` direction/size encoding. |

- [ ] **Step 2: Run red, port the module, refactor `Device::open`, run green.** Do not call `negotiate_cursor_caps` from the open sequence yet; leave a one-line comment naming P2b Task 5.
- [ ] **Step 3: Cross-target buildability evidence.** Attempt `cargo check -p yserver --target x86_64-unknown-linux-musl` and the FreeBSD target; local sysroots may be missing (P1 recorded that native `-sys` build scripts stop first). Record unexecuted targets honestly; CI's FreeBSD VM job is the evidence for the portable ioctl type.
- [ ] **Step 4: Mutation and repetition protocol** (5x under 16 loops).
- [ ] **Step 5: Run the per-task gate; commit only after every command passes.** Subject: `feat(drm): add cursor capability negotiation and a testable KMS open sequence`.

### Task 5: Property discovery and the static cursor state path

**Files:**

- Create: `crates/yserver/src/kms/cursor_plane/topology.rs`.
- Modify: `crates/yserver/src/kms/cursor_plane.rs` (module registration, discovery trait seam, stored properties and caps, sprite pool + buffer I/O fields, path accessors, test constructor).
- Modify: `crates/yserver/src/kms/render/platform.rs` (`KmsCursorState::state_path()` reads the plane's stored path; `Legacy` when there is no plane).
- Modify: `crates/yserver/src/kms/render/platform/cursor.rs` (`install_cursor_plane_for_device`, the refresh function's active-count update, and a test installer).
- Modify/Test: `crates/yserver/src/kms/render/platform/tests.rs` (refresh test), `crates/yserver/src/kms/render/backend/tests/cursor_sprites/{mod,support}.rs` (path-install test).
- Modify: `crates/yserver/src/kms/render/backend/tests/cursor_sprites/support.rs` (extend `assert_no_cursor_resources` with the pool assertion described below).

**Interfaces:**

- Produces in `topology.rs` (port reference `kms/cursor_plane/topology.rs`, lines 1-170 for discovery and properties, plus 240-273 for the test snapshot, with these changes):
  - `DiscoveredCursorPlane { handle, crtcs: HashSet<crtc::Handle>, properties: HashMap<String, property::Handle> }`; `CursorPlaneDiscovery { crtcs: Vec<crtc::Handle>, planes: Vec<DiscoveredCursorPlane> }` (no `caps` field: master's `Device::cursor_caps()` is read by the caller); `trait CursorPlaneDiscoveryIo { fn discover(&self, device: &Device) -> io::Result<CursorPlaneDiscovery>; }`; `DrmCursorPlaneDiscovery` (the production implementation: the same plane enumeration master does in `discover_cursor_plane_crtcs`, `kms/cursor_plane.rs:382`, but recording **every** property name -> handle, not only `CRTC_X`/`CRTC_Y`).
  - `CursorPlaneProperties { plane, fb_id, crtc_id, crtc_x, crtc_y, crtc_w, crtc_h, src_x, src_y, src_w, src_h, hotspot_x: Option<_>, hotspot_y: Option<_> }` (`Copy`, `Eq`).
  - The matching functions `assign_cursor_planes` / `augment_cursor_matching` **stay where master has them in `cursor_plane.rs` (or move verbatim to `topology.rs`)**; they must keep operating on all discovered planes without filtering by property completeness (spec R2). Master's existing matching tests (`cursor_planes_prefer_exclusive_and_never_serve_two_crtcs`, `cursor_plane_assignment_is_a_maximum_matching`, `cursor_plane_matching_keeps_exclusive_planes`, `cursor_plane_assignment_leaves_an_uncovered_crtc_unassigned`) must pass unchanged. **Do not** port the reference's eligibility-then-matching order (`cursor_plane_topology` filters planes by properties first): P2 matches first and decides per device afterwards.
  - `enum CursorStatePath { Atomic, Legacy }` (`Copy`, `Eq`, `Debug`) and `struct CursorPathDecision { path: CursorStatePath, reason: CursorPathReason }`; `enum CursorPathReason { NoCursorPlane, NoDeviceCaps, MissingProperty(&'static str), HotspotSupportMissing, Qualified }` (exactly these five; there is no topology reason, see spec R3) with a `Display` giving the lowercase log text (`no-cursor-plane`, `no-device-caps`, `missing-property:FB_ID`, `hotspot-support-missing`, `qualified`).
  - `pub(super) fn decide_cursor_state_path(assignment: &HashMap<crtc::Handle, plane::Handle>, planes: &[DiscoveredCursorPlane], caps: Option<&CursorDeviceCaps>) -> CursorPathDecision`: pure; spec R3 in this order: no plane assigned -> `Legacy/NoCursorPlane`; `caps == None` -> `Legacy/NoDeviceCaps`; any assigned plane missing one of `FB_ID, CRTC_ID, CRTC_X, CRTC_Y, CRTC_W, CRTC_H, SRC_X, SRC_Y, SRC_W, SRC_H` (that order) -> `Legacy/MissingProperty(name)`; `caps.requires_hotspot_support()` and (`!hotspot_cap_accepted` or any assigned plane lacks `HOTSPOT_X` or `HOTSPOT_Y`) -> `Legacy/HotspotSupportMissing`; otherwise `Atomic/Qualified`. It takes no active-CRTC set: CRTCs without an assigned plane are master's `topology_blocked` availability gate, not part of the path.
  - `#[cfg(test)]`: `CursorPlaneTestPlane`, `CursorPlaneTestSnapshot` (`crtcs`, `planes`, `driver_name: Option<String>`, `hotspot_cap_accepted`) and `into_discovery()` plus the matching `CursorDeviceCaps` (reference lines 215-273; property handles synthesized as `0x9000 + index`, unique per name within a plane).
- Produces in `cursor_plane.rs`: `CursorPlane` gains `caps: Option<CursorDeviceCaps>` (captured **once** at construction: production `device.cursor_caps().cloned()`, tests from the snapshot; nothing else reads `Device::cursor_caps()` for the decision), `discovery: CursorPlaneDiscovery`, `buffer_io: Rc<dyn CursorBufferIo>`, `sprites: SpritePool`, `properties_by_crtc: HashMap<crtc::Handle, CursorPlaneProperties>` (every assigned plane that exposes all ten names; hotspot handles `Option`) and `state_path: CursorPathDecision`, all computed in the constructor and **immutable afterwards**. New `CursorPlane::new_with_discovery(device: Rc<Device>, crtcs: &[crtc::Handle], buffer_io: Rc<dyn CursorBufferIo>, discovery_io: &dyn CursorPlaneDiscoveryIo, caps: Option<CursorDeviceCaps>) -> io::Result<Self>`; `CursorPlane::new(device, crtcs)` delegates with `DrmCursorBufferIo`, `DrmCursorPlaneDiscovery` and `device.cursor_caps().cloned()`, so production and tests share one body (this replaces master's inline `discover_cursor_plane_crtcs`/`cursor_plane_topology` call, keeping `explicit_plane_assignment` and `atomic_positions` outputs identical). Accessors: `pub(crate) fn state_path(&self) -> CursorPathDecision`, `properties_for(crtc) -> Option<CursorPlaneProperties>`, `sprites(&self) -> &SpritePool`, `sprites_mut(&mut self) -> &mut SpritePool`, `buffer_io(&self) -> Rc<dyn CursorBufferIo>` (dead until P2b Task 2; tests read the pool through `sprites()`). The pool is created with the driver cursor-cap dimensions `new` already queries, `set_active_crtc_count(crtcs.len())`, and allocates nothing until `prepare`. Legacy dumb-buffer allocation stays exactly master's. `retain_crtcs` and refresh never touch the assignment, `properties_by_crtc` or the path. Test setters/constructor: `#[cfg(test)] for_tests_set_show_executor(&mut self, Rc<dyn CursorShowExecutor>)` (beside `for_tests_set_hide_executor`, `kms/cursor_plane.rs:555`) and `#[cfg(test)] pub(crate) fn for_tests_from_snapshot(device: Rc<Device>, crtcs: &[crtc::Handle], buffer_io: Rc<dyn CursorBufferIo>, snapshot: CursorPlaneTestSnapshot, width: u32, height: u32) -> Self` running the same `new_with_discovery` body over a recording discovery with the snapshot's caps, no ioctl, building the legacy buffer from `for_tests_stub`'s in-memory backing (`kms/cursor_plane.rs:490`).
- Produces in `platform.rs` / `platform/cursor.rs`: `KmsCursorState::state_path(&self) -> CursorStatePath` (the plane's stored path, `Legacy` without a plane); `install_cursor_plane_for_device` (`platform/cursor.rs:261`) logs one info line `render cursor: device {key} cursor_state_path={path} reason={reason}`; **no production code reads `state_path()` in P2a**. `refresh_cursor_topology_for_devices_with` (`platform/cursor.rs:1374`) keeps recomputing master's `atomic_cursor` and `topology_blocked`, additionally calls `plane.sprites_mut().set_active_crtc_count(crtcs.len())` for the changed device, and does nothing to the path. `#[cfg(test)] pub(crate) fn for_tests_install_cursor_plane_from_snapshot(&mut self, device_idx: usize, snapshot: CursorPlaneTestSnapshot, buffer_io: Rc<dyn CursorBufferIo>, width: u32, height: u32)`: builds the plane via `for_tests_from_snapshot` for the device's active CRTCs and runs the real `install_cursor_plane_for_device`.
- `assert_no_cursor_resources` additions (support.rs): for every device with a plane, `plane.sprites().all_users_zero()`; and, when a test passes its ledger, `ledger.end_state() == (0, 0, 0)` (`fn assert_no_cursor_resources(backend, ledger: Option<&CursorBufferLedger>)`; update Task 1's call sites); `sprites()` is the read accessor used.

- [ ] **Step 1: Write the tests first.**

| Test | Drives and asserts | Mutation that must fail it |
| --- | --- | --- |
| `cursor_path_decision_table` (pure boundary) | Table over the decision function with `(path, reason)`: no planes assigned; `caps == None` with everything else present (`NoDeviceCaps`, even for a complete plane); each of the ten required names removed individually (ten rows, `MissingProperty(<name>)`); physical `amdgpu` with the cap rejected and all ten present -> `Atomic`; each of `virtio_gpu`, `vmwgfx`, `qxl`, `vboxvideo` with cap accepted and both hotspot properties -> `Atomic`; the same with the cap rejected -> `HotspotSupportMissing`; accepted cap but `HOTSPOT_Y` missing -> `HotspotSupportMissing`; name-query-failed caps (`driver_name == None`) with the cap rejected -> `HotspotSupportMissing`; physical driver missing hotspot properties -> `Atomic`; `msm`/`xe`/`i915`/`nouveau`/`nvidia-drm`/`apple` complete -> `Atomic`. | Require only `CRTC_X`/`CRTC_Y`; demote a physical driver on a rejected cap; treat `caps == None` as an unknown driver; accept a paravirtualized plane without hotspot properties. |
| `cursor_matching_ignores_property_completeness` (pure boundary) | Two cursor planes, one lacking `FB_ID`, exclusive to a CRTC: the assignment still gives that plane (matching unchanged from master) and the decision is `Legacy/MissingProperty("FB_ID")`. Master's four matching tests pass unchanged. | Filter planes by completeness before matching. |
| `plane_from_snapshot_stores_properties_caps_and_allocates_no_sprite` (pure boundary) | `CursorPlane::for_tests_from_snapshot` with a complete plane: `properties_for(crtc)` is `Some` with ten distinct handles (hotspot present when listed); `state_path()` is `Atomic` using **only** the snapshot's caps (the `/dev/null` device has `cursor_caps() == None`); the ledger has no record and `sprites().all_users_zero()`. | Read `Device::cursor_caps()` for the decision (fails at `Atomic`); allocate the initial pair in the constructor. |
| `installing_a_complete_plane_computes_atomic_but_changes_no_behavior` (backend-level, headless) | `AtomicEventFixture::new(1)`; install a complete-plane snapshot via `for_tests_install_cursor_plane_from_snapshot`; `device.cursor.state_path() == Atomic`. Make the plane visible (`plane.for_tests_set_visible(crtc, true)`), `Backend::warp_pointer_root`, `Backend::maybe_composite`: exactly one accepted `CursorOnly` record whose cursor-plane properties are exactly `CRTC_X` and `CRTC_Y`; `retire_pending_output` settles it; the buffer ledger has no record; `finish_fixture` passes. | Make install allocate or bind through the pool; branch `submit_atomic_cursor_move_for_output` on the path. |
| `installing_an_incomplete_plane_computes_legacy_and_the_same_behavior` (backend-level) | `FB_ID` removed: `Legacy/MissingProperty("FB_ID")` and the identical `CRTC_X`/`CRTC_Y`-only record (master hybrid). | Compute `Atomic` for an incomplete plane. |
| `apple_dcp_style_device_keeps_the_legacy_probe` (backend-level) | Zero-plane snapshot: `explicit_plane_assignment` is `None`, `Legacy/NoCursorPlane`, `cursor_plane_available_for_output(0)` true (optimistic probe), and `note_unbound_cursor_failure(0, crtc, ENXIO)` (the hook master's `unsupported_cursor_failure_latches_plane_unavailable`, `platform/tests.rs:1890`, uses) latches `hw_cursor_disabled_for_device` and `!cursor_plane_available()`. | Treat zero planes as blocked, or latch at install. |
| `topology_refresh_never_changes_the_path_or_the_assignment` (boundary test in `platform/tests.rs`, beside master's refresh tests) | Device with an `Atomic` plane and 2 active CRTCs sharing one cursor plane: refresh with a set that leaves a CRTC unassigned -> `topology_blocked` toggles to true and back, `state_path()` stays `Atomic`, `properties_for` and `explicit_plane_assignment` are identical before and after, and `sprites().set_active_crtc_count` followed the active count; refreshing another device leaves this one untouched. | Recompute the path (or the matching) at refresh. |

- [ ] **Step 2: Run red.** Implement in this order: `topology.rs`, then `CursorPlane` integration (keeping the four existing master matching tests green), then the platform field/log/refresh, then the test installer and the extended `assert_no_cursor_resources`.
- [ ] **Step 3: Regression sweep.** Run all of `cargo test -p yserver --locked --features xdmcp kms::cursor_plane`, `platform::tests` and `atomic_cursor` and confirm every pre-existing test passes without edits other than constructor argument updates forced by the new `CursorPlane` fields (list any edited existing test in the report).
- [ ] **Step 4: Mutation and repetition protocol** (5x under 16 busy loops; none of these tests waits on GPU).
- [ ] **Step 5: Run the per-task gate; commit only after every command passes.** Subject: `feat(kms): discover cursor plane properties and decide the static cursor state path`.

### Task 6: Scene seam with Legacy implementations (no behavior change)

**Files:**

- Modify: `crates/yserver-render/src/kms/render/scene.rs` (`CursorTransitionResult::Deferred`; re-export `CursorUploadOutcome`).
- Modify: `crates/yserver-render/src/kms/render/scene/sink.rs`, `scene/cursor.rs`, `scene/flip.rs`.
- Modify/Test: `crates/yserver-render/src/kms/render/scene/tests/{mod,cursor}.rs` (`TestSink`, pure tests).
- Modify: `crates/yserver/src/surface/platform.rs`, `crates/yserver/src/kms/render/platform/scene_sink.rs`.
- Modify: `crates/yserver/src/kms/cursor_plane.rs` (`CursorShowError::Deferred`), `crates/yserver/src/kms/render/platform/cursor.rs` (Legacy-only upload/matches adapters with the new signatures), `crates/yserver/src/kms/render/backend/kms/scanout.rs` (direct hook: `Ok(Deferred)` is a hook failure like an `Err`; unreachable on Legacy).

**Interfaces:**

- Port the scene-seam diffs of `git show 14fc3e47 -- crates/yserver-render/src/kms/render/scene/{cursor,flip,sink}.rs crates/yserver-render/src/kms/render/scene.rs`. **Keep:** `CursorTransitionResult::Deferred` with `retained_mode` resolution (actual mode = retained, `commit_desired_metadata == false`, `force_repaint == true`, `scene_structure_dirty = true`), `update_force_show_retry_version` treating `Deferred` like `VisibleNeedsShowRetry`, `CursorShowFailure.deferred`, `CursorUploadOutcome`, the hotspot parameter on `upload`, `uploaded_image_matches` replacing `uploaded_version`. **Do not port** the reference's change of the `no matching entry bytes` branch to `Deferred` nor the removal of `resolve_failed_cursor_upload`: master's resolution of that branch and of a failed upload stays on every path (spec R19).
  - `pub enum CursorUploadOutcome { Ready, Deferred }` in `scene/sink.rs`, re-exported from `scene.rs`.
  - `CursorPlaneSink::uploaded_image_matches(&self, output_idx, version, hot_x, hot_y) -> bool`; `upload(&mut self, output_idx, version, hot_x, hot_y, width, height, bgra_bytes) -> io::Result<CursorUploadOutcome>`.
  - `apply_cursor_transition_on_retire`: an `Ok(Deferred)` upload returns `Deferred`; a `show` failure with `deferred == true` returns `Deferred`; everything else as master.
  - Implementors (three, all in the tree): `PlatformBackend` (Legacy: `cursor_plane_upload_image_for_output(output_idx, version, hot_x, hot_y, width, height, bytes)` calls `plane.upload_image` as master and returns `Ready`, classification unchanged; `cursor_plane_uploaded_image_matches_for_output` is `uploaded_version == Some(version)` and ignores the hotspot); `SurfacePlatform` (version-only match, `upload` stores the hotspot, returns `Ready`, `show` sets `deferred: false`); `TestSink` (`Ready`, records the hotspot it was given).
  - `CursorShowError::Deferred { remains_visible: bool, reason: io::Error }` with `remains_visible()` returning the flag and `is_deferred()`; `needs_full_rebind` false for it; `scene_sink.rs` maps `is_deferred()` to `CursorShowFailure.deferred`. Nothing on the Legacy path constructs it.
- Observable behavior: none. Every Legacy sink returns `Ready` and `deferred: false`.

- [ ] **Step 1: Write the tests first** (render-crate pure boundary tests with fake sinks; Legacy regression through the unmodified master suites):

| Test | Drives and asserts | Mutation that must fail it |
| --- | --- | --- |
| `deferred_transition_keeps_the_retained_mode_and_forces_a_show_retry` | `apply_cursor_transition_on_retire` with a `TestSink` whose `upload` returns `Deferred`, then one whose `show` returns `deferred: true`: `CursorTransitionResult::Deferred`; `resolve_retired_cursor_state(Deferred, desired, retained)` gives `actual_mode == retained`, no metadata commit, `force_repaint`; `update_force_show_retry_version` returns the attempted version. | Map `Deferred` to `Hidden` (drops to software); clear the retry version. |
| `legacy_sink_resolutions_are_unchanged` | A Legacy-style `TestSink` (always `Ready`, `deferred: false`): the three master outcomes still hold: upload `Err` with a prior visible binding and failing hide -> `VisibleNeedsShowRetry`; upload `Err` with no prior binding -> `Hidden`; no matching entry bytes -> master's `resolve_failed_cursor_upload` result (not `Deferred`). | Return `Deferred` for the no-matching-bytes branch. |
| `surface_platform_sink_keeps_version_only_identity` (in `surface/tests.rs` or beside the impl) | `uploaded_image_matches(v, 1, 1)` is true after `upload(v, ..)` regardless of hotspot; `show` reports `deferred: false`. | Compare the hotspot. |
| master's `scene::tests`, `platform::tests`, `backend::tests::atomic_cursor` and `present_scanout` suites | Pass with only the mechanical signature edits (list each edited test). | n/a |

- [ ] **Step 2: Run red, implement:** scene crate first, then the three implementors and the platform adapters, then the direct hook; fix every compile consumer in this task.
- [ ] **Step 3: Mutation and repetition protocol** (5x under 16 busy loops).
- [ ] **Step 4: Run the per-task gate; commit only after every command passes.** Subject: `refactor(kms): add the deferred-upload seam to the cursor plane sink`.

## Coverage table

| Spec rule / constraint | Task and decisive test |
| --- | --- |
| R1 open sequence once each, non-fatal rejection, exact names, unknown vs no caps | T4 `open_sequence_requests_universal_atomic_then_checks_crtc_events_once`, `negotiate_cursor_caps_orders_hotspot_then_driver_name`, `physical_driver_hotspot_rejection_is_non_fatal_and_cached_false`, `unknown_name_query_failure_requires_hotspot_support`, `paravirtualized_names_are_exact_and_unknown_needs_hotspot`, `render_node_and_inherited_fd_devices_have_no_caps`; the cursor step is wired in P2b T5 |
| R2 property handles recorded, matching unchanged | T5 `cursor_matching_ignores_property_completeness`, `plane_from_snapshot_stores_properties_caps_and_allocates_no_sprite`, master's matching tests |
| R3 static path decision from stored caps, logged once | T5 `cursor_path_decision_table`, install tests, `topology_refresh_never_changes_the_path_or_the_assignment` (P2b T5 turns it on by supplying caps) |
| R4 no cursor plane keeps legacy probe | T5 `apple_dcp_style_device_keeps_the_legacy_probe` |
| R5 full-capacity ARGB pool, padding, growth bound | T2 `cursor_buffer_allocation_order_and_full_geometry`, `cursor_buffer_write_places_top_left_and_clears_padding`; T3 `sprite_pool_allocates_two_full_capacity_argb_buffers`, `sprite_pool_writes_top_left_image_and_clears_pitch_padding`, `sprite_pool_pressure_defers_and_keeps_only_the_newest_wanted_image` |
| R6 all-or-nothing allocation | T2 `cursor_buffer_failure_rolls_back_only_what_was_acquired_in_reverse_order`; T3 `sprite_pool_allocation_failure_rolls_back_in_reverse_order` |
| R7 hotspot identity, lookup over every slot | T3 `sprite_pool_hotspot_only_change_stages_a_distinct_buffer`, `sprite_pool_restaging_a_held_identity_reuses_its_slot` (Backend re-proof in P2b T2) |
| R8 pressure defers without overwriting; pool bound | T3 pressure test |
| R9 oversize and wrong length | T2 and T3 `..._rejects_oversize_and_wrong_length...` |
| R10 pool half (users, shared sprite) | T3 `sprite_pool_never_rewrites_a_buffer_with_users_or_reservations`; attribution is P2b T2 |
| R13 reservation bracket (pool half) | T3 `sprite_pool_reservation_rejection_restores_the_prior_owner` |
| R15 teardown never frees a used framebuffer | T3 `sprite_pool_drop_forgets_buffers_with_live_users` |
| R19 scene seam, Legacy results unchanged | T6 `deferred_transition_keeps_the_retained_mode_and_forces_a_show_retry`, `legacy_sink_resolutions_are_unchanged`, `surface_platform_sink_keeps_version_only_identity` |
| R24 assignment fixed | T5 `topology_refresh_never_changes_the_path_or_the_assignment` |
| R25 legacy unchanged | T5 install tests, T6 regression sweep, whole master suite |
| R11, R12, R14, R16-R18, R20-R23 | P2b |
| A end-state | T1 `assert_no_cursor_resources`/`finish_fixture`, T5 extension, every test's ledger assertions |
| B production drivers | T1 and T5 backend tests; T2-T4 and T6 stated boundary tests, re-proven through Backend in P2b |
| F repetition and skips | Verification rules; per-task protocol |
| Shared helpers first and small | T1 |
| No cfg(test) in production bodies; no env vars; layout ceilings | Every gate; T2 splits `ops/recording.rs` out of the production file |

## Open questions and handoff

No design question blocks P2a. Rulings of revision round 1 are applied (static per-device path, paravirtualized without the capability stays Legacy, exact name list, six tasks). Hardware authorization is a P2b Task 6 matter.

This plan has six tasks and one commit per task. It contains no implementation results.
