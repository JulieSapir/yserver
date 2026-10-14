## Verdict

**1 blocking, 2 major, 0 minor**

Coverage: COMPLETE FOR DECLARED SCOPE

This is a design-review result, not an implementation approval or a claim that compilation or tests pass.

## Incorporation audit

| Prior finding | Status | Assessment |
|---|---|---|
| B-1: grouped cursor positions | APPLIED | Task 2 supplies every participating output’s position, clears only positions carried by successful commits, and requires two-CRTC coordinate tests ([plan L118–134](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:118)). |
| M-1: direct Present continuation | TRADED | Task 3 adds retention and cursor-retirement resumption. Reusing the successor slot also creates a queued-only direct state whose cancellation lacks a retirement owner; see B-2 ([plan L154–180](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:154)). |
| M-2: rejected-submit gate | APPLIED | Accepted-only in-flight bookkeeping and an EBUSY test explicitly require subsequent scene progress without a synthetic event ([plan L56–58](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:56), [L182–187](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:182)). Permanent-error recovery remains a separate gap. |

## Findings

### Blocking

**B-2 — A queued first direct Present has no cancellation retirement owner**

Task 3 queues an eligible Present behind a cursor commit whenever no direct frame is pending, including initial direct entry with neither `pending` nor `current` present ([plan L155–159](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:155)). That queue alone makes direct state active ([backend.rs L504–505](/home/ariel_santangelo/Projects/yserver-atomic-cursor/crates/yserver/src/kms/render/backend.rs:504)).

Concrete sequence:

1. Composed scanout is current; a cursor-only commit starts.
2. The first admitted direct Present enters the successor slot.
3. A root overlay appears before cursor retirement, requesting an unflip.
4. Cursor retirement invokes the existing successor helper, which returns while `unflip_requested` is set and retains the candidate ([scanout.rs L539–545](/home/ariel_santangelo/Projects/yserver-atomic-cursor/crates/yserver/src/kms/render/backend/kms/scanout.rs:539)).
5. Composed replacement is scheduled only when `current.is_some()`, which is false here ([L2056–2080](/home/ariel_santangelo/Projects/yserver-atomic-cursor/crates/yserver/src/kms/render/backend/kms/scanout.rs:2056)). Ordinary scene retirements have neither a pending direct frame nor an unflip transaction to retire ([L879–907](/home/ariel_santangelo/Projects/yserver-atomic-cursor/crates/yserver/src/kms/render/backend/kms/scanout.rs:879)).

The candidate’s completion and pins remain retained, and the unflip flag prevents direct re-entry. This violates the promised waiting-Present continuation. The review log’s unconditional “all three … applied” claim omits this ownership consequence ([plan L264–269](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:264)).

**Required correction:** define cancellation for queued direct work when no direct framebuffer owns scanout. Release its never-submitted pins, publish appropriately ordered Skip completion, and clear entry state without requiring a replacement flip. Test initial entry invalidated before cursor retirement, including completion, pin release and subsequent presentation progress.

### Major

**M-3 — Permanent cursor rejection can poison subsequent scene commits**

The rejection contract retains the unsent position and permits the next scene commit to carry it ([plan L56–58](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:56)). Task 2 appends visible atomic cursor positions to scene requests; failure clears nothing ([L118–125](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:118)). No permanent-error recovery is specified, although the recorder supports EINVAL ([L89–92](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:89)).

Concrete failure: a persistently invalid cursor state produces EINVAL on a cursor-only request. The next otherwise valid scene request carries that same state and is rejected too. Existing scene recovery preserves repaint and schedules another attempt after 100 ms ([tick.rs L1368–1387](/home/ariel_santangelo/Projects/yserver-atomic-cursor/crates/yserver-render/src/kms/render/scene/tick.rs:1368)); without a cursor recovery transition, every attempt remains poisoned.

Master’s legacy path explicitly handles EINVAL and disabling errors through hide rollback before software fallback ([cursor.rs L619–638](/home/ariel_santangelo/Projects/yserver-atomic-cursor/crates/yserver/src/kms/render/platform/cursor.rs:619)). The new contract does not preserve or replace that handoff.

**Required correction:** specify bounded recovery or controlled shutdown for permanent rejection. A fallback must preserve actual hardware ownership until hiding succeeds, then permit presentation without the rejected cursor fields. Add a scripted EINVAL case proving that handoff and its failure outcome.

**M-4 — Motion-to-acceptance does not establish visible cursor latency**

The plan claims at most one vblank of added latency and requires that delay to remain within one vblank ([plan L32–34](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:32), [L73–74](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:73)). Instrumentation instead ends at ioctl acceptance, and AMD acceptance checks that metric’s p99 ([L95–97](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:95), [L239–240](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:239)).

Concrete ordering: motion arrives shortly after a scene commit starts. The single-commit rule holds its position until that commit retires at the next vblank. The position is then accepted in another commit and becomes visible at the following vblank. Acceptance delay can be below one frame while visible delay approaches two frames. A final motion followed by stopping exposes this without coalescing ambiguity.

**Required correction:** correlate carried positions with their retirement timestamps and include motion during an existing commit in latency validation. State acceptance delay and visible delay separately, and resolve the one-vblank claim against that evidence. This finding does not reconsider the accepted choice to use this model on AMD/i915.

### Minor

None.

## Coverage and implementation checks

1. **Incorporation:** assessed every prior finding against actual task text. Successful-path corrections are present; successor-slot reuse introduces B-2.
2. **Architecture:** verified scene readiness/submission gates, direct queue ownership, retirement routing and composed replacement scheduling. Grouped and copied-output cursor routing was assessed at the plan-contract level.
3. **Safety and failure semantics:** checked device-qualified event correlation, bounded old-event draining, VT ordering and accepted-only bookkeeping. Cancellation and permanent rejection remain unresolved. Underlying framebuffer/fence implementation and ABI layouts were not independently audited.
4. **Specification and verification:** checked the normative commit-stream, topology, direct-entry, rejection and latency requirements. Formatting, clippy, tests, stress checks and hardware validation are assigned to implementation ([plan L250–260](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:250)); CI supplies linked builds and FreeBSD checks ([CI L55–68](/home/ariel_santangelo/Projects/yserver-atomic-cursor/.github/workflows/ci.yml:55), [L157–159](/home/ariel_santangelo/Projects/yserver-atomic-cursor/.github/workflows/ci.yml:157)). VT restoration has manual hardware coverage; no deterministic cursor-in-flight VT test is specified.

**Excerpts used: 23/24**, each at most 120 lines; plan and prior review read once. No builds, tests, benchmarks, installs or compilation experiments ran. External spike artifacts, hardware scripts, physical driver behavior and musl buildability were not verified and are not declared sound. Rust/API correctness, actual test outcomes, portability and hardware acceptance remain implementation checks.