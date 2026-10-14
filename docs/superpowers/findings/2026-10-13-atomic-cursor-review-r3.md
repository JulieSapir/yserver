## Verdict

**0 blocking, 2 major, 0 minor**

Coverage: COMPLETE FOR DECLARED SCOPE

This is a design-review result, not implementation approval or evidence that code compiles or tests pass.

## Incorporation audit

| Prior finding | Status | Assessment |
|---|---|---|
| B-1: grouped cursor positions | APPLIED | Every participating output supplies its position; successful submissions clear only carried positions. Two-CRTC direct/unflip tests cover coordinates ([plan L134–147](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:134)). |
| M-1: direct Present continuation | APPLIED | Task 3 distinguishes first entry from an existing direct successor and resumes retained successors on cursor retirement ([L170–178](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:170)). General scene arbitration remains a separate gap below. |
| M-2: rejected-submit gate | APPLIED | Only accepted commits become in flight. The EBUSY test requires scene progress without a synthetic retirement ([L57–62](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:57), [L212–217](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:212)). |
| B-2: queued first-entry cancellation | APPLIED | First entry follows composed fallback instead of creating queued-only direct state. Its test checks released pins, composed completion and subsequent entry ([L171–174](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:171), [L201–206](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:201)). |
| M-3: permanent rejection recovery | PARTIAL | Hiding before software ownership and suppressing rejected cursor fields address scene poisoning. Failed-hide recovery has no specified producer or bounded outcome; see M-3 below. |
| M-4: visible cursor latency | APPLIED | Event timestamps travel with positions through retirement; acceptance and visibility are separate metrics. The claim and AMD limit now allow two frames ([L108–113](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:108), [L269–273](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:269)). |

## Findings

### Blocking

None.

### Major

**M-3 — Failed hide rollback can leave position updates disabled indefinitely**

The rejection contract retains hardware ownership when hiding fails, drops the unsent position and suppresses cursor fields until a successful show ([plan L63–69](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:63)). Its failed-hide test proves retained ownership and clean scene requests, but specifies no recovery transition ([L207–211](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:207)).

Concrete sequence:

1. A visible hardware cursor’s position commit returns EINVAL.
2. Legacy hide rollback fails.
3. The plane remains visible and eligible for hardware ownership; cursor fields are quarantined.
4. Further pointer motion cannot submit positions under that quarantine.
5. With unchanged cursor image and visibility, ordinary scene frames need not show the plane again: the existing `Hw → Hw` transition produces no show ([scene/cursor.rs L229–291](/home/ariel_santangelo/Projects/yserver-atomic-cursor/crates/yserver-render/src/kms/render/scene/cursor.rs:229)).

Presentation can recover while the visible cursor remains frozen indefinitely. “Until the next successful show” names a clearing condition without assigning anyone to produce it. Existing recovery explicitly uses an outcome to wake the scene and request direct unflip when necessary ([scanout.rs L267–278](/home/ariel_santangelo/Projects/yserver-atomic-cursor/crates/yserver/src/kms/render/backend/kms/scanout.rs:267)); that handoff is absent from this branch.

**Required correction:** retain actual hardware ownership while recording a separate recovery obligation. Assign its wakeup/unflip owner and a bounded hide-retry or fail-stop policy. Test an initial failed hide followed by recovery, including resumed cursor motion. The review log’s unconditional “applied” claim overstates this correction ([plan L303–308](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:303)).

**M-5 — Immediate motion submission can bypass the promised scene priority**

Task 3 permits motion to submit immediately whenever its enumerated blockers are absent; a ready scene frame is not among them ([plan L161–163](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:161)). Separately, retirement arbitration promises direct work, then ready scene work, then cursor-only work ([L166–169](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:166)). No contract makes the motion entry point participate in that priority decision.

The current loop dispatches DRM retirement and input during its event walk ([run.rs L2315–2345](/home/ariel_santangelo/Projects/yserver-atomic-cursor/crates/yserver-core/src/core_loop/run.rs:2315), [L2546–2559](/home/ariel_santangelo/Projects/yserver-atomic-cursor/crates/yserver-core/src/core_loop/run.rs:2546)); composition runs later in the iteration tail ([L3064–3090](/home/ariel_santangelo/Projects/yserver-atomic-cursor/crates/yserver-core/src/core_loop/run.rs:3064)).

An allowed ordering is therefore:

1. Scene damage waits behind a cursor-only commit.
2. DRM retirement clears that commit.
3. Another motion callback runs before the composition pass and immediately occupies the clear CRTC.
4. The new scene gate blocks composition again.

Repeated ordering under sustained motion can starve ready scene work. The specified scene-gate test does not require intervening motion between retirement and scene scheduling ([plan L191–192](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:191)).

**Required correction:** assign one arbitration boundary that motion cannot bypass—either queue motion for the common scheduling pass, or check and reserve ready scene work before immediate cursor submission. Add a retirement → motion → scheduling test with pre-existing scene damage that proves the scene receives the next commit.

### Minor

None.

## Coverage and implementation checks

1. **Incorporation:** assessed all six findings represented in the prior review. First-entry cancellation is corrected; failed-hide recovery remains partial.
2. **Architecture:** checked the scene/sink gate, direct admission and successor continuation, retirement callbacks and actual dispatch-before-compose ordering. Grouped and copied-output routing was assessed at the plan-contract level.
3. **Safety and failure semantics:** checked accepted-only bookkeeping, device-qualified retirement, topology/VT draining and ownership-preserving rollback. Existing old-event drain failures terminate progression before topology reuse ([crtc_config.rs L91–100](/home/ariel_santangelo/Projects/yserver-atomic-cursor/crates/yserver/src/kms/render/backend/kms/crtc_config.rs:91)). Direct ShowCursor already has a bind-or-unflip path; it was verified.
4. **Specification and verification:** assessed commit-stream, rejection, topology, hidden-cursor and latency requirements. The plan assigns formatting, all-target clippy, tests, stress and hardware validation to implementation ([plan L253–293](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:253)). CI supplies linked builds and FreeBSD checks. The two findings require additional failure/ordering evidence.

**Excerpts used: 24/24**, each at most 120 lines; plan and prior review read once. Investigation stopped at the budget. No builds, tests, benchmarks, installs or compilation experiments ran.

External spike artifacts, hardware scripts, physical driver behavior, underlying framebuffer/fence and ABI internals, and musl buildability were not independently assessed and are not declared sound. Exact Rust APIs, fixtures, test outcomes and portability remain checks for implementation.