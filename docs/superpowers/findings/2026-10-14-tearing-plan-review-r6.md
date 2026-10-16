## Verdict

**0 blocking, 1 major, 0 minor**

**Coverage: INCOMPLETE**

This is a bounded design-review result. It does not establish compilation, passing tests, or implementation approval.

## Incorporation audit

Statuses describe the actual task contracts, not completed implementation.

| Prior finding | Result |
|---|---|
| R5 B-1 — Capability advertisement uses the primary device | **APPLIED.** Task 1 resolves the Present CRTC to its owning device and assigns differing-device tests ([plan:131–159](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:131)). |
| R4 B-1 — Pre-entry retirement can rebind the cursor | **APPLIED.** Entry waits for submitted cursor transitions to retire; a later qualifying owner Present enters ([plan:311–316,366–369](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:311)). |
| R3 B-1 — Live revalidation loses shape and target contracts | **APPLIED.** Revalidation shares admission’s gate, re-derives target classification and checks the complete shape chain ([plan:412–423](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:412)). |
| R3 M-1 — Probation lacks a window owner | **APPLIED.** Another qualifying window restarts the streak at one ([plan:304–310,370–372](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:304)). |
| R2 B-1 — Handoff waits for retired classification | **APPLIED.** Desired visibility triggers handoff before the held-direct return; tests require an actual software cursor draw ([plan:433–473](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:433)). |
| R2 M-1 — Outputs erase rejection history | **APPLIED.** Independent submission streams retain separate histories and receive interleaved rejection coverage ([plan:189–198,250–252](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:189)). |
| R1 B-1 — Hidden entry permits HW rebinding | **APPLIED.** Whole-`On` suppression covers direct binding, refresh, ShowCursor and scene assignment ([plan:330–350](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:330)). |
| R1 M-1 — Composed outcomes cannot reach the ledger | **APPLIED.** Both routes synchronously update the platform-owned ledger ([plan:199–203,246–249](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:199)). |
| R1 M-2 — Eligibility remains cached between Presents | **APPLIED.** Every composition pass revalidates ownership and assigns every output’s timing; Copy completion remains separate ([plan:408–432,504–506](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:408)). |
| R1 m-1 — Exit must restore the appropriate cursor policy | **APPLIED.** Task 5 specifies HW, SW, Hidden and suspended exit policies with end-state tests ([plan:538–554](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:538)). |

## Findings

### Blocking

None established.

### Major

**M-1 — Present-driven cursor handoff lacks ownership of the newest Copy fallback**

Task 4 requests the existing grouped unflip at tearing entry and makes cursor-only ineligible Presents return to Copy without the usual unflip branch ([plan:438–459](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:438)). It does not assign the fallback-source registration needed to distinguish a newly copied authoritative frame from the retained direct frame.

That registration is an existing ownership contract:

- The ordinary ineligible-root branch records `unflip_fallback_source` ([source:755–768](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver/src/kms/render/backend/kms/present.rs:755)).
- After Copy, `note_present_pixmap` marks the shadow ready **only when the source matches that registration** ([source:679–686](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver/src/kms/render/backend/kms/present.rs:679)).
- Requesting unflip merely changes flags; it does not materialize immediately ([source:303–313](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver/src/kms/render/backend/kms/scanout.rs:303)).
- Without the ready marker, lazy materialization copies the retained direct source into the fallback target ([source:775–818](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver/src/kms/render/backend/kms/scanout.rs:775)).

Concrete sequence permitted by the plan:

1. W holds direct frame F0 with a visible hardware cursor.
2. A qualifying Present F1 enters `On` and requests cursor handoff.
3. Direct admission declines while unflip is requested; the core records F1’s ordinary Copy, then calls `note_present_pixmap` ([source:788–794](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver/src/kms/render/backend/kms/present.rs:788), [core:995–1050](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver-core/src/core_loop/process_request/present_ext.rs:995)).
4. No contract registers F1 as the handoff fallback. Its notification therefore leaves `unflip_shadow_ready` false.
5. Grouped unflip materializes F0 **after F1’s Copy**, overwriting the newest scene content. If W stops presenting, subsequent cursor compositions retain F0.

The older requirement introduced by **9c4ef8f0** is explicit: “When an ineligible authoritative successor requests unflip, its normal Copy is the current fallback.” Lazy copying of the retained frame applies when there is no such successor ([M2 plan:110–113](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-08-05-present-direct-scanout-m2.md:110)). The new plan acknowledges overriding cursor binding, but does not acknowledge or replace this fallback distinction. The authoritative spec requires preserving the handoff’s retained-source contract and unchanged Copy semantics ([spec:143–145,244–247](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/specs/2026-10-14-tearing-on-demand-design.md:143)).

**Smallest correction:** specify that every authoritative W Present taking Copy while handoff is pending registers its source and resolved fallback target before declining direct admission. Its Copy notification must prevent subsequent materialization of an older direct frame. Assign a core-entry test using distinct F0/F1 content, with F1 being the entry Present and no further client frames.

### Minor

None established.

## Coverage and implementation checks

1. **Incorporation:** assessed all ten findings carried by R5 against current task text. No compiler-only backlog was carried forward.

2. **Architecture and cross-task contracts:** assessed capability routing, probation ownership, ledger streams, scene timing, live eligibility and cursor handoff. Verified immediate scene wakeup and the fallback producer/consumer mismatch in M-1. Full lifecycle-hook and core-loop coverage was not completed.

3. **Safety, ownership and failure semantics:** inspected cursor assignments, retirement transitions, retry gating, physical visibility checks and retained fallback ownership. Suppression prevents retry-generated Shows because those require a fresh HW assignment. Complete cancellation and event-routing interactions remain unassessed.

4. **Specification and verification:** assessed admission, exits, request restrictions, suspension, completion, pacing and cursor-cap requirements. The plan assigns formatting, builds, tests, all-targets clippy, layout checks, mutations, loaded runs and hardware validation. None was executed. Musl/FreeBSD compatibility remains implementation verification. The no-out-fence reuse proof remains Task 2’s explicit stop-and-report prerequisite, not a finding.

5. **Provenance:** traced:
   - Cursor modes/assignments/transitions: **a14edf52**, Stage-5 plan:341–399; Hidden and ShowCursor behavior: **e118bc3e**, atomic-cursor plan:292–307. The plan acknowledges its binding overrides.
   - Direct probation and fallback ownership: **9c4ef8f0**, M2 plan:30–120 and direct-scanout spec:184–231. Fallback preservation is unresolved in M-1.
   - Capability meanings: **363dd7d7**, Phase-4.2 spec:451–480. Window/device routing is now explicit.
   - Atomic rejection attribution: **d83df86a** rationale located; referenced design-document provenance was not established.

**Excerpt accounting: 24/24 general; 12/12 provenance.** One commit-message lookup exceeded the length limit and was truncated. Investigation stopped at the budget.

Unverified ground includes the latest retry-state design provenance, atomic-kind/event routing, shared/copied-route provenance and complete lifecycle cancellation. A targeted follow-up should establish whether pending cursor positions, recovery state and accepted cursor-only events retain correct ownership across `On` entry, exit and hidden direct re-entry. These areas are unassessed, not sound.