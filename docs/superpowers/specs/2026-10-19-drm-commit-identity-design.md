# DRM commit identity (P1)

Date: 2026-10-19. Base: `joske/master` `cb8297d02d7b55451c9bdc635761b3490f4d0b17`, branch `feat/drm-commit-identity`.

This is the first independent extraction from the read-only reference worktree `/home/ariel_santangelo/Projects/yserver-cursor`, HEAD `9023da148884acfd5d373bd5416a396147c6ecf2`. The inventory at `/home/ariel_santangelo/Projects/claude-scratch/atomic-cursor-split/inventory.md` is a navigation aid; the source at those revisions is authoritative. Implementation is described in [the plan](../plans/2026-10-19-drm-commit-identity.md).

## Problem and current behavior

Master already has `AtomicCommitRequest`, an injectable `AtomicCommitSink`, and diagnostic commit IDs. `CommitWindow::record_commit` allocates an ID **after** a successful executor call and puts an entry into a queue indexed only by raw CRTC handle. `record_page_flip_event` removes the oldest entry. The ID never reaches the kernel: the locked `drm` 0.15.0 / `drm-ffi` 0.9.1 atomic submission interface leaves `drm_mode_atomic.user_data` at its default zero. Its page-flip event abstraction also drops the independent cookie.

The backend currently attributes a completion by output/CRTC membership. Scene retirement checks the pending KMS stage and paired BO, but calls the sink's BO-pool advancement before checking the scene acknowledgment. Direct Present and composed unflip similarly retire output obligations without an accepted submission ID. An unrelated, duplicate, or delayed completion on the same CRTC can therefore consume the wrong accounting entry, advance the wrong scene pool, or release direct-frame pins before their replacement was accepted and displayed. Identical raw CRTC handles on two devices also need separate accounting domains.

P1 carries identity from atomic acceptance to resource retirement. It must be usable on master by itself, including master's position-only atomic cursor path, legacy `set_cursor2` / `move_cursor` fallback, and cursor transition bookkeeping.

## Global constraints

- P1 changes DRM commit identity and primary-frame retirement only; master's cursor positions, legacy set_cursor2/move_cursor fallback, and cursor transition state remain intact.
- Do not introduce cursor property tagging, the reference EBUSY attribution change, legacy telemetry removal, KmsServices, cursor receipts, sprite pools, or recovery enums.
- Preserve Linux glibc, Linux musl, and FreeBSD ioctl request typing and buildability; do not assume libc::Ioctl exists on every target.
- Require DRM_CAP_CRTC_IN_VBLANK_EVENT == 1 on each real KMS device at setup; Linux >= 4.12 always advertises this capability.
- No new YSERVER_* behavior environment variables and no cfg(test) branches inside production function bodies.
- Respect tools/check-layout.sh: code files at most 5,000 lines and test files at most 6,000 lines, plus its dependency and platform seams.
- Keep yserver-render independent of drm, gbm, input, udev, and yserver-core; shared scene interfaces use Option<u64> for event identity.
- Do not change scheduling, admission, retry timing, async qualification, or cursor rejection policy.

## Identity and acceptance rules

**R1 — Allocate before submission, accept after success.** Each event-producing sink request receives a nonzero `u64` commit ID before the executor sees it. The raw `DRM_IOCTL_MODE_ATOMIC` request carries exactly that value as `user_data`. An ioctl failure consumes the allocated number but creates no pending entry or retirement obligation. No-event and `TEST_ONLY` requests create no completion association. Keep the existing unit-returning sink APIs; add ID-returning variants for primary producers. Validate the ID-returning variants' event-producing preconditions before submitting, so an accepted ioctl cannot subsequently be reported as a local validation failure.

IDs are unique within the sink's lifetime and are not reset by a telemetry-window reset or lifecycle queue discard. Grouped requests use one ID for all their target CRTCs. Do not port the reference's saturating counter literally: exhaustion must fail before submission instead of reusing an ID. This is an allocation error, not a new recovery state machine.

**R2 — Device-qualified association.** Key pending queues by `(device_fd, crtc::Handle)` within the owning sink. The DRM fd used for acceptance and the fd whose readiness is being drained must identify the same live device. Record one entry per target CRTC for a successful grouped request. Keep existing all-off/event-stream-discard clearing in place before a device/route is retired; do not retain associations across fd reuse. Raw DRM object handles are not globally unique.

**R3 — Explicit matching has no FIFO escape.** A nonzero event cookie selects only the entry with that ID in that device/CRTC queue, even if it is beyond the front. Remove only that entry; return its matched ID to retirement consumers. Unknown, rejected-attempt, duplicate, already-retired, wrong-device, and wrong-CRTC explicit IDs remove nothing. An empty queue and a nonmatching nonempty queue have the same no-retirement result.

**R4 — Preserve zero-cookie compatibility.** Normalize an incoming zero cookie to absent identity. It preserves the existing oldest-entry attribution on the device/CRTC queue, returning that entry's ID when one exists. Remaining non-rendering submitters still use the crate's zero-cookie path and existing API. Zero-cookie compatibility cannot distinguish stale events and does not acquire P1's explicit-ID guarantee. Never use the FIFO fallback for a nonzero cookie, and never treat absent identity as a wildcard against `Some(expected_id)` in scene/direct retirement.

**R5 — Observable events, filtered retirement.** For a valid, routed CRTC event, platform MSC/UST and receipt/completion timestamp recording still occur before backend identity filtering. Unmatched explicit cookies remain visible through that existing clock observation and an unmatched-event diagnostic identifying device, CRTC, and cookie. They create no matched latency sample and retire no queue entry, scene acknowledgment, direct/unflip obligation, source pin, or existing cursor-position completion state. Apply the explicit-ID filter in both active and inactive-seat event drains. Preserve sequence handling and existing post-drain scheduling; the new filter grants no new submit opportunity.

## Raw submission and event boundary

**R6 — Kernel-shaped atomic request.** Replace only the sink's underlying atomic builder/submission with a small raw builder. Preserve object and property ordering, per-object property counts, duplicate-property replacement, signed position values, fence ownership, flags, and error propagation. The kernel ABI is a 56-byte `drm_mode_atomic`; reserved data is zero and pointers refer to live request arrays for the ioctl's duration. Use the existing portable ioctl-request typing pattern with the atomic request's ABI/number verified on supported targets. Keep master's `CursorPlanePosition` and its current property attribution; its builder-parameter change is mechanical.

**R7 — Parse the event cookie independently.** Read and walk the raw DRM event byte stream rather than round-tripping through the crate's cookie-losing `PageFlipEvent`. A flip callback receives `(crtc, sequence, duration, user_data: u64)`; carry the cookie through `KernelPageFlip` and `FlipEventTimestamps` without truncation. Preserve the complete sequence-event cookie, signed timestamp and `u64` sequence independently of flip identity. Empty reads, unknown event types, short payloads, incomplete headers, invalid lengths, and records overrunning the read must not cause an out-of-bounds read, infinite loop, or fabricated completion. Walk every complete record in a batch; stop at an unwalkable length. Preserve the existing poll-before-read behavior and error contract. The non-rendering drain callbacks accept and ignore the added cookie argument.

**R8 — Require a real CRTC, never decode one from a commit ID.** Query `drm::DriverCapability::CRTCInVBlankEvent` (`DRM_CAP_CRTC_IN_VBLANK_EVENT`) once while opening each real KMS device, before returning it to callers that can submit event-producing commits. A value other than 1 or an unsuccessful query rejects setup with a contextual error naming the device and capability. This is a driver-capability query, not a new `SET_CLIENT_CAP` opt-in. Keep the existing UniversalPlanes/Atomic negotiation policy. Render-node-only devices are exempt. An inherited duplicate KMS fd relies on the parent device's completed setup and does not repeat negotiation; synthetic test devices install declared capabilities through fixture construction.

Linux documents this capability as always set since 4.12. FreeBSD must report the same required capability; do not infer it from a Linux version check. P1 offers no older-kernel cookie-only routing mode: one grouped commit cookie can name several CRTCs.

A `DRM_EVENT_FLIP_COMPLETE` with `crtc_id == 0` is unroutable even when its cookie's low 32 bits equal an existing CRTC. Log the reason and original cookie, then drop that record before the normal flip callback, clock routing, or retirement. Continue walking other valid records in the batch. Never cast or reinterpret `user_data` as a CRTC, including when the cookie is zero. Rename and invert the reference test `dispatch_falls_back_to_user_data_crtc_when_crtc_id_is_zero`. The existing CRTC-sequence cookie encoding is a separate protocol and remains intact.

## Producer and retirement rules

**R9 — Shared scene acceptance and BO ordering.** A successful shared scene submit propagates its accepted ID through `FrameHandoff::FlipPending` into `InFlightStage::KmsFlipPending { event_id: Some(id) }`. Retirement requires that identity and the existing paired-BO check. Check the pending stage and event identity **before** `OutputSink::on_page_flip_complete` advances the sink's BO pool. A wrong or missing ID leaves BO phases, damage acknowledgments, descriptor-pool ownership, and pending scene state untouched. The later paired-BO check remains; P1 does not redesign the pre-existing wrong-BO failure handling.

The platform-neutral shared APIs carry `Option<u64>`: KMS acceptance is `Some(nonzero_id)`, surface acceptance is `None`, and a surface completion passes `None`. Equality is exact, so `None` is compatible only with an identity-less pending frame. Do not add a cursor receipt just to transport an event ID.

**R10 — Copied render readiness is not KMS acceptance.** A copied frame starts at `WaitingForRenderCompletion { job_id }`, which has no accepted KMS ID. The render job ID is a separate namespace. Through `Backend::on_scanout_render_completion`, the sink copy and KMS submission return the accepted event ID; only successful sink KMS acceptance changes the pending stage to `KmsFlipPending`. A render completion, rejected sink submit, or earlier event cannot install an ID or retire the copied frame. Apply the same identity-before-BO-advance rule on copied sinks, including separate render/KMS devices. Keep master's copied-submit error paths, rather than porting recovery enums/retry machinery.

**R11 — Direct and grouped unflip obligations.** `DirectPresentFrame.accepted_event_id: Option<u64>` is set only from the successful grouped direct submission. An event must match that ID and an awaiting output before removing the output, recording the reference completion clock, or publishing completion/idle. All grouped outputs must retire; a duplicate on one output cannot satisfy another. Preserve master's chosen reference-output clock and direct-frame ownership policy.

For accepted grouped composed unflip, retain an output-to-accepted-ID map. A matching event satisfies only its output's accepted replacement. Keep the held direct frame and its source/fallback pins until every output that still scans it has been replaced. An unrelated completion must not release those pins.

**R12 — Fallback and degraded acceptance.** When an async attempt is rejected and Vsync succeeds, store and await the Vsync request's ID. Failed-attempt IDs never create queue entries. Preserve existing async/EBUSY/EINVAL policy and diagnostics while changing the result plumbing for both shared scene and direct submission.

When grouped unflip fails and master degrades to per-output scene replacement, distinguish `unflip_outputs_to_replace: HashSet<usize>` from `unflip_awaiting_outputs: HashMap<usize, u64>`. The set retains every output still needing replacement, including outputs without any accepted KMS replacement. The map contains only accepted replacement IDs. Shared replacement IDs become eligible immediately after acceptance; copied replacements become eligible only after sink KMS acceptance. Sink KMS acceptance of a copied frame has two production paths and both register the replacement: the Backend render-completion handler, and the scene tick's own completion drain, which submits an already-signalled completion (the reverse-PRIME `None` sentinel, `specs/2026-07-14-reverse-prime-copy-design.md`) without invoking the Backend hook. Failed or not-yet-submitted replacements add no map entry. A matching accepted replacement removes its map entry and output requirement. Release held direct ownership only when the requirement set is empty; an empty accepted-ID map alone is insufficient. A degraded scene completion must still retire its scene acknowledgment and BO exactly once.

**R13 — Lifecycle and accounting remain correctness state.** Treat pending flip associations as required lifecycle state even though the existing methods and container are named for telemetry. Retain existing all-off discard boundaries and adapt set/map membership and cleanup in Present, scanout, and CRTC configuration. Normal completion tests must first drain the explicit accepted events and prove quiescence; destroying a fixture or clearing a map is not evidence of successful retirement. This extraction adds no recovery, topology reassignment, or power-management policy.

## Non-goals

- No cursor changes: no full-state cursor updates, content/position redesign, changed fallback, cursor transition rewrite, or cursor rejection-policy change.
- No sprite pool, sprite leases, cursor-frame receipts, or cursor property tagging.
- No legacy removal, including legacy submission paths or latency telemetry.
- No scheduling changes, new batching/admission rules, vblank gates, or retry timing changes.
- No recovery implementation, recovery enums, `KmsServices`, qualification/reassignment machinery, or new hardware policy beyond the required CRTC-event capability.
- No import of the reference's large `atomic_cursor_only/round_c_reassign` fixtures; reuse only its P1 assertions and small infrastructure pieces.

## Test requirements and acceptance

**A — End state.** Every stateful scenario asserts both intermediate retention and a final normal-completion state: no pending sink flip entry on any tested device/CRTC, no scene acknowledgment/render job/copy obligation, no direct pending/successor/unflip requirement or accepted-ID obligation, no held source/fallback pin, and no unreleased deferred pool resource after GPU completion. An OnScreen baseline BO is permitted; an outstanding Pending BO is not. Do not erase state to obtain these assertions. A fixture skip happens before any obligations are acquired.

**B — Production drivers.** Lifecycle tests initialize resources and inject external observations, then advance only through `Backend` trait entries or `core_loop::run_core`, using the recording atomic executor and device-qualified explicit-cookie event injector. In particular use `maybe_composite`, `try_present_direct`, `on_scanout_render_completion`, `on_page_flip_ready`, and normal completion drains. Never call a retirement/submit internal, simulate a completion by a scene helper, insert a pending acknowledgment/accepted-ID obligation, or replace the live direct submitter to manufacture progress. Stateless ABI, byte-decoder, capability-result policy, and immutable queue-selection tests may inspect boundary values; they do not advance lifecycle state or substitute for integration coverage.

Build shared helpers first: a small cursor-independent `AtomicEventFixture`, `queue_commit_completion`, and `accepted_event_id`. Put the optional recording scene/BO adapter next to it, without coupling the base helper to Vulkan or cursor state. Reuse master recording-pool setup where appropriate. Tests should show the event schedule and assertions, with setup centralized.

Required coverage includes raw request ordering/layout/cookie serialization; success/failure/TEST_ONLY accounting; exact and non-front selection; unmatched/duplicate/stale/wrong-device/wrong-CRTC IDs; zero-cookie FIFO; full-width flip and sequence cookies; malformed event streams; zero-CRTC rejection; capability acceptance/error policy; clock observation without retirement; shared and copied scene retention before BO advancement; grouped direct reference-clock completion; async fallback IDs; grouped and degraded shared/copied unflip acceptance and pins. Each planned test states the production mutation that must make it fail. Verify the relied-on path has a non-test caller on master before adding a scenario.

**C — Hardware controller.** Hardware validation is a separate final controller task, run by the coordinator on AMD and NVIDIA (the user authorized unattended runs on 2026-10-19; GPU clients are checked before each run). It verifies the real ioctl/event contract and driver differences; the recording executor and ABI tests cannot claim to execute the atomic ioctl. Keep software gates and hardware results distinct. No hardware is accessed while writing these documents.

**F — Repetition and skips.** Run every new test five times under 16 concurrent CPU busy loops; run GPU-waiting tests twenty times under the same load. Report executed and skipped cases separately. Every fixture skip prints exactly one line naming its reason, emitted by the deciding constructor and propagated without duplicate output. Unexpected setup errors fail. `.github/workflows/ci.yml` already invokes ignored workspace tests on its xdmcp lavapipe leg; being invoked does not prove a capability-dependent test executed. Do not call skipped copied/SYNC_FD cases covered by lavapipe.

Each implementation task runs formatting, full-feature workspace clippy/tests, the specified lavapipe ignored suite, and `tools/check-layout.sh` using the plan's exact commands. Preserve relevant existing cursor and non-KMS regressions. The final `docs/status.md` entry describes the resulting P1 behavior and its verified limits, with no A/B/C split-history narrative.

## Provenance

Sources consulted, by static read only:

- Worktree `AGENTS.md`, `README.md`, `docs/status.md`, `tools/check-layout.sh`, and `.github/workflows/ci.yml` at master `cb8297d0`.
- Inventory sections 1, 3, and 4, verified against reference HEAD `9023da14`: `drm/atomic.rs`, `drm/page_flip.rs`, `drm/modeset.rs`, platform event/scene/copy submission, scene sink/stages/retirement, backend direct/unflip state and event handling, and the P1 assertions in `round_c_reassign.rs`. Reference line numbers are revision-specific; it also contains the zero-CRTC defect and unrelated later-concern code.
- Corresponding master production paths, including `device.rs:88`, `atomic.rs:854,963`, `page_flip.rs:183,327`, `platform/scene_sink.rs:143,556`, `platform/scanout_bos.rs:61`, `backend/kms/scanout.rs:532,1041,2093,2126`, `backend/kms/present.rs:780`, render `scene/flip.rs:77,254`, surface handoff/completion, and the `core_loop/run.rs:2338,2376,3104` callers.
- Kernel DRM UAPI headers `/usr/src/linux-7.2.8-gentoo-dist/include/uapi/drm/drm.h` (capability documentation near line 768; flip layout near 1412) and `drm_mode.h` (atomic layout near 1332), cross-checked with installed `/usr/include/drm/drm.h` and `drm_mode.h`. These establish capability 0x12, the Linux 4.12 guarantee, independent cookie/CRTC fields, and atomic ABI layout.
- Locked dependency source under `/home/ariel_santangelo/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`: `drm-0.15.0/src/control/mod.rs:768,994,1159`, `drm-0.15.0/src/lib.rs:156,305`, `drm-0.15.0/src/control/atomic.rs`, and `drm-ffi-0.9.1/src/mode.rs:758`. These establish the missing atomic cookie parameter, zero default, private builder arrays, and cookie-losing page-flip parsing, including the obsolete fallback.

This document specifies behavior in original prose. It contains no copyleft code excerpts or copied kernel implementation; ABI names, field sizes, and constants identify the public interface.
