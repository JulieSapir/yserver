# DRM Commit Identity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL at implementation time: use superpowers:executing-plans to implement this plan task by task. Work in this worktree without subagents. The user's explicit execution and scope restrictions override the skill's default delegation and handoff suggestions. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make accepted DRM atomic commit IDs survive kernel submission and event decoding, and require those identities for primary-frame retirement.

**Architecture:** Keep master's injectable atomic sink, event sources, scene stages, direct Present, and cursor path. Replace the sink's cookie-less ioctl boundary and the cookie-losing event decoder, then carry accepted IDs through shared/copied scene and direct/unflip retirement. Introduce small shared test helpers first, and finish with a separate coordinator-run hardware controller and behavior-focused status entry.

**Tech Stack:** Rust workspace; locked drm 0.15.0 / drm-ffi 0.9.1; DRM atomic/event UAPI; existing recording executors/event sources; Vulkan/lavapipe where scene resources require it.

**Spec:** [docs/superpowers/specs/2026-10-19-drm-commit-identity-design.md](../specs/2026-10-19-drm-commit-identity-design.md). Read it before executing any task.

## Global Constraints

- P1 changes DRM commit identity and primary-frame retirement only; master's cursor positions, legacy set_cursor2/move_cursor fallback, and cursor transition state remain intact.
- Do not introduce cursor property tagging, the reference EBUSY attribution change, legacy telemetry removal, KmsServices, cursor receipts, sprite pools, or recovery enums.
- Preserve Linux glibc, Linux musl, and FreeBSD ioctl request typing and buildability; do not assume libc::Ioctl exists on every target.
- Require DRM_CAP_CRTC_IN_VBLANK_EVENT == 1 on each real KMS device at setup; Linux >= 4.12 always advertises this capability.
- No new YSERVER_* behavior environment variables and no cfg(test) branches inside production function bodies.
- Respect tools/check-layout.sh: code files at most 5,000 lines and test files at most 6,000 lines, plus its dependency and platform seams.
- Keep yserver-render independent of drm, gbm, input, udev, and yserver-core; shared scene interfaces use Option<u64> for event identity.
- Do not change scheduling, admission, retry timing, async qualification, or cursor rejection policy.

This document is planning only. Its preparation authorizes writes only to `docs/superpowers/specs/` and `docs/superpowers/plans/` in `/home/ariel_santangelo/Projects/yserver-commit-id`, with no git writes, cargo runs, device access, hardware, or subagents. Tasks below describe future implementation, not actions taken during planning. The listed source/status writes and commits require a later implementation instruction that permits them. Do not merge as part of this plan; retain the repository's confirmation rule for a later merge.

Use `feat/drm-commit-identity` at base `cb8297d0`. Read-only porting source: `/home/ariel_santangelo/Projects/yserver-cursor` at `9023da14`. Read inventory sections 1, 3, and 4 at `/home/ariel_santangelo/Projects/claude-scratch/atomic-cursor-split/inventory.md`, then inspect each candidate hunk. Reference line numbers below refer to that HEAD, not master. Do not cherry-pick whole reference files. This plan gives contracts, invariants, scenarios, and mutation oracles; implementation code is deliberately left to the porter.

## Review Focus

1. A zero CRTC or failed capability query must never turn a commit cookie into a device object: Task 2 `dispatch_does_not_reinterpret_user_data_when_crtc_id_is_zero` and `crtc_event_capability_requires_one_and_preserves_query_errors`.
2. Batched, truncated, or unknown raw event records must preserve the next walkable record and the full sequence-cookie namespace: Task 2 raw-event matrix, especially `dispatch_walks_every_event_in_one_read` and `dispatch_decodes_crtc_sequence`.
3. Identical handles on different devices, a queue discard, and an inactive seat must not let an old event consume new obligations: Task 2 device, discard, and inactive-drain tests; retain the existing lifecycle discard order during review.
4. An async rejection followed by a failed or successful Vsync fallback must leave only actual accepted work: Task 3 shared-scene and Task 4 direct fallback tests, including the both-attempts-fail cases.
5. A copied or partial degraded unflip must hold the direct buffer even when its accepted-replacement map is temporarily empty: Task 4 mixed-output acceptance test, including delayed/rejected sink acceptance and source-pin assertions.

## Verification rules for all tasks

- **A:** Each stateful test inspects retention at every wrong/delayed event, explicitly completes accepted work, and asserts no sink entries, scene acks/render jobs/copy obligations, Pending BOs, direct/successor/unflip requirements, accepted-replacement map entries, or source/fallback pins remain. Drain deferred GPU/pool releases through `Backend::before_block` and other normal Backend entries with a bounded wait. An OnScreen baseline BO is valid. Assert quiescence before freeing fixture drawables through Backend entries; fixture drop, map clearing, and forced scene drains do not establish normal completion.
- **B:** Setup may install topology, baseline BOs, recording executors/sources, declared capabilities, and probe results. After setup, advance lifecycle only through `Backend` trait entries or `core_loop::run_core`. Do not call `submit_direct_frame_live`, `retire_direct_output`, scene retirement, `simulate_*_for_tests`, or `record_commit` from lifecycle tests; do not seed accepted IDs/acks or replace the live direct submitter. Use explicit-cookie injection, even for the final correct completion. Pure ABI/decoder/policy/immutable-selection tests exercise values without advancing accepted lifecycle state.
- **C:** Task 5 is the separate final hardware controller. The coordinator runs the AMD and NVIDIA cases unattended (user authorization 2026-10-19), checking for GPU clients before each run; software/recording tests do not exercise the real atomic ioctl.
- **F:** Every newly introduced test runs 5 times under exactly 16 busy loops; any test waiting on GPU/render/copy completion runs 20 times. The mutation for each table row must produce a failure at its named assertion, not merely a timeout or build failure. Revert each local mutation before the next case and before the gate. Include the nontrivial acceptance/failure branches in the repetition run.
- Every fixture skip prints exactly one line, for example `SKIP p1_copied_scene: SYNC_FD export unavailable`. Only the constructor deciding the skip prints; `Option` propagation is silent. Unexpected setup errors fail. Preserve the reason, including no Vulkan ICD or missing SYNC_FD export. Run test commands with `--nocapture` when auditing skips, and count executed versus skipped cases.
- Current CI **does run ignored tests**: `.github/workflows/ci.yml`, xdmcp leg, software-Vulkan step, runs the workspace `-- --ignored` suite. Capability-dependent fixtures can still skip inside an invoked test. A skipped copied/SYNC_FD case is not lavapipe coverage; it remains on Task 5's controller checklist.
- Do not grow `backend/tests/tearing.rs` (already 5,830 lines) with P1 scenarios. New helpers and scenarios live in focused files below. Code/test ceiling checks include newly tracked files when each future commit is prepared; do not add exceptions to hide growth.
- One commit per task, after its gate. The five intended commit subjects appear with their tasks. Keep all intermediate commits buildable; update consumers and their tests in the same task as an interface change.

For repetition, start one Bash session, save the 16 busy-loop PIDs in a task-specific array, and trap EXIT/INT/TERM to kill and reap only those PIDs. Keep the loops alive while executing each test the required number of times. Use `cargo test --workspace --locked --features xdmcp <full-test-name> -- --exact --nocapture` for ordinary tests; add `--ignored` and the exact lavapipe environment from the gate for ignored GPU tests. Use `--list` first to obtain full names and ensure a filter did not select zero tests. Mutate and run the specific named test once before restoring it, then repeat the restored tests. Record test name, repetitions executed, skip reason/count, and result in the implementation review notes; do not start background load during planning.

## Production caller audit (performed before defining the tests)

Read-only `rg -n` searches on master excluded `tests/` and `for_tests.rs`; the source bodies were then inspected. Re-run these searches if the base changes. A future test does not justify porting a path found only in reference tests.

| Production path relied on | Non-test caller verified on master | Driver used by planned tests |
| --- | --- | --- |
| Device KMS setup / capability check insertion | `drm/device.rs:88-109` calls `enable_atomic_capabilities`; `kms/outputs.rs:308` and `platform/drm.rs:181` call `Device::open` | Pure result-policy tests plus source wiring review; real setup verified by Task 5, not by opening a device in unit tests |
| Atomic sink accounting / existing request builder | `drm/page_flip.rs:274`; `drm/modeset.rs:1570,1695`; sink `submit_with_tearing_context` calls `record_commit` | Backend scene/direct submission with recording executor; pure raw-layout/selection tests for boundary algorithms |
| Raw flip drain and platform routing | `platform/flip_events.rs:85` calls `page_flip::drain_events`; `backend/kms/scanout.rs:2126` drains it | `Backend::on_page_flip_ready`, including a socket-backed live raw-event source adapter |
| Shared scene submit / acknowledgment | `platform/scene_sink.rs:143,596,622`; `scene/tick.rs` consumes handoff; `backend/kms/scanout.rs:2169` invokes scene retirement | `Backend::clear_area`, `Backend::maybe_composite`, then `Backend::on_page_flip_ready` |
| Copied render-to-sink KMS submission | `scene/flip.rs:308` calls `OutputSink::submit_copied`; `platform/scene_sink.rs:273` calls `submit_copied_scanout`; `scanout.rs:2116` dispatches render completion | `Backend::on_scanout_render_completion`; core loop invokes it at `core_loop/run.rs:2376` |
| Direct Present and Vsync fallback | `core_loop/process_request/present_ext.rs:1012` calls `Backend::try_present_direct`; `backend/kms/present.rs:780` reaches `scanout.rs:599,627`; production state initializes the live submitter in `kms/session.rs:150` | `Backend::try_present_direct` after normal probation, with live submitter and recording executor |
| Grouped unflip / degraded scene replacement | `backend/kms/scanout.rs:2472` invokes `submit_composed_unflip`; failure falls through to production composition; `:1092` calls composed scanout | Ineligible authoritative-root Present through `Backend::try_present_direct`, then `Backend::maybe_composite`; copied completion through its Backend hook |
| Core fd/tick hooks / lifecycle clears | `core_loop/run.rs:2338,3104`; `backend/kms/crtc_config.rs:44,105-119`; `core_loop/run.rs:1433` calls Backend `begin_vt_release` / `finish_vt_release`; acquire uses `on_vt_acquire` | Backend fd/tick/VT entries with existing recording lifecycle executors; do not use the default no-op `on_vt_release` or a reference-only shutdown API |
| Platform-neutral surface completion | `surface/swapchain.rs:308`, `surface/platform.rs:332,440` | Existing surface regression entry paths and workspace tests; identity remains `None` |

To reproduce the central audit, search `submit_scene_flip_with_fences`, `submit_shared_scanout_frame`, `submit_direct_scanout`, `submit_composed_scanout`, `handle_page_flip_complete`, `handle_scanout_render_completion`, `on_page_flip_ready`, `on_scanout_render_completion`, and `try_present_direct` in `crates/` with `rg -n -g '*.rs' -g '!**/tests/**' -g '!**/for_tests.rs'`. Inspect the caller, not just the definition. In particular, reference `Backend::shutdown_outputs` is absent on master: do not import it as a P1 fixture dependency.

## File and interface map

All paths below are relative to this worktree. `Y` means `crates/yserver/src/`; `R` means `crates/yserver-render/src/kms/render/`. These prefixes are expanded explicitly in each task's file list.

| Responsibility | Files |
| --- | --- |
| Cursor-independent recording fixtures and P1 scenarios | `Y/kms/render/backend/tests/commit_identity/{mod,support,recording_scene,atomic_events,scene,direct_unflip}.rs` |
| Atomic ABI/building and accepted pending associations | `Y/drm/atomic.rs`; focused unit tests `Y/drm/atomic/tests.rs` |
| Raw event boundary and setup requirement | `Y/drm/page_flip.rs`, `Y/drm/device.rs`; focused boundary tests `Y/drm/{page_flip,device}/tests.rs` |
| Event metadata and device-qualified dispatch | `Y/kms/render/platform.rs`, `Y/kms/render/platform/flip_events.rs`, non-rendering `Y/present/event_loop.rs` |
| Shared/copied ID propagation and pre-BO identity gate | `Y/drm/page_flip.rs`, `Y/kms/render/platform/{scene_sink,scanout_bos}.rs`, `R/scene{.rs,/sink.rs,/tick.rs,/flip.rs}` |
| Direct/grouped/degraded ID obligations | `Y/drm/modeset.rs`, `Y/kms/render/backend/kms/{state,present,scanout,crtc_config}.rs` |
| Non-KMS shared API adaptation | `Y/surface/{swapchain,platform}.rs`, existing `R/scene/tests/tick.rs` and test sink implementations |
| Final behavior/results documentation | `docs/status.md` and a controller result note under `docs/superpowers/plans/` |

### Task 1: Establish small recording fixtures before identity scenarios

**Files:**

- Create: `crates/yserver/src/kms/render/backend/tests/commit_identity/mod.rs`.
- Create: `crates/yserver/src/kms/render/backend/tests/commit_identity/support.rs`.
- Create: `crates/yserver/src/kms/render/backend/tests/commit_identity/recording_scene.rs`.
- Create/Test: `crates/yserver/src/kms/render/backend/tests/commit_identity/atomic_events.rs`.
- Modify: `crates/yserver/src/kms/render/backend/tests/mod.rs` (register module).
- Modify: `crates/yserver/src/drm/atomic.rs` (request/recorder observation fields and read-only pending-ID probe).
- Modify: `crates/yserver/src/kms/render/platform.rs` and `crates/yserver/src/kms/render/platform/flip_events.rs` (cookie metadata and device-qualified test event injection).
- Modify: `crates/yserver/src/kms/render/platform/init.rs` (initialize the narrow render-completion readiness seam in live and synthetic constructors).
- Modify/Test: `crates/yserver/src/kms/render/backend/tests/tearing/round2.rs` (initialize the new cookie field in its existing synthetic event literal).
- Modify: `crates/yserver/src/kms/render/backend/tests/tearing.rs` only to share extracted recording-scene setup if that removes duplication without changing scenarios.

**Interfaces:**

- Consumes: master's `AtomicCommitSink::new(Rc<dyn AtomicCommitExecutor>)`, `RecordingAtomicCommitExecutor`, `RecordingPageFlipEventSource`, `KmsBackend::for_tests`, live Vulkan fixture constructor, and `Backend` trait hooks already listed in the audit.
- Produces: `AtomicCommitRequest.user_data: u64`, initially zero; `AtomicCommitRecord.{device_fd: RawFd, user_data: u64}` recording that request value and actual submitted fd. The production crate ioctl still submits zero until Task 2.
- Produces: `KernelPageFlip.commit_id: u64`, `FlipEventTimestamps.commit_id: Option<u64>`; live crate-decoded events initially supply zero/`None`, while test injection can retain an explicit cookie. Normalize zero only when constructing the optional timestamp identity.
- Produces: `AtomicEventFixture::new(output_count: usize) -> Self` and `AtomicEventFixture::from_backend(backend: KmsBackend) -> Self`; fixture owns `backend: KmsBackend`, `state: ServerState`, and device-fd-indexed recorders/event sources. The base fixture requires neither Vulkan nor a cursor pool.
- Produces: `AtomicEventFixture::queue_commit_completion(&self, device_fd: RawFd, crtc: crtc::Handle, commit_id: u64, frame: u32, ust: Duration)` queues exactly the supplied external observation on that fd; zero is used only in explicitly named compatibility scenarios. It never looks up a current ID automatically and never dispatches retirement.
- Produces: `accepted_event_id(record: &AtomicCommitRecord) -> Option<u64>` accepts only a successful, non-TEST_ONLY, PAGE_FLIP_EVENT record with nonzero `user_data`. Rejected/test/no-event/zero-cookie records return `None`.
- Produces: read-only `AtomicCommitSink::for_tests_pending_page_flip_commit_ids(&self, device: &Device, crtc: crtc::Handle) -> Vec<u64>`; it reads master's current queue until Task 2 changes its key. Add a total pending-entry count probe for leak assertions, as a separately cfg-gated method definition.
- Produces: `RecordingSceneFixture::new(output_count: usize) -> Result<Option<Self>, String>`, wrapping an `AtomicEventFixture` plus ordinary master recording scene pools. `AtomicEventFixture::assert_quiescent(&self)` checks current observable obligations without clearing/draining them. Later tasks extend read-only observations as their state appears.
- Produces: a narrow `RenderCompletionReadiness::is_ready(&self, job_id: u64, fd: Option<BorrowedFd<'_>>) -> io::Result<bool>` seam around master's existing zero-time completion-fd poll in `platform/flip_events.rs:294-319`. The live `PollRenderCompletionReadiness` preserves current poll flags, ready-without-fd behavior, and the caller's warn-and-ready handling on poll error. Install it through `PlatformBackend` construction, with a separately cfg-gated fixture setter. `RecordingRenderCompletionReadiness::{hold_ready_jobs, release_ready_job(job_id: u64)}` may withhold a notification until released but must still require the real poll to report readiness. It never replaces/signals a sync_file or creates a render completion. This is only the small observer seam needed by copied tests; exclude the reference recorder's fd-capture/failure/recovery machinery.

- [ ] **Step 1: Extract infrastructure, with a narrow boundary.** Use inventory §4 row 1 and the master scene-pool setup at `backend/tests/tearing.rs:3132-3260`. Keep only synthetic devices/topology, baseline BOs, recorder installation, explicit-cookie injection, and observations. For the controlled copied readiness required in Task 3, master's `drain_scanout_render_completions` has a non-test caller through the OutputSink drain and Backend completion hook; port only reference `platform/flip_events.rs:20-44` and the minimal hold/release observer contract above. Do not import `round_c_reassign`, `AtomicCursorFixture`, cursor creation/receipts, clocks that advance per read, or automatic retirement. Keep scene setup separate from the base event fixture; expected environmental absence returns `Ok(None)` with one skip line, unexpected construction errors return `Err` and fail the test.
- [ ] **Step 2: Write the following fixture contract tests before their helpers.** No test asserts that a master zero cookie is permanently correct; the smoke test's nonzero-record assertion is added by Task 2 when the submission contract changes.

| Test | Drives and asserts | Mutation that must fail it |
| --- | --- | --- |
| `p1_fixture_routes_events_only_from_the_ready_fd` | Two headless devices intentionally reuse a raw CRTC handle. Queue separately stamped events, dispatch only A through `Backend::on_page_flip_ready`, assert only A's clock/source queue changes; then dispatch B and assert both input queues and obligations empty. | Drain every source on A readiness, or index the event source by CRTC without its fd. |
| `p1_fixture_acceptance_reader_excludes_nonaccepted_records` | Pure record values cover success, rejection, TEST_ONLY, no PAGE_FLIP_EVENT, and zero cookie. Only the successful event-producing nonzero record yields its full-width ID. | Return a rejected or TEST_ONLY record's cookie, or truncate it to 32 bits. |
| `p1_recording_scene_smoke_completes_through_backend` (ignored, GPU-waiting) | Damage root via `Backend::clear_area`, compose via `maybe_composite`, queue completion for each observed accepted record, and dispatch through `on_page_flip_ready`. On pre-Task-2 master this is explicitly a zero-cookie smoke case. Assert actual recorder invocation, normal BO/ack retirement, deferred release completion via Backend hooks, and quiescence. | Replace the fixture's live scene with the fake flip override, or omit event delivery/dispatch while claiming a drained fixture. |

- [ ] **Step 3: Run the focused tests red, then wire the helpers and metadata.** Use `cargo test -p yserver --locked --features xdmcp p1_fixture -- --nocapture` and the smoke test with the gate's lavapipe environment and `--ignored`. Initial failure must identify missing helper/metadata/assertion behavior; final assertions must inspect real recorder calls and obligations. Add cfg gating to whole test methods/modules only, never a new branch inside production code.
- [ ] **Step 4: Check skip ownership and run the repeat/mutation protocol.** The base fixture must not skip for lack of Vulkan. The scene constructor owns its one-line reason; nested factories and callers do not print it again. Run ordinary new cases 5x and the smoke case 20x under 16 busy loops, recording executions/skips. The intended single commit after the gate is `test(kms): share cursor-independent atomic event fixtures`.
- [ ] **Step 5: Run the per-task gate; commit this task only after all commands pass.**

```sh
cargo +nightly fmt --all
cargo clippy --workspace --all-targets --locked --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 LD_LIBRARY_PATH=/var/lib/flatpak/runtime/org.freedesktop.Platform.GL.default/x86_64/25.08/bcfd828b0c4739753cadb31964f8c1b0f90c8d2bed79bda2c2760438eb48c4b3/files/lib cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
tools/check-layout.sh
```

### Task 2: Carry kernel cookies and match accepted device/CRTC entries

**Files:**

- Modify: `crates/yserver/src/drm/atomic.rs`.
- Create/Test: `crates/yserver/src/drm/atomic/tests.rs` (focused raw/immutable-selection tests; retain existing test-support recorder definitions).
- Modify: `crates/yserver/src/drm/page_flip.rs` and `crates/yserver/src/drm/device.rs`.
- Create/Test: `crates/yserver/src/drm/page_flip/tests.rs` and `crates/yserver/src/drm/device/tests.rs`; move existing related tests as needed.
- Modify: `crates/yserver/src/kms/render/platform.rs` and `crates/yserver/src/kms/render/platform/flip_events.rs`.
- Modify: `crates/yserver/src/kms/render/backend/kms/scanout.rs` (attribution/filter insertion only, on active and inactive drains).
- Modify: `crates/yserver/src/present/event_loop.rs` (both callback arities).
- Test/Modify: `crates/yserver/src/kms/render/backend/tests/commit_identity/{support,atomic_events,recording_scene}.rs` and existing atomic/event/tearing tests affected by argument additions.
- Modify/Test: `crates/yserver/src/kms/render/backend/tests/tearing/round2.rs` (supply device to the existing pending-direct-timestamp probe).

**Interfaces:**

- Consumes: Task 1's request/recorder cookie fields, device-indexed source, `queue_commit_completion`, `accepted_event_id`, and recording-scene driver.
- Produces: `RawAtomicModeReq` with sorted object/property arrays, duplicate replacement, and `submit(&self, fd: BorrowedFd<'_>, flags: AtomicCommitFlags, user_data: u64) -> io::Result<()>`. Factor its kernel-request construction into `kernel_request(&self, flags: AtomicCommitFlags, user_data: u64) -> io::Result<drm_mode_atomic>` used by production `submit`, so a pure serialization test inspects the actual ABI value used for ioctl, rather than only the recorder's shadow properties.
- Produces: `AtomicCommitRequest::submit(&self, device: &Device, flags: AtomicCommitFlags) -> io::Result<()>`; `DrmAtomicCommitExecutor` calls it. `CursorPlanePosition::add_to` takes the replacement builder; its content remains position-only.
- Produces: `AtomicCommitSink::submit_with_commit_id(&self, device: &Device, flags: AtomicCommitFlags, kind: AtomicCommitKind, request: AtomicCommitRequest, events: &[AtomicCommitEvent]) -> io::Result<u64>` and `submit_tearing_with_commit_id` with the same inputs plus `context: TearingCommitContext`. Existing `submit` / `submit_tearing` stay `io::Result<()>`. The private common submission returns `io::Result<Option<u64>>`; nonempty event-producing requests allocate before executor invocation, and successful accounting returns that exact ID.
- Produces: `CrtcPageFlipKey { device_fd: RawFd, crtc: crtc::Handle }`; pending association map uses that key. `allocate_commit_id(&mut self) -> io::Result<u64>` uses checked progression, never saturating reuse or zero. `pending_page_flip_index(queue: &VecDeque<PendingPageFlip>, event_id: Option<u64>) -> Option<usize>` is a pure selector used by production matching; nonzero is exact, absent is index zero only for a nonempty queue.
- Produces: pure `event_identity_required(flags: AtomicCommitFlags, events: &[AtomicCommitEvent]) -> bool`, used by the common submission path and ID-returning API validation: only a non-TEST_ONLY PAGE_FLIP_EVENT request with nonempty target events requires an ID. The dedicated `test_only` path still sends no events and no identity association.
- Produces: `AtomicCommitSink::note_async_flip_event(&self, device: &Device, crtc: crtc::Handle, timestamps: FlipEventTimestamps) -> Option<u64>` and `PlatformBackend::note_tearing_async_flip_event(&self, output_idx: usize, timestamps: FlipEventTimestamps) -> Option<u64>`. Return the removed entry's ID or `None`; a nonmatching explicit cookie never removes an entry. Probe APIs now use the device-qualified key.
- Produces: `drain_events<A,S>(device: &Device, on_advance: A, on_sequence: S) -> io::Result<()>`, where `A: FnMut(crtc::Handle, u32, Duration, u64)` and `S: FnMut(u64, i64, u64)`. Live source forwards cookie unchanged into Task 1 metadata. The raw decoder's CRTC comes exclusively from nonzero `crtc_id`.
- Produces: setup query inside `Device::open` and a small result-policy function `require_crtc_in_vblank_event(path: &str, result: io::Result<u64>) -> io::Result<()>` called with `get_driver_capability(DriverCapability::CRTCInVBlankEvent)`. Require exactly 1, contextualize error/unsupported results, preserve render-node and inherited-fd contracts. This helper is a setup check, not a service/capability framework.

- [ ] **Step 1: Write boundary tests using the existing non-test paths in the audit.** Port reference raw tests at `atomic.rs:1626-1652` and `page_flip.rs:544-756`, with the inverted zero-CRTC test. Add the missing cases below; each row is a named test, or an explicitly enumerated parameterized test. These are stateless tests except the socket drain, which exercises the actual public read boundary without frame state.

| Test | Input and assertion | Mutation that must fail it |
| --- | --- | --- |
| `raw_atomic_request_groups_properties_per_object_in_kernel_order` | Insert objects/properties in reverse/mixed order and overwrite one value; assert objects `[10,20]`, counts `[2,1]`, properties `[3,9,7]`, and replacement value in the correct object's slot. | Append duplicate properties or move a property's value to the next object's slice. |
| `drm_mode_atomic_struct_and_ioctl_request_code` | Assert size 56, relevant field offsets through cookie offset 48, reserved zero, and typed request code `0xC03864BC`; event ABI size test below remains independent. | Omit/reorder the reserved/cookie fields or encode a different ioctl payload size. |
| `raw_atomic_kernel_request_preserves_full_width_cookie` | Production-used ABI constructor receives a cookie with high bits set and actual arrays; assert cookie, flags, counts, and referenced array values. | Hardcode `user_data` to zero in kernel-request construction, even while the recorder still sees the correct ID. |
| `atomic_event_identity_policy_excludes_test_only_and_no_event_requests` | Pure flags/target-event values cover valid event request, no-event flags, empty targets, and TEST_ONLY, including invalid mixed flags. Assert only the valid event request requires identity. Separately review that ID-returning API validation uses this predicate before executor invocation; this pure test does not claim to exercise submission ordering. | Change the production-used predicate to accept TEST_ONLY, empty targets, or no-event flags. |
| `pending_page_flip_index_is_exact_or_zero_cookie_fifo` | Immutable queues `[A,B]`, empty, and one entry; assert B selects index 1, unknown/duplicate-retired ID selects none, absent selects oldest only when nonempty. | Use `pop_front` selection for all explicit IDs or fall back to index 0 after a failed explicit lookup. |
| `commit_id_allocator_does_not_reuse_after_exhaustion` | Pure allocator boundary near `u64::MAX`; assert last permitted allocation is nonzero and subsequent attempt errors without repeated/zero ID. | Use reference `saturating_add` or wrap to zero. |
| `crtc_event_capability_requires_one_and_preserves_query_errors` | Pure setup-result policy with 1, 0, another non-1 value, and an ioctl error; only 1 passes, errors name device/capability and preserve query cause. Review its actual `Device::open` caller separately. | Accept unsupported/error results as a warning-and-continue or omit error context. |
| `dispatch_passes_crtc_handle_and_commit_identity_for_page_flip` | Raw 32-byte flip with independent CRTC, high-bit cookie, sequence and time; assert all four callback values exactly. | Truncate/drop cookie or route using its low bits. |
| `dispatch_does_not_reinterpret_user_data_when_crtc_id_is_zero` | Raw flips with `crtc_id=0` and cookie zero/nonzero, including low bits equal to a known CRTC; assert zero flip callbacks, an unroutable diagnostic, and a following valid flip still dispatches. | Restore reference `user_data as u32` fallback. |
| `dispatch_ignores_empty_read` | Empty bytes produce no callbacks. | Fabricate a default flip for an empty read. |
| `dispatch_walks_every_event_in_one_read` | Valid flip, unknown walkable record, sequence event, second flip; assert callback order and preserved values. | Return after the first record or fail to advance over an unknown record. |
| `dispatch_stops_at_an_event_that_overruns_the_read` | First valid event followed by declared overrun and sentinel bytes; only the first callback occurs. | Decode the truncated payload or continue at a guessed next boundary. |
| `dispatch_rejects_short_and_zero_length_event_records` | Parameterize incomplete header, length below header/zero, short flip, and wrong-length sequence payload; assert no invalid callback or loop and only safely walkable subsequent records dispatch. | Remove length guard, or dispatch a short payload as a complete event. |
| `drm_event_vblank_struct_is_32_bytes` | Assert size 32 and offsets for independent `user_data`, sequence, and `crtc_id`. | Replace CRTC with a cookie alias or remove its independent field. |
| `dispatch_decodes_crtc_sequence` | Tagged high-bit sequence cookie, signed timestamp, full-width sequence pass unchanged; no flip callback. | Mask off cookie high bits or convert timestamp to unsigned before callback. |
| `drain_events_delivers_the_commit_identity_of_a_flip_complete` | Feed real bytes through a socket-backed inherited `Device`, call `drain_events`, assert poll/read path delivers independent CRTC and cookie; retain no-pending-event drain regression. | Keep the crate `receive_events` path and synthesize cookie zero. |

- [ ] **Step 2: Add backend-driven accounting tests.** Use `RecordingSceneFixture` and `Backend::clear_area` / `maybe_composite`; inspect records after submission, queue named cookies, then invoke `Backend::on_page_flip_ready` for the correct fd. These GPU-waiting tests are ignored with an explicit Vulkan reason. Update Task 1's smoke test to require `accepted_event_id` for accepted KMS events and inject that ID. End every scenario with A's complete leak checks.

| Test | Drives and asserts | Mutation that must fail it |
| --- | --- | --- |
| `p1_submit_allocates_cookie_before_executor_and_only_enqueues_success` | Record successful, rejected, then successful scene requests in normal Backend cycles. Assert every attempted event request has a distinct nonzero ID, the success cookie equals its pending entry, rejection leaves no entry, and a failed-attempt cookie cannot retire subsequent accepted work. Dedicated no-event/TEST_ONLY policy is covered by the pure boundary test and existing attribution-path regressions; do not call the sink directly to fake a Backend path. | Allocate after executor, enqueue on failure, or reuse the failed ID on the next accepted request. |
| `p1_explicit_cookie_duplicate_and_unknown_retire_nothing` | Complete accepted A, submit B, then inject A again and a never-accepted cookie. Assert B's queue entry and scene BO state stay unchanged; deliver B and assert no entries remain. | On explicit miss, remove the queue front and return its ID. |
| `p1_zero_cookie_preserves_fifo_attribution` | Submit accepted scene work, explicitly inject cookie zero, assert oldest accounting entry is attributed and normal completion occurs; immutable selector test covers multi-entry FIFO. | Reject all zero cookies or return `None` after removing their queue entry. |
| `p1_identical_crtc_handles_are_device_qualified` | Two synthetic devices reuse handles. In a setup variant share one sink/recorder across them, then submit ordinary Backend scene work on both. Assert records/queues name actual fds; ready/event on A leaves B's entry and BO pending. Complete B and assert both domains empty. Sharing is fixture configuration, never seeded accepted state. | Remove `device_fd` from the queue key or drain/match B from A readiness. |
| `p1_cookie_on_wrong_crtc_leaves_original_obligation` | Two CRTCs on one device receive distinct accepted scene IDs. Inject A's cookie on B's CRTC; assert both original queue entries, acks and BO phases remain. Complete B and A on their proper CRTCs, then assert full quiescence. | Search all of a device's CRTC queues for an explicit cookie instead of requiring the event's CRTC key. |
| `p1_unmatched_cookie_still_records_clock_without_retirement` | With accepted B pending, inject an unknown explicit cookie with advancing MSC/UST. Assert normal platform clock advances, unmatched diagnostic is observable, no matched-latency sample is added, and B remains; complete B. | Drop event before platform clock recording, or let unmatched identity produce a matched latency/retirement sample. |
| `p1_raw_fd_event_path_preserves_explicit_cookie` | Configure the fixture with socket-backed device plus the live raw-reading event source and recording atomic sink before submission. Encode observed accepted ID into raw bytes and call Backend readiness. Assert same ID is decoded/attributed and work drains. | Lose the cookie in `DrmPageFlipEventSource` after a correct raw decoder. |
| `p1_discarded_stream_old_cookie_cannot_match_new_work` | At fixture construction install master's accepting output-power executor, a recording VtResumeExecutor, and unchanged connector snapshot/probe results. Deliver each outstanding accepted event through its source during the real all-off drain; use Backend `begin_vt_release`, `finish_vt_release(state, input_inventory)`, and `on_vt_acquire`. After old associations and scene lifecycle clear, submit new work; inject a pre-discard ID, assert new ID differs and nothing new retires; complete new work. | Reset allocator during discard or retain old queue associations across the lifecycle boundary. |
| `p1_inactive_drain_filters_unmatched_explicit_cookie` | Enter inactive state through Backend `begin_vt_release` / `finish_vt_release`, with recorded power/master/probe I/O installed at construction. Inject unknown explicit flip plus a valid sequence event. Assert unmatched accounting diagnostic, no primary retirement or new atomic submission, and sequence clock/arm handling remains observable; reacquire and drain ordinary accepted work. Review that the existing cursor-retirement call also lies after the filter. | Omit attribution/filtering in the inactive path, suppressing its unmatched diagnostic, or drop sequence handling with the unmatched flip. |

- [ ] **Step 3: Run focused tests red and implement the P1 port.** Use ordinary focused `drm::atomic::tests`, `drm::page_flip::tests`, and `drm::device::tests` filters, then ignored `commit_identity::atomic_events` with the lavapipe environment. Port reference raw builder, pre-submit allocation, device key and exact selection, raw parser/metadata, and backend filtering. Preserve master `submit` / `submit_tearing` signatures; ID-returning APIs become consumers' choice in Tasks 3/4. Pass matched ID through the event loop as a local value now; old scene/direct API consumers are adapted later. Query capability at real setup, document inherited/render-only exceptions, and replace the stale `receive_events` module description. Enforce ID API preconditions before ioctl and keep existing TEST_ONLY/rejection attribution/legacy telemetry behavior.
- [ ] **Step 4: Review portability and observability, then perform mutations/repetitions.** Verify `IoctlReq` against current libc bindings on Linux glibc/musl and FreeBSD, with target-specific compilation on suitable existing CI runners before merge; local Linux-only gates are not cross-platform proof. Review that the capability helper is actually called from `Device::open`, not just unit tested. Do not port the EBUSY change around reference `atomic.rs:556-563` or `add_cursor_property`. No hardware here. The intended single commit after the gate is `feat(drm): preserve and attribute atomic commit event identities`.
- [ ] **Step 5: Run the per-task gate; commit this task only after all commands pass.**

```sh
cargo +nightly fmt --all
cargo clippy --workspace --all-targets --locked --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 LD_LIBRARY_PATH=/var/lib/flatpak/runtime/org.freedesktop.Platform.GL.default/x86_64/25.08/bcfd828b0c4739753cadb31964f8c1b0f90c8d2bed79bda2c2760438eb48c4b3/files/lib cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
tools/check-layout.sh
```

### Task 3: Bind shared and copied scene retirement to accepted IDs

**Files:**

- Modify: `crates/yserver/src/drm/page_flip.rs` (both scene submitter results).
- Modify: `crates/yserver/src/kms/render/platform/scene_sink.rs` and `crates/yserver/src/kms/render/platform/scanout_bos.rs`.
- Modify: `crates/yserver-render/src/kms/render/scene.rs` and `crates/yserver-render/src/kms/render/scene/{sink,tick,flip}.rs`.
- Modify: `crates/yserver/src/kms/render/backend/kms/scanout.rs` (matched-ID argument for scene retirement).
- Modify: `crates/yserver/src/kms/render/backend/for_tests.rs` (mechanically adapt existing simulation helper's scene call to read and pass its accepted event ID; no new P1 scenario may call this helper).
- Modify: `crates/yserver/src/surface/swapchain.rs` and `crates/yserver/src/surface/platform.rs`.
- Modify/Test: `crates/yserver-render/src/kms/render/scene/tests/tick.rs` and affected existing `OutputSink` test implementations.
- Modify: `crates/yserver-render/src/kms/render/scene/tests/mod.rs` (the existing TestSink's copied-submit return type).
- Modify: `crates/yserver-render/src/kms/render/scene/for_tests.rs` only for separately cfg-gated read-only obligation probes if existing observations are insufficient.
- Create/Test: `crates/yserver/src/kms/render/backend/tests/commit_identity/scene.rs`.
- Modify: `crates/yserver/src/kms/render/backend/tests/commit_identity/{mod,support,recording_scene}.rs`.

**Interfaces:**

- Consumes: Task 2 ID-returning sink variants; matched `Option<u64>` from event attribution; Task 1 fixtures.
- Produces: `submit_scene_flip_with_fences` and `submit_scene_flip_with_fences_tearing` keep master's arguments, including `&[CursorPlanePosition]`, and return `io::Result<u64>`.
- Produces: `submit_shared_scanout_frame` keeps master's inputs and returns `Result<(ComposeSubmit, AsyncOutcome, u64), PresentError>`; the ID belongs to the request that succeeded after any async fallback.
- Produces: `FrameHandoff::FlipPending { submit: ComposeSubmit, event_id: Option<u64> }`; `FrameHandoff::RenderCompletion(Option<OwnedFd>)` remains unchanged. No receipts/content fields are added.
- Produces: `PlatformBackend::submit_copied_scanout(&mut self, output_idx: usize, bo_idx: usize, render_completion: Option<OwnedFd>) -> io::Result<u64>`; `OutputSink::submit_copied(&mut self, output_idx: usize, bo_idx: usize, fd: Option<OwnedFd>) -> io::Result<Option<u64>>` wraps KMS success in `Some`. Surface copied submission stays unsupported with its existing error.
- Produces: `InFlightStage::KmsFlipPending { event_id: Option<u64> }`, `matches_kms_event(self, event_id: Option<u64>) -> bool`, and `kms_retirement_matches(stage: InFlightStage, pending_bo_idx: usize, presented_bo_idx: usize, event_id: Option<u64>) -> bool` with exact optional-ID and BO equality.
- Produces: `SceneCompositor::pending_page_flip_event_id(&self, output_idx: usize) -> Option<u64>` returns identity only in accepted KMS stage, and `handle_page_flip_complete(&mut self, output_idx: usize, event_id: Option<u64>, store: &mut DrawableStore, sink: &mut dyn OutputSink) -> bool` checks identity before touching the sink's pool.
- Produces: `P1SceneRoute` fixture enum with `Shared`, `CopiedSameDevice`, `CopiedSeparateDevice`; `RecordingSceneFixture::with_route(output_count: usize, route: P1SceneRoute) -> Result<Option<Self>, String>`. These are test setup choices, not new production route/recovery types.

- [ ] **Step 1: Write scene contract tests before changing the stage/handoff interfaces.** Extend the existing pure `kms_retirement_requires_the_exact_paired_bo` truth table; replace the reference's direct-internal state progression in copied tests with Backend-driven P1 scenarios. All integration cases use actual recording pools, include deferred resource checks, and run 20x under load.

| Test | Drives and asserts | Mutation that must fail it |
| --- | --- | --- |
| `kms_retirement_requires_the_exact_paired_bo` | Pure stage/BO/ID values: correct BO+ID succeeds; wrong/absent ID, wrong BO, or WaitingForRenderCompletion fails. Include `None`/`None` surface equality and `None`/`Some` mismatch. | Remove ID equality, treat absent ID as wildcard, or ignore paired BO. |
| `p1_shared_scene_checks_identity_before_advancing_bo_pool` | Submit A and retire it, submit B, snapshot BO phases/ack/damage/pool ownership; inject A again and unknown ID, assert every snapshot retained; inject B, assert exact normal retirement and quiescence. | Advance the pool in backend dispatch before the identity gate. Also verify the scene method's pre-sink guard by source review; an integration test alone cannot isolate removal of that redundant guard while backend filtering still masks the event. |
| `p1_copied_frame_gets_identity_only_after_sink_acceptance` | Configure copied route and Task 1 readiness observer at setup. Drive source composition through Backend, withhold delivery of its real completion notification, call the Backend completion hook and assert no accepted event ID. Release notification after actual readiness, script rejected sink KMS acceptance, assert no accepted-ID association/obligation; drive a subsequent normal accepted copied frame and assert its stored ID equals the sink-device record, then complete it. | Install render `job_id` as KMS ID before sink submit, or publish the attempted cookie when sink KMS rejects. |
| `p1_copied_scene_retains_bo_for_old_or_wrong_device_cookie` | Parameterize same-device and separate-device copied routes. After sink acceptance, inject an earlier retired explicit ID and a matching-number event on the render device; assert sink BO/ack retained. Only the sink-device accepted event drains the frame, copy ownership, and pool resources. | Attribute using render fd instead of sink fd, or use zero-cookie FIFO after an explicit mismatch. |
| `p1_shared_scene_async_fallback_awaits_successful_vsync_id` | Reach async through existing Backend Present/composition admission, script async EINVAL then Vsync success. Assert distinct attempted IDs and stored stage equals Vsync's ID; failed async cookie retires nothing, accepted Vsync cookie drains it. | Return or store the first attempted ID after Vsync succeeds. |
| `p1_shared_scene_failed_fallback_creates_no_flip_obligation` | Same normal async entry, but both requests fail. Assert neither attempted cookie creates a sink entry or KMS stage; follow normal failed-submit recycling and an accepted subsequent frame to quiescence. | Enqueue/store attempted ID before checking either request result. |

- [ ] **Step 2: Run focused scene tests red, then port ID/result plumbing.** Use `cargo test -p yserver-render --locked kms_retirement_requires_the_exact_paired_bo -- --nocapture` and ignored `commit_identity::scene` tests under the lavapipe environment. Port only the identity portions of reference `scene_sink.rs:24,706,1305`, `scanout_bos.rs:78,163`, render `scene/sink.rs:71,189`, `scene/tick.rs:1443,1503`, and `scene/flip.rs:17,71,86,323`. Keep master's cursor-transition resolution and copied-submit failures. Do not import acceptance/rejection receipts, primary cursor content fields, or recovery enums to obtain an ID.
- [ ] **Step 3: Check the rejection ordering and non-KMS seam.** Inspect that missing/wrong scene ID returns before `sink.on_page_flip_complete`, not after BO transition. Preserve the later paired-BO check and existing damage handling. Update surface handoff/completion with `None`, and all workspace test sinks atomically with the interface change. Existing scene retirement tests must still exercise BO pairing, not be weakened to cookie-only assertions. Check copied-stage probes remain read-only; test code must not set an accepted ID manually.
- [ ] **Step 4: Run mutations and repetitions, audit skips.** Build the copied-route adapter from master pool/ledger fixtures and the reference's basic route portion only. Skip unsupported SYNC_FD capability once, without calling it a passed copy scenario. Verify each effective-boundary mutation fails its assertion; document the redundant scene-guard source review instead of claiming an unknown-cookie integration case proves every layer independently. The intended single commit after the gate is `fix(kms): retire scene frames by accepted commit identity`.
- [ ] **Step 5: Run the per-task gate; commit this task only after all commands pass.**

```sh
cargo +nightly fmt --all
cargo clippy --workspace --all-targets --locked --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 LD_LIBRARY_PATH=/var/lib/flatpak/runtime/org.freedesktop.Platform.GL.default/x86_64/25.08/bcfd828b0c4739753cadb31964f8c1b0f90c8d2bed79bda2c2760438eb48c4b3/files/lib cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
tools/check-layout.sh
```

### Task 4: Match direct and grouped/degraded unflip acceptance

**Files:**

- Modify: `crates/yserver/src/drm/modeset.rs` (direct/composed scanout result IDs).
- Modify: `crates/yserver/src/kms/render/backend/kms/{state,present,scanout,crtc_config}.rs`.
- Create/Test: `crates/yserver/src/kms/render/backend/tests/commit_identity/direct_unflip.rs`.
- Modify: `crates/yserver/src/kms/render/backend/tests/commit_identity/{mod,support,recording_scene}.rs` (small cursor-independent direct candidate setup).
- Modify/Test: existing direct constructors in `crates/yserver/src/kms/render/backend/tests/mod.rs` and affected assertions in `crates/yserver/src/kms/render/backend/tests/present_scanout.rs` and `tests/tearing.rs`; do not add new P1 scenarios to these large files.

**Interfaces:**

- Consumes: Task 2 accepted IDs and device-qualified event attribution; Task 3 scene event ID accessor and copied acceptance timing; existing Backend direct admission/probe/Present interfaces.
- Produces: `submit_direct_scanout` and `submit_composed_scanout` retain master's complete argument lists and return `io::Result<u64>`; direct retry code preserves the accepted ID through `Result` handling.
- Produces: `DirectPresentFrame.accepted_event_id: Option<u64>`, initialized `None` and assigned only after accepted live direct submission. `submit_direct_frame_live` continues returning `io::Result<AsyncOutcome>` and stores the successful ID into its frame; do not change the submitter seam to bypass production in tests.
- Produces: `ScanoutM2State.unflip_awaiting_outputs: HashMap<usize,u64>` and `unflip_outputs_to_replace: HashSet<usize>`. Grouped acceptance populates both; grouped failure initializes requirements only before degraded work. Cleanup and scheduling membership use the proper map/set without changing submit timing.
- Produces: `record_accepted_degraded_unflip_replacements(&mut self)` records `scene.pending_page_flip_event_id(output)` only for outputs still requiring replacement; call it after shared composition acceptance, after copied sink acceptance in `backend_scanout_on_scanout_render_completion`, and after every scene tick whose completion drain may have accepted a copied sink submission (the already-signalled `None` path in `scene/tick.rs:323-338` → `scene/flip.rs:308-313`, which never reaches the Backend hook).
- Produces: `retire_direct_output(&mut self, output_idx: usize, event_id: Option<u64>, clock: PresentClockSample) -> bool`; it checks accepted ID before output removal/reference-clock capture, and for degraded scene replacements returns control to normal scene retirement after updating the unflip obligation.

- [ ] **Step 1: Add small direct setup to the shared fixture, then write the tests below.** Install framebuffer/probe results and recording scene targets at construction, hide the cursor only through existing Backend cursor visibility API if required for direct admission, and drive the existing eight-frame probation through `Backend::try_present_direct`. Do not set `pending`, `current`, awaiting maps, or probation counters to simulate acceptance. Trigger unflip with an ineligible authoritative-root successor through Backend; use existing copied-route fixture configuration established before work starts. Source/fallback pins and event queues are read-only assertions. All cases wait on real scene/GPU work and therefore repeat 20x.

| Test | Drives and asserts | Mutation that must fail it |
| --- | --- | --- |
| `p1_grouped_direct_awaits_accepted_id_on_every_output` | Two-output direct acceptance uses one nonzero ID and per-output queue entries. Inject wrong/old ID, then correct reference-output event, its duplicate, and final other-output event. Assert no early Complete/Idle, reference clock comes from selected output, exactly one completion after all outputs, and no pending entries. Finish through accepted unflip so source pins also reach zero. | Attribute an explicit miss to queue front, count duplicate output events, or complete on the last output's sample rather than selected reference sample. |
| `p1_direct_async_fallback_uses_only_vsync_acceptance` | Normal direct async entry: EINVAL followed by accepted Vsync. Assert frame ID and all output obligations use only Vsync ID; async attempt cookie changes nothing; accepted events complete once. | Discard the fallback result ID or retain the rejected async attempt's ID. |
| `p1_direct_both_attempts_fail_without_obligation_or_pin_leak` | Async EINVAL then Vsync error through Backend direct entry. Assert no pending/accepted-output state and no leaked source/fallback pins; attempted events cannot satisfy later accepted work. Drain the later accepted cycle. | Assign/retain a direct accepted ID or pins on either failed request. |
| `p1_grouped_unflip_ignores_late_direct_or_duplicate_cookie` | Hold a normally completed direct frame, request/accept grouped composed replacement, and assert map values equal replacement record ID. Inject old direct ID and duplicate output completion; pins and remaining map entries persist. Only all accepted replacement events release direct/fallback pins; drain any resulting ordinary scene frame normally. | Use output membership alone for unflip retirement, or clear every output for the first matching event. |
| `p1_degraded_shared_unflip_waits_for_accepted_replacements` | Reject grouped replacement, then script one per-output shared acceptance and one rejection. Assert requirement set includes both until replaced, map includes accepted output only, rejected/earlier cookies retire nothing. After first valid completion map may be empty while second requirement and direct pins remain. Drive subsequent accepted replacement and complete it; assert both map/set and all pins/scene/sink entries empty. | Populate map from attempted submits or stop direct when accepted-ID map becomes empty while requirements remain. |
| `p1_degraded_copied_unflip_registers_immediately_accepted_replacement` | Copied same-device route whose replacement composition yields an already-signalled completion (`None` sentinel), so the scene tick accepts the sink KMS submit without any Backend render-completion hook call. Reject grouped unflip first. Assert the accepted replacement ID is in the awaiting map right after that `maybe_composite`, an earlier direct cookie releases nothing, and the matching sink event retires scene ack, unflip requirement and direct/fallback pins exactly once. | Register degraded replacements only in `backend_scanout_on_scanout_render_completion` (omit the post-tick registration). |
| `p1_degraded_copied_unflip_has_no_id_before_sink_acceptance` | Configure copied same-device route before initial direct work, with recording direct admission still using production eligibility. Reject grouped unflip; source compose/render readiness alone leaves output requirement and pins held with no accepted replacement ID. Script rejected sink acceptance, assert map remains empty; then drive a normally accepted replacement. Assert its ID is the sink's accepted cookie; old initial direct cookie cannot release it. Correct sink event retires scene+unflip exactly once and leaves no copy/pin/BO obligation. | Mark awaiting at source rendering or rejected KMS attempt, or use render job ID as replacement ID. |

- [ ] **Step 2: Run new scenarios red and port only acceptance/retirement state.** Use ignored `commit_identity::direct_unflip` with the lavapipe environment. Reference P1 hunks: backend `state.rs:248,285,289`, `scanout.rs:885,1437,1462,1484,2486,2496,2551,3171`, `present.rs:901`, and map membership in `crtc_config.rs:42`. Reference tests `round_c_reassign.rs:3444,3833` supply identity/pin assertions only. Keep master's direct-shadow/materialization behavior; do not bring later cursor-content recovery, reassignment, or full fixture setup.
- [ ] **Step 3: Audit every set/map consumer and failure path.** Search `unflip_awaiting_outputs`, direct state construction/cleanup, `pending_pageflip_crtcs`, membership in Present admission and scanout gating, and all `retire_direct_output` callers. Accepted map membership and still-to-replace requirements are different questions. Keep inactive explicit-ID filtering from Task 2; pass the matched ID into both direct and scene retirement. On degraded completion, make sure updating the direct obligation does not swallow the scene's own ack/BO retirement. Preserve master's cursor calls and retry ordering after attributed events.
- [ ] **Step 4: Exercise mutations and repetitions with full end-state assertions.** The test for an empty accepted map with an outstanding requirement must reach that intermediate state through real rejection/completion, not manual map editing. Do not claim isolated direct-guard coverage when backend filtering masks it; test the effective exact-match boundary and inspect the redundant direct guard during review. Report skipped copied cases separately. The intended single commit after the gate is `fix(kms): match direct and unflip retirement to accepted commits`.
- [ ] **Step 5: Run the per-task gate; commit this task only after all commands pass.**

```sh
cargo +nightly fmt --all
cargo clippy --workspace --all-targets --locked --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 LD_LIBRARY_PATH=/var/lib/flatpak/runtime/org.freedesktop.Platform.GL.default/x86_64/25.08/bcfd828b0c4739753cadb31964f8c1b0f90c8d2bed79bda2c2760438eb48c4b3/files/lib cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
tools/check-layout.sh
```

### Task 5: Final controller — coordinator hardware validation and status documentation

**Files:**

- Modify: `docs/status.md` (new behavior-focused P1 entry at the recent-work list).
- Create: `docs/superpowers/plans/2026-10-19-drm-commit-identity-validation.md` during implementation/controller execution, containing the concrete software/repetition/mutation/skip and hardware result matrix. Do not fabricate this result file during planning.
- Read only: this design/plan, `.github/workflows/ci.yml`, `docs/setup.md`, implementation commits and their test output.

**Interfaces:**

- Consumes: four buildable implementation commits; per-test mutation/repetition results; executed/skipped lavapipe cases; glibc/musl/FreeBSD build evidence; the concrete controller matrix below.
- Produces: coordinator-run AMD (card0) and NVIDIA (card1) results, each with kernel/driver/GPU/output topology, build revision, capability result, scenario, observed accept/completion cookie relation, visible behavior, and quiescence evidence. Missing capability coverage or unavailable topology is recorded as unexecuted, not success.
- Produces: final `docs/status.md` P1 behavior entry and linked validation note; intended single commit after the gate is `docs: describe DRM commit identity behavior and validation`.

- [ ] **Step 1: Prepare the reviewable controller packet after software tasks pass.** Include actual build revision, normal startup instructions from `docs/setup.md`, focused new-test names, already captured software/mutation/repeat results, and all fixture skip reasons. Review the diff against base to confirm no cursor/fallback/transition implementation changes beyond the raw-builder type accommodation and upstream event filtering. Confirm the capability setup check is in the real `Device::open` path and portable ioctl types were built on all three required platforms. The implementer does not access `/dev/dri` or run hardware; the coordinator runs Step 2.
- [ ] **Step 2: The coordinator runs this controller matrix on AMD and NVIDIA, unattended (user authorization 2026-10-19).** Before each run, check that no other client holds the card (DRM master, Vulkan/GL users); start yserver from the bench harness `tools/cursor-bench` (master vs branch) on a free display, and drive clients by script (mpv, xdotool) so no human watching is needed. The user may add an optional visual look. Use the existing session/device setup; do not introduce a new behavior env var or change scheduler policy to make a case run. Collect existing debug observations of commit ID/device/CRTC and matched/unmatched events, not a new receipt framework. Hardware need not manufacture stale kernel events: those adversarial cases are covered by the explicit-cookie software injector.

| Coordinator-run case (both vendors) | What to execute/observe | Required result and defect it detects |
| --- | --- | --- |
| KMS setup and ordinary shared scene | Start normal KMS session, expose/repaint windows, capture setup capability and flip logs | Capability is 1; accepted nonzero request cookie is echoed with independent nonzero CRTC; no stuck pending work. Detects cookie omitted from real ioctl or missing CRTC support. |
| Direct Present and return to composition | Exercise an existing fullscreen direct client, then exit/obscure it through the normal desktop flow; repeat on available dual-head topology | Grouped accepted ID completes per CRTC; return to composition waits for replacements; no early Idle, blackout, lost pins, or lingering obligations. Detects membership-only retirement and dropped grouped IDs. |
| Async path and fallback when available | Use an existing qualified vsync-off client, retain normal async admission and driver behavior; record accepted/fallback result | Stored/observed ID belongs to the successful request; no queue entry for failed attempts. Natural fallback unavailable is recorded; scripted software fallback remains its evidence. |
| Copied/degraded path, when available | Run skipped copied/SYNC_FD integration tests on a capable real ICD with recording KMS; exercise normal multi-GPU copied display or degraded unflip only if the topology/driver reaches it naturally | Source readiness creates no accepted KMS obligation; only accepted sink replacement releases held direct resources. Record unexecuted physical scenarios separately; do not force policy or call a recording test a physical scanout test. |
| Lifecycle and cursor regression smoke | Switch VT away/back through normal session controls, repaint, move/show/hide cursor, then close client/session normally | New work after discard uses new IDs; cursor positioning, legacy fallback availability, transition behavior and session liveness match master; no stale entry/pin remains after normal completion. Detects lifecycle association leakage and accidental cursor extraction. |

- [ ] **Step 3: Record the hardware results and close the coverage gaps honestly.** Persist vendor results, failed/skipped/unexecuted cases, and exact software repetition counts in the validation note. The controller is complete only after AMD and NVIDIA results arrive; do not infer hardware approval from silence. Return any actual implementation defect to the owning task, rerun affected tests/gates, and refresh the controller packet. A physical scenario unsupported by available topology remains an explicit validation limit, with its software acceptance/pin case still required to execute on a capable ICD.
- [ ] **Step 4: Write the final status entry as behavior, with actual verification scope.** State that atomic event cookies identify accepted commits; stale/duplicate/unmatched explicit IDs cannot retire scene/direct/unflip resources; copied frames acquire IDs only at sink KMS acceptance; zero-cookie FIFO remains for compatibility; KMS setup requires CRTC-in-event capability and zero-CRTC flips are discarded. Mention no cursor redesign and link the design/validation record. State CI runs ignored tests on lavapipe but fixtures may report capability skips. Describe actual AMD/NVIDIA results and any remaining hardware/topology limits; omit A/B/C branch-split history. This is the final docs task, after code and the separate controller work.
- [ ] **Step 5: Run the per-task gate; commit this documentation task only after all commands pass.** The documentation-only commit does not require a second repetition campaign if source/tests are unchanged; it still runs the required gate. There is no merge in this task.

```sh
cargo +nightly fmt --all
cargo clippy --workspace --all-targets --locked --features xdmcp -- -D warnings
cargo test --workspace --locked --features xdmcp
VK_DRIVER_FILES=/home/ariel_santangelo/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1 LD_LIBRARY_PATH=/var/lib/flatpak/runtime/org.freedesktop.Platform.GL.default/x86_64/25.08/bcfd828b0c4739753cadb31964f8c1b0f90c8d2bed79bda2c2760438eb48c4b3/files/lib cargo test --workspace --locked --features xdmcp --no-fail-fast -- --ignored
tools/check-layout.sh
```

## Coverage table

| Spec rule / constraint | Task and decisive test or review |
| --- | --- |
| R1 allocation before executor, raw cookie, success-only accounting, no TEST_ONLY obligation | T2 `raw_atomic_kernel_request_preserves_full_width_cookie`, `p1_submit_allocates_cookie_before_executor_and_only_enqueues_success`, `atomic_event_identity_policy_excludes_test_only_and_no_event_requests`, `commit_id_allocator_does_not_reuse_after_exhaustion`; ID-API precondition review |
| R2 device-qualified queues and readiness | T1 `p1_fixture_routes_events_only_from_the_ready_fd`; T2 `p1_identical_crtc_handles_are_device_qualified`, `p1_cookie_on_wrong_crtc_leaves_original_obligation`, `p1_discarded_stream_old_cookie_cannot_match_new_work` |
| R3 exact/non-front match, unmatched explicit ID removes nothing | T2 `pending_page_flip_index_is_exact_or_zero_cookie_fifo`, `p1_explicit_cookie_duplicate_and_unknown_retire_nothing`; T4 direct/unflip stale schedules |
| R4 zero-cookie FIFO, absent ID is not a wildcard | T2 `p1_zero_cookie_preserves_fifo_attribution`; T3 `kms_retirement_requires_the_exact_paired_bo`; non-rendering callback/API review |
| R5 clock observation before filtering, no matched samples/retirement, inactive drain | T2 `p1_unmatched_cookie_still_records_clock_without_retirement`, `p1_inactive_drain_filters_unmatched_explicit_cookie`; active/inactive filter source review |
| R6 ABI, ordering, duplicate replacement, portable request type | T2 raw layout/order/kernel-cookie tests; glibc/musl/FreeBSD compile evidence collected by T5 |
| R7 raw full-width flip/sequence data, bounded event walk, actual fd read | T2 raw parser table, `drain_events_delivers_the_commit_identity_of_a_flip_complete`, `p1_raw_fd_event_path_preserves_explicit_cookie` |
| R8 setup capability, Linux 4.12 fact, zero-CRTC drop | T2 capability-result and inverted zero-CRTC tests; actual Device::open wiring review; T5 AMD/NVIDIA setup observations; spec provenance |
| R9 shared accepted identity checked before sink advance; paired BO; non-KMS None | T3 `p1_shared_scene_checks_identity_before_advancing_bo_pool`, exact paired-BO truth table, pre-sink guard review, surface regression suite |
| R10 copied gets ID only at sink acceptance | T3 copied acceptance and wrong-device tests; T4 `p1_degraded_copied_unflip_has_no_id_before_sink_acceptance` |
| R11 grouped direct reference clock and all-output unflip/pin retirement | T4 `p1_grouped_direct_awaits_accepted_id_on_every_output`, `p1_grouped_unflip_ignores_late_direct_or_duplicate_cookie` |
| R12 successful fallback ID, failed attempts absent, accepted-only degraded map plus requirement set | T3 shared fallback pair; T4 direct fallback pair and both degraded-unflip tests |
| R13 lifecycle associations/cleanup are correctness state | T2 discard/inactive tests, T4 membership/cleanup audit, every stateful test's quiescence checks |
| A end-state/leak checks | T1 shared `assert_quiescent`; all T2/T3/T4 lifecycle scenarios and T5 normal-session completion observations |
| B Backend/core-only lifecycle advancement; master non-test callers | Caller audit table; T1 helpers and every scenario driver; T3/T4 explicit ban on reference fake-retirement helpers |
| C coordinator-run AMD + NVIDIA (user-authorized, unattended), separate final controller | T5 controller matrix and result note; no claimed ioctl coverage from recorder tests |
| F 5x/20x under 16 busy loops, one-line skips, CI ignored-suite facts | Verification rules; each task's mutation/repeat step; T5 execution/skip accounting |
| Shared helpers first; no large reference fixtures | T1 `AtomicEventFixture`, `queue_commit_completion`, `accepted_event_id`, optional recording scene adapter |
| No cursor changes/later concerns, no legacy removal, no scheduling/recovery | Global Constraints; T2 exclusion and portability review; T3 minimal handoff; T4 source/membership review; T5 diff audit and cursor smoke |
| No production cfg(test) branches or new behavior env vars; layout/dependency ceilings | Every task gate and source review; only existing software-Vulkan opt-in appears in prescribed test environment |
| Final behavior-focused status documentation | T5 `docs/status.md` entry plus actual validation record |

## Open questions and handoff

No design decision remains open: require CRTC-in-event capability at setup, drop zero-CRTC flips without cookie reinterpretation, and retain zero-cookie FIFO compatibility. Implementation validation must establish which copied/SYNC_FD cases execute under the supplied lavapipe runtime and which require a capable ICD; AMD/NVIDIA topology availability and cross-platform compile evidence are controller inputs, not silently assumed results.

This plan has five tasks and one commit per task. It contains no implementation results. Execution awaits a later instruction that expands the present documentation-only write scope; no permission or execution-method question is needed to finish the requested planning deliverables.
