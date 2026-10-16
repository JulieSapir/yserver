## Verdict

**1 blocking, 0 major, 0 minor**

**Coverage: INCOMPLETE**

This is a design-review result. It does not establish compilation, passing tests, or implementation approval.

## Incorporation audit

| Prior finding | Result |
|---|---|
| R3 B-1 — Live revalidation loses shape and target contracts | **APPLIED.** Task 4 retains admission context, re-derives target classification, preserves COW-specific rules, and checks the complete shape chain. Shape-loss and COW-descendant tests are assigned ([plan:382–393,454–459](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:382)). |
| R3 M-1 — Probation lacks a window owner | **APPLIED.** `probation_owner` owns the streak; another qualifying window restarts it at one. The A→B test exercises this contract ([plan:263,284–291,340–342](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:284)). |
| Carried R2 B-1 — Handoff waits for retired classification | **APPLIED.** Desired visibility triggers handoff before the held-direct return. Tests require an actual software cursor draw ([plan:403–423,436–443](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:403)). |
| Carried R2 M-1 — Outputs erase rejection history | **APPLIED.** Separate submission streams retain independent histories; interleaved rejection coverage is assigned ([plan:172–181,233–235](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:172)). |
| Carried R1 B-1 — Hidden entry permits hardware rebinding | **PARTIAL.** Direct binding, refresh, ShowCursor and future scene assignments are guarded. A previously queued scene retirement can still bind the plane; see B-1. |
| Carried R1 M-1 — Composed outcomes cannot reach the ledger | **APPLIED.** Both routes synchronously consult and update the platform-owned ledger ([plan:182–186,229–232](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:182)). |
| Carried R1 M-2 — Eligibility remains cached between Presents | **APPLIED at the contract level.** Every composition pass revalidates current eligibility and assigns every output’s timing. Task 4 now includes shape changes ([plan:378–402](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:378)). |

The cursor-restoration correction discussed in the prior review is also explicitly assigned to Task 5 ([plan:508–524](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:508)).

## Findings

### Blocking

**B-1 — Pre-entry scene retirement can rebind the hardware cursor during `On`**

The plan suppresses future scene assignments through `SceneInput::hw_cursor_suppressed`, while stating that “Nothing else changes in `yserver-render`” ([plan:277–279](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:277)). Its whole-`On` suppression contract guards direct binding, refresh, ShowCursor and subsequent scene builds ([plan:304–316](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:304)). It does not deliver that policy to the owner of an already-submitted scene transition.

Existing `CursorTransition::ShowOnRetire` is stored on a successful frame’s `PendingAck` and binds the plane when that frame retires ([definition:231–244](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver-render/src/kms/render/scene.rs:231)). Retirement applies the saved transition, then records its resolved mode ([source:193–211](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver-render/src/kms/render/scene/flip.rs:193)); the Show branch uploads and calls `show` without consulting the new suppression flag ([source:471–514](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver-render/src/kms/render/scene/cursor.rs:471)).

Concrete sequence:

1. Fullscreen W has seven qualifying Async Presents.
2. A scene frame is accepted with `ShowOnRetire`; the plane remains physically hidden until its event.
3. W’s eighth qualifying Present enters `On`. The cursor term is ignored, so the pending transition’s `Mixed` classification does not prevent entry.
4. The earlier scene frame retires. Its saved Show transition binds the hardware cursor while `On`.

Changing the next build’s strategy cannot cancel this saved action. The physical visibility check prevents subsequent async submissions once the show has happened, but it does not satisfy the specification’s separate requirement that nothing rebind the hardware cursor while `On` ([spec:148–156](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/specs/2026-10-14-tearing-on-demand-design.md:148)).

The old retirement ownership is deliberate: **a14edf52** introduced per-output transitions whose application occurs at retirement. Its referenced plan requires: “Then call `cursor_plane_show_on_crtc`” for the retiring CRTC ([historical plan:385–397](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-05-19-stage-5-hw-cursor.md:385)). The revised plan acknowledges overrides of M2 direct binding and ShowCursor, but leaves this older producer authoritative.

**Smallest correction:** define how entry handles queued HW transitions. Either serialize entry behind their retirement or deliver suppression to the retirement consumer and resolve the canceled Show using actual physical ownership, with a software repaint when needed. Extend the existing entry test with a `ShowOnRetire` queued before the eighth Present and retired afterward. This requires a cross-owner contract, beyond another future-build guard.

### Major

None established.

### Minor

None established.

## Coverage and implementation checks

1. **Incorporation:** checked the two R3 findings and the carried findings listed in its audit against actual task text. Shape/COW revalidation and probation ownership are incorporated. Whole-`On` suppression remains partial.

2. **Architecture and cross-task contracts:** verified existing Present admission, authoritative-target classification, direct successor arbitration, shared/copy submission boundaries and immediate scene wakeup. Ledger ownership is explicit. Found the missing policy delivery to queued scene retirement. Full core-loop and every lifecycle path were not traced.

3. **Safety, ownership and failure semantics:** examined cursor state definitions, pending transition ownership, retirement ordering and cursor-only commit arbitration. Deferred sprite uploads and every cancellation/exit path remain unassessed. The no-out-fence BO proof remains Task 2’s explicit stop-and-report prerequisite; it is not a finding or completed proof.

4. **Specification and verification:** assessed entry/exit, live eligibility, request restrictions, fallback/suspension, Copy completion, pacing and cursor-cap requirements. Host build/tests, formatting, all-targets clippy, mutations, loaded runs and hardware validation are assigned. No checks were executed. Explicit musl/FreeBSD gates and per-window versus primary-device capability routing were not fully reconciled.

5. **Provenance:** traced these changed contracts:
   - Cursor assignment/mode/retirement: **a14edf52/a7fc9752**, Stage-5 cursor plan; **e118bc3e**, atomic-cursor plan. Hidden compatibility and grouped arbitration are retained; queued Show ownership is unresolved in B-1.
   - Direct targets, probation and unflip ownership: **9c4ef8f0**, M2 plan and direct-scanout design. Target-specific shape rules and retained-source retirement are explicitly preserved.
   - Capability meanings: **363dd7d7**, Phase-4.2 spec:451–480. `flip_path` remains separate; Async decoupling is acknowledged.
   - Cursor rejection attribution: **d83df86a**, commit rationale. Excluding cursorless Async requests preserves the stated attribution reason; no referenced design document was located.
   - Later COW/restack changes **dc753c13/0b1bb820** were located, but their document rationale was not established. BO-phase and copied-transport provenance was not completed.

**Excerpt accounting:** **24/24 general; 12/12 provenance**, conservatively charging the ten commit-message reads separately. One batched history lookup exceeded the per-excerpt limit and was truncated; unseen history was not treated as verified evidence. Investigation stopped at the budget.

Specific unassessed follow-up scope: deferred cursor-upload cancellation across entry/exit, preservation of the later COW/restack revocation rationale, and capability routing across different KMS devices. These areas are **unverified**, not sound.