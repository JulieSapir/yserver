## Verdict

**1 blocking, 0 major, 0 minor**

**Coverage: INCOMPLETE**

This is a design-review result. It does not establish compilation, passing tests, or implementation approval.

## Incorporation audit

The table includes R4’s finding and the earlier findings carried in its audit.

| Prior finding | Result |
|---|---|
| R4 B-1 — Pre-entry retirement can rebind the cursor | **APPLIED.** Entry waits until submitted cursor transitions retire; a later qualifying Present enters. The queued-Show sequence has a mutation test ([plan:294–299,349–352](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:294)). |
| R3 B-1 — Live revalidation loses shape and target contracts | **APPLIED.** Revalidation uses the shared admission gate, current target classification and complete shape chain, with separate COW rules ([plan:395–406](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:395)). |
| R3 M-1 — Probation lacks a window owner | **APPLIED.** The streak has an owner; another qualifying window restarts it at one ([plan:291–293,353–355](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:291)). |
| R2 B-1 — Handoff waits for retired classification | **APPLIED.** Desired visibility triggers handoff before the held-direct return. Tests require an actual SW cursor draw ([plan:416–436,449–456](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:416)). |
| R2 M-1 — Outputs erase rejection history | **APPLIED.** Independent streams retain independent histories; interleaved rejection coverage is assigned ([plan:172–181,233–235](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:172)). |
| R1 B-1 — Hidden entry permits HW rebinding | **APPLIED at the contract level.** Whole-`On` suppression now includes serialization behind queued transitions ([plan:294–319](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:294)). |
| R1 M-1 — Composed outcomes cannot reach the ledger | **APPLIED.** Both submission routes synchronously update the platform-owned ledger ([plan:182–186,229–232](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:182)). |
| R1 M-2 — Eligibility remains cached between Presents | **APPLIED.** Every composition pass revalidates ownership and assigns every output’s timing. Copy completion remains separate ([plan:391–415,487–489](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:391)). |
| R1 m-1 — Exit must restore the appropriate cursor policy | **APPLIED.** Task 5 specifies HW, SW, Hidden and suspended exit policies with parameterized end-state checks ([plan:521–537](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:521)). |

## Findings

### Blocking

**B-1 — Capability advertisement uses the primary device instead of the window’s output device**

Task 1 requires `async_flip = primary_async_page_flip()` **for every window**, and its test explicitly pins that behavior ([plan:123–142](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:123)). The authoritative specification requires advertisement when **the window’s output device** has the capability, and requires the advertised bit to follow that device exactly ([spec:257–263,297](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/specs/2026-10-14-tearing-on-demand-design.md:257)).

These identities differ in supported multi-device configurations. `primary_device()` returns `devices.first()` ([source:90–92](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver/src/kms/render/platform/devices.rs:90)); live CRTC routing resolves an output’s owning device separately ([source:606–629](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver/src/kms/render/backend/kms/present.rs:606)).

Concrete sequence:

1. Primary device A supports atomic async flips; output device B does not.
2. A window displayed on B issues `QueryCapabilities`.
3. Task 1 returns A’s flag and advertises Async for that window, contrary to §3.

Reversing the capability values incorrectly suppresses advertisement on B. Direct eligibility checks cannot repair a wrong capability reply.

The older contract also distinguishes window queries from global capabilities. **363dd7d7**, Phase-4.2 spec:475–477, requires the interface to be window-keyed “because Present’s `QueryCapabilities` is per-window” ([historical spec](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/specs/2026-05-09-phase4-2-dri3-present-glx-design.md:475)). The current definition limits identical answers to “single-output single-GPU configurations” ([source:169–171](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver-core/src/backend/trait_def.rs:169)). The plan does not acknowledge overriding this requirement.

**Smallest correction:** define how the capability query resolves the window’s current output/device, including ambiguous or unavailable placement. Replace the primary-only assertion with two-device tests that query windows on each output while the devices’ capability flags differ.

### Major

None established.

### Minor

None established.

## Coverage and implementation checks

1. **Incorporation:** checked R4 and all findings carried in its audit against actual task text. R4’s serialization correction is explicit and preserves retirement-time cursor ownership.

2. **Architecture and cross-task contracts:** verified admission gates, authoritative-target classification, successor arbitration, ledger ownership, submission boundaries and immediate scene wakeup. Found the capability-routing conflict. The full core loop and every lifecycle entry were not traced.

3. **Safety, ownership and failure semantics:** inspected cursor state definitions, transition producers, deferred sprite-refresh behavior and retirement actions. Deferred sprite refresh produces a Show only when the new assignment is HW ([source:744–759](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver-render/src/kms/render/scene/tick.rs:744)); suppression therefore addresses that producer. Retry-metadata cleanup across entry/exit remains unassessed. The no-out-fence BO reuse proof remains Task 2’s explicit stop-and-report prerequisite, not a completed proof or finding.

4. **Specification and verification:** assessed entry/exit, live eligibility, request restrictions, fallback/suspension, completion, pacing and cursor-cap contracts. Formatting, build/tests, all-targets clippy, layout checks, mutations, loaded runs and hardware validation are assigned. No checks were executed. The plan does not explicitly name musl/FreeBSD target gates; target compatibility remains implementation verification.

5. **Provenance:** traced these families:
   - **Cursor modes, assignments and transitions:** **a14edf52**, Stage-5 plan:341–399; **e118bc3e**, atomic-cursor plan:292–307. Definitions and production assignment/transition producers were inspected. Hidden compatibility, per-output retirement and visible-cursor handoff requirements are preserved by the stated contracts.
   - **Direct targets and probation:** **9c4ef8f0**, M2 plan:30–120 and direct-scanout spec:184–231. Target producers and probation writers were searched. Retained-source ownership and independent direct probation remain explicit.
   - **Later restack behavior:** **dc753c13/0b1bb820** commit rationales were located. The plan preserves the distinction between COW targets and coverable unredirected targets; a referenced design/status requirement was not established within budget.
   - **Present capability meanings:** **363dd7d7**, Phase-4.2 spec:451–480. Definitions and constructors were searched. Async decoupling is acknowledged; window/device routing is unresolved in B-1.
   - **Atomic rejection attribution:** **d83df86a** explains why cursorless failures must not blame the cursor. Async’s exclusion preserves that reason; no referenced design document was located.
   - **Shared/copied routes:** **a6f8909c**, reverse-PRIME spec:23–42. Installed-route definitions and allocation producers were located; copied transport’s independent sink ownership remains unchanged. BO-phase/no-fence provenance was not completed.

**Excerpt accounting: 24/24 general; 12/12 provenance.** Investigation stopped at the budget.

Specific unassessed follow-up scope: deferred cursor retry-state cleanup through entry, exit and hidden direct re-entry, plus preservation of cancellation and lifecycle handoffs. These areas are **unverified**, not sound.