## Verdict

**1 blocking, 2 major, 0 minor**

**Coverage: INCOMPLETE**

This is a design-review result, not implementation approval or evidence that code compiles or tests pass.

## Incorporation audit

| Prior findings | Result |
|---|---|
| None | First review; incorporation audit skipped. |

## Findings

### Blocking

**B-1 — Hidden-cursor entry leaves hardware cursor rebinding enabled**

The plan defines `NotSuppressed` for hidden-cursor tearing, but guards hardware binding only in `HandoffPending`, `Suppressed`, or `HideFailed` ([plan:329–357](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:329)). The specification requires that **nothing rebind the hardware plane while `On`** ([spec:153–156](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/specs/2026-10-14-tearing-on-demand-design.md:153)).

Concrete sequence:

1. The cursor is Hidden because it is off every output; XFIXES hiding is false.
2. Qualifying Presents enter `On`, with `NotSuppressed`.
3. Direct-frame retirement invokes `bind_direct_cursor_on_all_outputs`. Its existing non-XFIXES branch binds the cursor to every output ([scanout.rs:365–411](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver/src/kms/render/backend/kms/scanout.rs:365)).
4. The proposed suppression guard permits that binding. Subsequent direct timing still uses the scene’s Hidden classification, allowing async submission with a bound cursor plane.

Initially XFIXES-hidden tearing has a related edge: `ShowCursor` explicitly rebinds while the scene may still report Hidden ([cursor.rs:484–492](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver/src/kms/render/backend/kms/cursor.rs:484)). A guard tested only from `Suppressed` misses this initial `NotSuppressed` state.

This incompletely overrides an existing requirement introduced by **9c4ef8f0**: “On direct entry, bind the uploaded hardware cursor to every participating CRTC once” ([M2 plan:67–69](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-08-05-present-direct-scanout-m2.md:67)).

**Required correction:** prohibit hardware rebinding throughout `On`, including `NotSuppressed`. Define how ShowCursor and off-output→visible motion initiate the handoff before any binding. Test initial hidden entry, retirement, and first show/motion with physical plane-state assertions.

### Major

**M-1 — Composed async outcomes cannot reach their authoritative ledger**

The ledger belongs to `KmsBackend`; submit helpers report `AsyncOutcome` ([plan:164–186](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:164)). Composed submission runs inside `PlatformBackend::finish_frame`, but Task 4 provides only a backend→platform timing setter ([plan:336–348](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:336)).

The existing sink returns a frame handoff, GPU timing, and fail-stop status; it has no outcome delivery to the backend ledger ([scene_sink.rs:130–149](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver/src/kms/render/platform/scene_sink.rs:130), [FinishOutcome:77–89](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver-render/src/kms/render/scene/sink.rs:77)).

Consequently, successful vsync resubmission hides composed async `EINVAL`s behind ordinary successful frame handoffs. Without an additional contract, the backend cannot count those failures, clear counts after async acceptance, or enforce suspension. The proposed multi-CRTC suspension test exercises direct submission; property-recording tests cannot establish composed feedback.

The specification requires suspension after two same-shape async rejections ([spec:225–235](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/specs/2026-10-14-tearing-on-demand-design.md:225)).

**Required correction:** define one ledger owner and an explicit composed submission feedback path carrying shape, outcome, and lifecycle identity—or place ledger decisions where both submission routes can synchronously access them. Add a Backend-driven composed test proving two rejections produce fallback, fallback, then vsync-only submissions.

**M-2 — Composed tearing lacks an eligibility invalidation contract between Presents**

The plan evaluates tearing eligibility per Present and enables composed async from cached `On(W)`/`Suppressed` state ([plan:9](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:9), [plan:347–348](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:347)). It lists “W fails the gate” as an exit, but assigns no delivery or reevaluation contract for scene changes that occur without another W Present ([plan:266–275](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-14-tearing-on-demand.md:266)).

Concrete sequence: W is fullscreen and tearing through composition; W pauses; another application maps a dialog above W. The existing gate rejects W once another mapped window is topmost ([scanout.rs:1424–1440](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver/src/kms/render/backend/kms/scanout.rs:1424)). Dialog damage or cursor motion now triggers scene composition, but no W Present reruns the tearing decision. Cached authorization can therefore make the resulting desktop scene async.

That violates leaving `On` when W loses eligibility and keeping non-fullscreen/desktop composition vsync ([spec:175–179](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/specs/2026-10-14-tearing-on-demand-design.md:175), [spec:205–208](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/specs/2026-10-14-tearing-on-demand-design.md:205)).

**Required correction:** assign scene eligibility invalidation to relevant mutations, or revalidate current scene ownership before composed async authorization. Define how stored output timing returns to vsync. Test mapping an overlapping window while W produces no further Presents.

### Minor

None.

## Coverage and implementation checks

- **Check 1:** skipped; no prior review.
- **Check 2:** traced Present admission, grouped unflip, held-direct scheduling, scene submission, and sink results. Confirmed the immediate wakeup premise at [scanout.rs:2035–2041](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver/src/kms/render/backend/kms/scanout.rs:2035).
- **Check 3:** examined cursor ownership and transition retirement, BO phase definitions, and replacement-event release. The full shared-route GPU-reader and lifecycle-reset proof remains unassessed. Task 2’s stop-and-report prerequisite is acknowledged; it is not treated as completed evidence.
- **Check 4:** assessed fallback, handoff, exit, pacing, and completion tests. Host build/test/fmt/clippy gates and hardware validation are assigned. Actual execution, portability/buildability, and compiler-level checks remain implementation work.
- **Check 5:** searched production state assignments and admission/capability producers. Traced cursor semantics to **e118bc3e** and the [atomic cursor plan:282–307](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:282); direct ownership/probation to **9c4ef8f0** and the M2 plan; capabilities to **363dd7d7** and the Phase 4.2 spec; BO lifetime to **51e0612d** and the [Phase 4.1 spec:166–196](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/specs/2026-05-07-phase4-1-vulkan-compositor-design.md:166). Copied-route synchronization was checked against **a6f8909c**’s reverse-PRIME spec. The sink move was located at **eaf47374**, but no referenced design document was found. Remaining cursor-transition and lifecycle provenance chains were not fully closed.

**Reading budget:** 24/24 general excerpts; 12/12 provenance excerpts. Investigation stopped at the limit. Follow-up scope is the non-Present invalidation path and the no-out-fence GPU-reader/lifecycle reuse proof; neither is established as sound here.