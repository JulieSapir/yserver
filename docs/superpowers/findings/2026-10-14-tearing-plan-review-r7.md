## Verdict

**0 blocking, 0 major, 0 minor**

**Coverage: INCOMPLETE**

No new design defect was established. The bounded investigation did not complete lifecycle/event-routing verification or all behavior provenance. This is not implementation approval or evidence that builds or tests pass.

## Incorporation audit

The prior review contains R6 M-1 and ten carried findings. All eleven have explicit corrections in the current task text.

| Prior finding | Result |
|---|---|
| R6 M-1 — Newest Copy lacks handoff ownership | **APPLIED.** Task 4 registers every authoritative Copy fallback before declining direct admission, including the entry Present; the core-entry test distinguishes F1 from retained F0 ([plan:460–488](docs/superpowers/plans/2026-10-14-tearing-on-demand.md#L460)). |
| R5 B-1 — Capability uses primary device | **APPLIED.** Resolve the Present CRTC to its owning device; test differing device capabilities through QueryCapabilities ([plan:131–159](docs/superpowers/plans/2026-10-14-tearing-on-demand.md#L131)). |
| R4 B-1 — Pre-entry retirement can rebind cursor | **APPLIED.** Entry waits for submitted cursor transitions to retire, with a queued-Show test ([plan:311–316,366–369](docs/superpowers/plans/2026-10-14-tearing-on-demand.md#L311)). |
| R3 B-1 — Revalidation loses shape/target contracts | **APPLIED.** Reuse admission’s gate, rederive target classification, and check the complete shape chain ([plan:412–423](docs/superpowers/plans/2026-10-14-tearing-on-demand.md#L412)). |
| R3 M-1 — Probation lacks window owner | **APPLIED.** Another qualifying window restarts the streak at one; a restacking test covers it ([plan:304–310,370–372](docs/superpowers/plans/2026-10-14-tearing-on-demand.md#L304)). |
| R2 B-1 — Handoff waits for retired classification | **APPLIED.** Desired visibility triggers handoff before the held-direct return; tests require an actual software cursor draw ([plan:433–450,476–483](docs/superpowers/plans/2026-10-14-tearing-on-demand.md#L433)). |
| R2 M-1 — Outputs erase rejection history | **APPLIED.** Independent streams retain independent histories, with interleaved rejection coverage ([plan:189–198,250–252](docs/superpowers/plans/2026-10-14-tearing-on-demand.md#L189)). |
| R1 B-1 — Hidden entry permits HW rebinding | **APPLIED.** Whole-`On` suppression covers direct binding, refresh, ShowCursor, sprite refresh and scene assignment ([plan:330–350](docs/superpowers/plans/2026-10-14-tearing-on-demand.md#L330)). |
| R1 M-1 — Composed outcomes cannot reach ledger | **APPLIED.** Both routes synchronously update the platform-owned ledger ([plan:199–203,246–249](docs/superpowers/plans/2026-10-14-tearing-on-demand.md#L199)). |
| R1 M-2 — Eligibility cached between Presents | **APPLIED.** Every composition pass revalidates ownership and assigns every output’s timing; Copy completion remains independent ([plan:408–432,519–521](docs/superpowers/plans/2026-10-14-tearing-on-demand.md#L408)). |
| R1 m-1 — Exit restores wrong cursor policy | **APPLIED.** Task 5 specifies HW, SW, Hidden and suspended policies, with parameterized end-state tests ([plan:551–569](docs/superpowers/plans/2026-10-14-tearing-on-demand.md#L551)). |

These statuses assess incorporation into the plan, not implementation correctness.

## Findings

### Blocking

None established.

### Major

None established.

### Minor

None established.

## Coverage and implementation checks

1. **Incorporation — complete.** Checked all eleven findings against actual task contracts and tests. No compiler-level backlog was carried forward.

2. **Architecture and cross-task contracts — partial.** Assessed capability routing, shared admission/revalidation, probation ownership, ledger streams, direct/composed timing, cursor handoff and fallback registration. Verified that the existing direct topology gate retains primary-device and homogeneous-refresh restrictions ([source:1032–1075](crates/yserver/src/kms/render/backend/kms/scanout.rs#L1032)). The immediate scene wake premise is supported by `scene_wants_compose` and `backend_scanout_next_wakeup` ([source:1772–1811](crates/yserver/src/kms/render/backend/kms/scanout.rs#L1772), [2030–2046](crates/yserver/src/kms/render/backend/kms/scanout.rs#L2030)). Complete core-loop event delivery and lifecycle draining were not inspected.

3. **Safety, ownership and failure semantics — partial.** Inspected cursor mode definitions and producers, transition derivation/retirement, physical visibility, accepted cursor-only ownership, EBUSY waits, rejection recovery and successful-hide cleanup. The plan’s physical visibility authority matches the spec’s confirmed-hide requirement ([spec:143–156](docs/superpowers/specs/2026-10-14-tearing-on-demand-design.md#L143)). Complete cancellation and retry-state interactions across exit, resume and hidden direct re-entry remain unverified. The no-out-fence BO reuse proof remains Task 2’s explicit stop-and-report prerequisite, as required by [spec:249–253](docs/superpowers/specs/2026-10-14-tearing-on-demand-design.md#L249).

4. **Specification and verification strategy — assessed within inspected contracts.** Read the normative admission, probation, exit, pacing, cursor-cap, request, fallback, completion, advertisement and testing requirements. The plan assigns evidence for the principal contracts in [spec:158–208](docs/superpowers/specs/2026-10-14-tearing-on-demand-design.md#L158), [215–247](docs/superpowers/specs/2026-10-14-tearing-on-demand-design.md#L215) and [274–319](docs/superpowers/specs/2026-10-14-tearing-on-demand-design.md#L274). Formatting, builds, tests, all-targets clippy, layout checks, mutations, loaded runs and hardware measurements are assigned to implementation. None was executed. Musl/FreeBSD buildability also remains implementation verification.

5. **Changed-behavior provenance — partial.** Definitions and producer searches covered these families:

   - **`PresentCaps` and capability encoding:** **363dd7d7**, [Phase-4.2 spec:451–480](docs/superpowers/specs/2026-05-09-phase4-2-dri3-present-glx-design.md#L451). The plan preserves `flip_path` and per-window routing; the authoritative tearing spec knowingly changes Async semantics.
   - **Cursor modes, assignments, transitions and recovery:** **a14edf52**, [Stage-5 plan:341–399](docs/superpowers/plans/2026-05-19-stage-5-hw-cursor.md#L341); **e118bc3e**, [atomic-cursor plan:54–95](docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md#L54) and [292–307](docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md#L292). Binding and ShowCursor overrides are acknowledged. History traced `SwPending` to **a6f8909c**, but its specific introducing document rationale was not established.
   - **Direct probation and fallback:** **9c4ef8f0**, [M2 plan:30–120](docs/superpowers/plans/2026-08-05-present-direct-scanout-m2.md#L30). The plan explicitly preserves successor-Copy ownership and confines its cursor exception to `On`. Later fullscreen/topology gate provenance was not completed.
   - **Atomic kinds and rejection attribution:** producer sites and sink behavior were inspected. **d83df86a** supplies the attribution rationale; an associated design document was not located. Async’s exclusion is explicit in [spec:222–223](docs/superpowers/specs/2026-10-14-tearing-on-demand-design.md#L222).
   - **Shared/copied routes and `finish_frame`:** **7e325a61**, [Phase-3b plan:81–159](docs/superpowers/plans/2026-10-13-phase3b-scene-split.md#L81); **a6f8909c**, [reverse-PRIME spec:286–315](docs/superpowers/specs/2026-07-14-reverse-prime-copy-design.md#L286). Platform ledger ownership preserves the synchronous sink boundary; copied transport remains unchanged.
   - **Wake/pacing behavior:** current wake ownership was inspected; its complete historical rationale was not established.

**Excerpt accounting: 24/24 general; 12/12 provenance.** Boundary deviations: one source excerpt contained 121 lines, and one history lookup produced oversized, truncated output. Investigation stopped at the budget.

The specific unresolved follow-up is whether pending cursor positions, recovery records and accepted cursor-only events retain correct device/CRTC ownership across `On` exit, hidden direct re-entry, and VT/topology draining. That interaction and the incomplete provenance items above are **unassessed, not sound**.