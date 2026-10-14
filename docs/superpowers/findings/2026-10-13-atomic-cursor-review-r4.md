## Verdict

**0 blocking, 3 major, 0 minor**

Coverage: COMPLETE FOR DECLARED SCOPE

This is a bounded design-review result, not implementation approval or evidence that code compiles or tests pass.

## Incorporation audit

| Prior finding | Status | Assessment |
|---|---|---|
| B-1: grouped cursor positions | APPLIED | Task 2 requires positions for every participating output and two-CRTC direct/unflip tests with distinct local coordinates ([plan L141–156](docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md#L141)). |
| M-1: direct Present continuation | APPLIED | Existing direct successors are retained and resumed after the last blocking cursor retirement; unflip retains its Skip ownership ([L180–188](docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md#L180)). Scheduling placement remains contradictory; see M-5. |
| M-2: rejected-submit gate | APPLIED | Only successful ioctls establish in-flight state. The EBUSY test requires subsequent scene progress without a synthetic retirement ([L57–62](docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md#L57), [L228–233](docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md#L228)). Repeated scheduling after rejection is a separate gap, M-6. |
| B-2: queued first-entry cancellation | APPLIED | First entry blocked by a cursor commit takes composed fallback, releases pins and creates no queued successor ([L181–184](docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md#L181), [L214–219](docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md#L214)). |
| M-3: failed-hide recovery | PARTIAL | The plan adds a wakeup owner, retirement-driven retries and a final state. It still does not guarantee the retirements needed to reach recovery or exhaustion; see M-3. |
| M-4: visible cursor latency | APPLIED | Pointer timestamps travel through accepted commits to retirement; hardware acceptance separately measures motion-to-visible and permits the accepted two-frame latency ([L114–120](docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md#L114), [L285–289](docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md#L285)). |
| M-5: motion bypassing scene priority | PARTIAL | Motion now records positions and an ordering test covers retirement → motion → scheduling. The universal scheduling claim conflicts with the retained direct-retirement submit contract and leaves clear-CRTC first-entry admission unspecified; see M-5. |

## Findings

### Blocking

None.

### Major

**M-3 — Hide recovery still depends on retirements nobody guarantees**

The authoritative recovery contract retries hiding at later flip retirements, while retaining hardware ownership and postponing `note_einval` until hiding succeeds ([plan L63–75](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:63)).

The assigned handoff only wakes the scene and requests an unflip when direct scanout is active ([scanout.rs L267–278](/home/ariel_santangelo/Projects/yserver-atomic-cursor/crates/yserver/src/kms/render/backend/kms/scanout.rs:267)). A scene wake adds no output damage ([damage.rs L17–30](/home/ariel_santangelo/Projects/yserver-atomic-cursor/crates/yserver-render/src/kms/render/scene/damage.rs:17)); unchanged hardware assignment produces no transition ([cursor.rs L291](/home/ariel_santangelo/Projects/yserver-atomic-cursor/crates/yserver-render/src/kms/render/scene/cursor.rs:291)), and unchanged cursor metadata produces no damage ([L535–547](/home/ariel_santangelo/Projects/yserver-atomic-cursor/crates/yserver-render/src/kms/render/scene/cursor.rs:535)).

Concrete sequence: an idle composed desktop rejects a cursor position; hide fails; the wake may produce one frame because the pointer footprint changed. Its retirement retries hiding, which fails again. With no further damage and unchanged hardware assignment, subsequent inspection can skip composition. No next retirement is guaranteed, so recovery can remain pending indefinitely without reaching the promised final state.

**Required correction:** make recovery itself produce the required cursorless transactions, or assign a bounded retry mechanism independent of unrelated flips. Test idle recovery with an initial failed retry, requiring accepted transactions to produce subsequent retirements. The unconditional “applied” claim at [plan L328–330](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:328) remains overstated.

**M-5 — The single scheduling boundary conflicts with direct admission and retirement**

Task 3 says every commit is decided in one scheduling pass, but also requires cursor retirement to call `submit_queued_direct_successor` ([plan L168–188](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:168)). That existing function submits immediately ([scanout.rs L539–558](/home/ariel_santangelo/Projects/yserver-atomic-cursor/crates/yserver/src/kms/render/backend/kms/scanout.rs:539)). Clear-CRTC direct admission likewise submits inside Present handling ([present.rs L834–869](/home/ariel_santangelo/Projects/yserver-atomic-cursor/crates/yserver/src/kms/render/backend/kms/present.rs:834)), before the later composition boundary ([run.rs L3083–3090](/home/ariel_santangelo/Projects/yserver-atomic-cursor/crates/yserver-core/src/core_loop/run.rs:3083)).

For an eligible first-entry Present arriving with clear CRTCs, preserving immediate submission violates the selected boundary. Deferring acceptance to that boundary creates a queued first-entry frame without a current direct frame. If an overlay invalidates eligibility before scheduling, the plan supplies no cancellation/completion owner for that state—the ownership problem B-2 intentionally avoided.

**Required correction:** explicitly permit existing direct callbacks outside the pass, with shared blocker checks, or specify their handoff into the pass and ownership of deferred first entries. Add Present → eligibility invalidation → scheduling coverage if first entry is deferred. Motion queuing fixes the original starvation sequence; it does not establish the broader universal-boundary claim.

**M-6 — EBUSY leaves an immediately schedulable cursor retry**

The plan preserves the unsent position after EBUSY, makes its existence request scheduling, and directs an otherwise idle pass to submit that position ([plan L61–62](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:61), [L168–179](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:168)). Existing wake scheduling returns an immediate deadline when compose work exists and an output is ready ([scanout.rs L1903–1909](/home/ariel_santangelo/Projects/yserver-atomic-cursor/crates/yserver/src/kms/render/backend/kms/scanout.rs:1903)).

Concrete sequence: idle cursor submission returns EBUSY; no in-flight state exists, but the position remains unsent. The next pass has neither direct work nor a scene frame, so arbitration attempts cursor-only submission again. Repeated rejection can drive immediate retries across loop iterations. Nothing distinguishes a retained position from one eligible for another cursor-only attempt.

The test forbids only a second attempt **in the same pass** ([plan L228–233](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:228)); it cannot establish the no-retry-loop requirement at L57–59.

**Required correction:** specify what progress makes a rejected position eligible again, while allowing scene commits to carry it. Test successive idle passes without new input, including wakeup behavior, rather than only one pass.

### Minor

None.

## Coverage and implementation checks

1. **Incorporation:** assessed all seven findings represented in round 3. M-3 and M-5 remain partial; their unconditional incorporation claims are not established.
2. **Architecture:** checked motion routing, scene gating, direct admission/continuation, recovery handoff and the actual Present-before-compose boundary. Grouped and copied-output positions were assessed at the plan-contract level.
3. **Safety and failure semantics:** checked accepted-only bookkeeping, recovery progress, rejection scheduling and topology/VT event draining. Both lifecycle paths snapshot pending CRTCs before draining and stop progression on drain failure; the plan requires adding cursor commits to that ownership set.
4. **Specification and verification:** assessed rejection, arbitration, topology, hidden-cursor and latency contracts. Implementation gates assign formatting, all-target clippy, tests, stress and hardware validation ([plan L269–309](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:269)); CI separately runs linked builds.

**Source excerpts: 24/24**, each at most 120 lines. Investigation stopped at the budget. The plan and prior review were read once; one initial locator exceeded the requested 40-line search-output limit.

External spike artifacts, hardware scripts, physical driver behavior, framebuffer/fence internals and musl buildability were not independently assessed and are not declared sound. Exact Rust APIs, fixtures, compilation, test outcomes and portability checks remain implementation work. No builds, tests, benchmarks, installs, compilation experiments or subagents were used.