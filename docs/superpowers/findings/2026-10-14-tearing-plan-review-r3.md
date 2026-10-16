## Verdict

**1 blocking, 1 major, 0 minor**

**Coverage: INCOMPLETE**

This is a design-review result. No builds or tests were run, and implementation is not approved.

## Incorporation audit

| Prior finding | Result |
|---|---|
| R2 B-1 — Cursor handoff waits for the classification it must create | **APPLIED.** Desired visibility triggers handoff before the held-direct return. Tests now require an actual software cursor draw after ShowCursor and dead-zone motion ([plan:389–428](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:389)). |
| R2 M-1 — Independent outputs erase rejection history | **APPLIED.** The authoritative platform ledger now has a grouped direct stream and separate composed streams. Interleaved two-output rejection coverage is assigned ([plan:172–186,233–235](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:172)). |
| Carried R1 B-1 — Hidden entry permits hardware rebinding | **APPLIED at the declared contract level.** Whole-`On` suppression and physical visibility authorization cover direct retirement, refresh, ShowCursor and sprite changes ([plan:301–321](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:301)). Complete producer-path preservation remains unverified. |
| Carried R1 M-1 — Composed outcomes cannot reach the ledger | **APPLIED.** Both submission routes synchronously update the platform-owned ledger; composed rejection tests exercise that ownership ([plan:182–186,229–235](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:182)). |
| Carried R1 M-2 — Eligibility remains cached between Presents | **PARTIAL.** Current stacking and coverage are checked, addressing the demonstrated dialog case. The broader “any other scene change” claim does not hold for shape changes; see B-1. |

## Findings

### Blocking

**B-1 — Live revalidation loses the direct gate’s shape and target contracts**

`tearing_scene_still_owned` checks mapping, topmost placement, rectangular coverage, overlays, transforms and capabilities, then authorizes composed async submission. It explicitly reuses `unredirected_direct_scene_eligible` ([plan:376–388](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:376)).

That helper checks top-level geometry and stacking; it does not check Bounding or Clip shapes ([source:1383–1440](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver/src/kms/render/backend/kms/scanout.rs:1383)). Actual Present admission additionally requires the complete shape chain to cover the root ([source:727–729](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver/src/kms/render/backend/kms/present.rs:727)). The authoritative spec includes that requirement and requires leaving `On` when eligibility is lost ([spec:107–114,175–179](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/specs/2026-10-14-tearing-on-demand-design.md:107)).

Concrete sequence:

1. Fullscreen W reaches `On` and completes its handoff to composed software-cursor frames.
2. W stops presenting; its Bounding or Clip shape becomes partial or empty.
3. The existing shape handler wakes composition. Its direct-frame invalidation check concerns retained direct frames, which need no longer exist after handoff ([source:2491–2502](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver/src/kms/render/backend/portable/windows.rs:2491)).
4. W remains mapped, geometrically fullscreen and topmost. The planned live check passes, authorizing async composition of the newly exposed desktop.

This violates the tear-free desktop and eligibility-loss requirements ([spec:199–208](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/specs/2026-10-14-tearing-on-demand-design.md:199)). The historical requirement included “stacking or shape change” as an unflip trigger: **9c4ef8f0**, [direct-scanout design:222–227](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/specs/2026-08-05-present-direct-scanout-design.md:222). This override is not acknowledged.

The same replacement predicate also mishandles COW targets. Existing admission accepts `Cow`, `CowDescendant` and `Unredirected`, applying the topmost-window helper only to `Unredirected`. The helper excludes the COW from its topmost search. Applying it universally therefore makes a qualifying COW-stage owner fail the next composition-pass check ([source:27–34](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver/src/kms/render/backend/kms/scanout.rs:27), [source:1318–1344](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver/src/kms/render/backend/kms/scanout.rs:1318), source:727–729 above). The original M2 goal explicitly includes compositor-authoritative buffers ([M2 plan:3–5](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-08-05-present-direct-scanout-m2.md:3)).

**Smallest correction:** retain enough owner/admission context after unflip to revalidate the full live eligibility contract, preserving target-specific COW versus unredirected rules and complete shape-chain coverage. Assign Backend-driven tests for shape loss without another W Present and composed tearing from a qualifying COW descendant.

### Major

**M-1 — Entry probation has no window-owner contract while `Off`**

The spec requires eight consecutive qualifying Presents **from the same window** ([spec:163–166](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/specs/2026-10-14-tearing-on-demand-design.md:163)). The plan defines a counter and an `On` window identity, but no probation identity while `Off`. Its reset rules cover failed eligibility and missing Async, without defining what happens when another qualifying root window presents ([plan:259–288](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:259)).

Consider four qualifying Presents from fullscreen A, then a stacking change raising fullscreen B, followed by four qualifying Presents from B. No intervening Present fails the gate or lacks Async. Counting according to the stated advance/reset rules reaches eight and authorizes B after only four B Presents.

This is an ownership-contract gap, not a Rust field-inventory objection: the counter needs an authoritative client identity before entry. The existing direct counter is deliberately a root-stream admission counter; preserving its meaning does not establish the new per-window requirement ([source:517–529](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver/src/kms/render/backend.rs:517), **9c4ef8f0**, [M2 plan:117–120](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-08-05-present-direct-scanout-m2.md:117)).

**Smallest correction:** define the probation owner and restart its streak when a different qualifying window takes ownership. Add a qualifying A→B switch test; Async/non-Async alternation does not establish this contract.

### Minor

None.

## Coverage and implementation checks

1. **Incorporation:** assessed both R2 findings and the carried design corrections. The handoff cycle and competing ledger streams are addressed; live eligibility remains partial.

2. **Architecture:** verified Present admission, target classification, direct-hold arbitration, per-output shared submission and immediate scene wakeup. Ledger ownership and delivery are explicit. Found the revalidation and probation-identity gaps.

3. **Safety and failure semantics:** examined retired cursor modes, desired assignments, two-phase hide/reveal ordering and shared submission’s rejection boundary. Complete pending cursor-event/lifecycle preservation was not established. No-out-fence BO reuse remains Task 2’s explicit stop-and-report prerequisite; it is neither a finding nor completed evidence.

4. **Specification and verification:** reviewed entry, exits, request shape, fallback, suspension, Copy completion, pacing and cursor-cap requirements. Formatting, host build/tests, all-targets clippy, mutations, loaded runs and hardware checks are assigned. Actual results and musl/FreeBSD buildability remain implementation checks.

5. **Provenance:** searched production producers and read affected state definitions. Traced:
   - Cursor modes/assignments: **a14edf52/a7fc9752**, Stage-5 plan:47–119; **e118bc3e**, atomic-cursor plan:213–307. Retirement ownership is retained; direct binding and ShowCursor overrides are acknowledged.
   - Targets/direct probation: **9c4ef8f0**, M2 plan and direct-scanout design. Preservation is incomplete in B-1.
   - Capability meanings: **363dd7d7**, Phase-4.2 spec:451–480. `flip_path` remains separate; Async decoupling is approved.
   - BO phases/fences: **51e0612d**, Phase-4.1 spec:147–196. GPU-reader exclusion remains required and unverified.
   - Copied transport: **a6f8909c**, reverse-PRIME spec:289–330. Retaining vsync declares preservation.
   - Cursor attribution: **d83df86a** commit rationale; no referenced design document was located.

**Reading budget:** **24/24 general excerpts; 12/12 provenance excerpts**, including two historical commit-rationale excerpts. Investigation stopped at the boundary.

Unassessed follow-up scope: verify the later COW/restack revocation rationale identified through **dc753c13/0b1bb820**, and pending cursor-event cancellation across every whole-`On` suppression and exit path. These areas are not marked sound.