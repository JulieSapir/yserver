## Verdict

**1 blocking, 1 major, 0 minor**

**Coverage: INCOMPLETE**

This is a design-review result, not evidence that implementation compiles, tests pass, or execution is approved.

## Incorporation audit

| Prior finding | Result |
|---|---|
| B-1 — Hidden entry permits hardware rebinding | **PARTIAL.** Whole-`On` suppression and physical visibility checks address rebinding authorization ([plan:292–312](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:292)). However, the proposed visibility handoff depends on a scene classification that held direct scanout prevents from changing. See B-1 below. |
| M-1 — Composed outcomes cannot reach the ledger | **APPLIED.** The ledger moves to `PlatformBackend`; both routes update it synchronously, and a composed rejection test is assigned ([plan:176–180,223–226](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:176)). Its new stream-sharing semantics have a separate defect, M-1 below. |
| M-2 — Eligibility remains cached between Presents | **APPLIED for the demonstrated overlap scenario.** Each composition pass rechecks current scene ownership and assigns output timing; the dialog-without-another-W-Present test covers the original sequence ([plan:363–379,411–413](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:363)). Completeness for shape and geometry invalidation remains unassessed. |

## Findings

### Blocking

**B-1 — Cursor handoff waits for the classification it must create**

The plan says suppression causes the scene to assign `Sw`, which then triggers unflip ([plan:300–304](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:300)). Task 4 expressly initiates its handoff when held direct scanout already reports `Sw` or `Mixed` ([plan:380–386](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:380)).

But `cursor_mode()` classifies **retired output modes**, not the desired strategy ([cursor.rs:110–137](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver-render/src/kms/render/scene/cursor.rs:110)). Held direct scanout returns before the scene tick while the mode is `Hw` or `Hidden` and no unflip has been requested ([scanout.rs:2165–2212](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver/src/kms/render/backend/kms/scanout.rs:2165)).

Concrete sequence:

1. W holds direct scanout while XFIXES-hidden; tearing enters `On`.
2. `ShowCursor` registers the cursor, but the plan suppresses its direct rebind.
3. The retired scene classification remains `Hidden`. Task 4’s handoff condition does not match.
4. Composition keeps returning at the direct hold. No tick assigns `Sw`, so the cursor remains invisible while direct async Presents can continue.

Entering `On` from held direct with a `Hw` cursor has the same circular dependency. The specification explicitly requires initiating the grouped handoff from that state and on hidden→visible changes ([spec:136–156](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/specs/2026-10-14-tearing-on-demand-design.md:136)).

The old requirement was “`ShowCursor` during direct scanout binds the plane (Hw) or unflips (Sw), and never leaves the cursor invisible,” introduced by **e118bc3e** ([atomic-cursor plan:306–307](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:306)). Suppressing the bind requires replacing its visibility-preservation mechanism.

**Smallest correction:** initiate handoff from desired cursor visibility/strategy while direct ownership is held, before the hold-return decision. Do not require a retired `Sw`/`Mixed` classification. Extend the hidden-entry test to prove a software cursor actually becomes visible; its current physical-plane assertions can pass with an invisible cursor ([plan:333–338](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:333)).

### Major

**M-1 — One current-shape ledger makes independent outputs erase each other’s rejection history**

The ledger remembers only one shape; every different shape resets its count and suspension ([plan:165–180](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:165)). Composed timing is selected per output ([plan:363–378](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:363)), and the existing shared route submits separately for each output, with that output’s framebuffer and CRTC ([scene_sink.rs:91–149,455–485](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver/src/kms/render/platform/scene_sink.rs:91)).

Consider composed tearing across two shared outputs A and B, both permanently rejecting async. Their shape keys differ by CRTC set, and may also differ by dimensions:

- A rejects: A’s count becomes one; vsync fallback succeeds.
- B rejects: the shape change erases A’s history; B’s count becomes one.
- Next frame, A erases B’s history; B subsequently erases A’s again.

Neither output reaches suspension. Both keep issuing rejected async attempts indefinitely, despite stable output configurations. This defeats the permanent-driver-limit handling required by [spec:232–235](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/specs/2026-10-14-tearing-on-demand-design.md:232). The proposed composed suspension test does not assign independent-output coverage ([plan:223–226](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:223)).

**Smallest correction:** retain one authoritative ledger owner, but scope current-shape history to independent submission streams. Define the grouped direct stream and per-output composed streams; a shape change resets its own stream. Add a two-output Backend-driven test proving persistent rejection on both outputs reaches vsync-only submission.

### Minor

None.

## Coverage and implementation checks

1. **Incorporation:** assessed all three prior findings against task text. The handoff correction remains partial; outcome delivery and the original dialog invalidation scenario are addressed.

2. **Architecture:** traced Present admission, direct hold/successor scheduling, scene strategy propagation, per-output shared submission, and ledger ownership. Found the handoff cycle and competing ledger streams. Full shape/geometry invalidation coverage remains unassessed.

3. **Safety and failure semantics:** examined physical cursor visibility updates, retire-time transitions, direct ownership retention, rejection transitions, replacement-event BO release, and VT shutdown ordering. Physical hide failure retains visibility. The complete proof excluding outstanding GPU readers before no-out-fence BO reuse was not established. Task 2’s stop-and-report prerequisite is acknowledged, not treated as completed evidence.

4. **Specification and verification:** assessed entry, fallback, handoff, exits, pacing, cursor cap, and completion contracts against spec §§1–3 and testing requirements. Backend-driven recording, mutations, loaded runs, hardware validation, formatting, build, tests, and regular all-targets clippy are assigned. Those checks cannot substitute for the missing state/stream contracts. No checks were executed.

5. **Provenance:** searched producers of cursor modes/assignments, direct probation, capability values, BO phases/fences, physical visibility, and commit kinds.
   - **9c4ef8f0, M2 plan:42–69,110–120:** grouped ownership and anti-thrash admission are preserved in the declared contracts; hardware binding is knowingly overridden while `On`.
   - **Stage-5 cursor plan:50–119**, traced through **a14edf52/a7fc9752:** per-output retirement ownership is retained, but its retired classification is misused as a desired-strategy signal in B-1.
   - **e118bc3e, atomic-cursor plan:1–18,282–307:** hidden direct compatibility and one commit stream per CRTC remain declared; ShowCursor visibility preservation is incomplete.
   - **363dd7d7, Phase-4.2 spec:451–480:** explicit-fence path selection remains separate; Async advertisement is knowingly decoupled.
   - **51e0612d, Phase-4.1 spec:147–196:** GPU-reader completion remains required before `Free`; preservation without an out fence is unverified.
   - **a6f8909c, reverse-PRIME spec:289–330:** copied-route synchronization is preserved by retaining vsync.
   - **d83df86a:** cursor-blame rationale was available in the commit; no referenced design document was located.

**Reading budget:** **24/24 general excerpts; 12/12 provenance excerpts.** Investigation stopped at the boundary.

Targeted unresolved scope: establish GPU-reader exclusion and lifecycle reuse without an out fence, and establish whether every shape/geometry eligibility loss invalidates `On` without another W Present. Portability/buildability, Rust API details, fixtures, and actual test outcomes remain implementation checks.