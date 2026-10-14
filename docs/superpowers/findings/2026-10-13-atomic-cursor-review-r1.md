# Atomic cursor plan — codex review round 1 (2026-10-13)

Plan revision reviewed: f263c9ac (NVIDIA-only). Result below; all findings applied in the next plan revision.

## Verdict

**1 blocking, 2 major, 0 minor**

Coverage: COMPLETE FOR DECLARED SCOPE

This is a bounded design review, not an implementation approval.

## Incorporation audit

| Prior findings | Status |
|---|---|
| None | First review; check 1 skipped. |

## Findings

### Blocking

**B-1 — Grouped commits cannot receive every CRTC’s cursor position**

The architecture requires cursor positions to ride every direct and composed-unflip commit on their respective CRTCs ([plan/spec L9–13](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:9)). Task 2 supplies each submit function with only one `Option<&CursorPlanePosition>`, whose descriptor contains one CRTC and one plane ([plan L71–72](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:71), [L93–96](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:93)).

The existing direct path builds a transaction spanning all outputs ([scanout.rs L519–535](/home/ariel_santangelo/Projects/yserver-atomic-cursor/crates/yserver/src/kms/render/backend/kms/scanout.rs:519)); both direct submission and composed replacement build one atomic request containing multiple output planes ([modeset.rs L1581–1692](/home/ariel_santangelo/Projects/yserver-atomic-cursor/crates/yserver/src/drm/modeset.rs:1581)). Direct mode also binds cursors on every output ([scanout.rs L397–411](/home/ariel_santangelo/Projects/yserver-atomic-cursor/crates/yserver/src/kms/render/backend/kms/scanout.rs:397)).

Concrete failure: a two-CRTC direct transaction is pending; motion updates both cursor positions. Its successor can receive only one descriptor, so one cursor remains at its previous position during that successor. This violates the next-commit requirement. It is an information-cardinality defect, not a Rust signature complaint.

**Required correction:** grouped submitters receive positions for every participating visible cursor plane. Consume pending positions only for descriptors included in a successful request. Add a two-CRTC test with different output origins that checks both planes’ property triples.

### Major

**M-1 — Waiting behind a cursor commit lacks a direct Present continuation**

Task 3 says direct submission waits behind cursor-only commits, but specifies no retention and resumption contract for the waiting Present. Its scheduling rule discusses choosing between cursor-only and scene submission ([plan L112–121](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:112)). The direct-blocker test covers motion arriving behind a direct successor, not the reverse ordering ([plan L134–135](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:134)).

Today, eligible successors are queued when a **direct frame** is pending ([present.rs L834–837](/home/ariel_santangelo/Projects/yserver-atomic-cursor/crates/yserver/src/kms/render/backend/kms/present.rs:834)). The queue is submitted by direct-frame retirement; that handler returns immediately when no direct frame is pending ([scanout.rs L906–907, L949–953](/home/ariel_santangelo/Projects/yserver-atomic-cursor/crates/yserver/src/kms/render/backend/kms/scanout.rs:906)). Composition can also return while the current direct frame remains held ([scanout.rs L2048–2054](/home/ariel_santangelo/Projects/yserver-atomic-cursor/crates/yserver/src/kms/render/backend/kms/scanout.rs:2048)).

Concrete gap: a direct frame has fully retired; a cursor-only commit starts; an eligible Present arrives and must wait. Reusing the existing successor queue retains the Present and its pins, but cursor retirement has no specified transition that submits it. There may be no subsequent direct event to release the wait.

**Required correction:** define who retains that candidate, its pins and completion wake, and which scheduling transition re-executes it after all participating cursor blockers clear. Pending direct work must participate in arbitration before another cursor-only submission. Test this cursor-first ordering through Backend entries and verify eventual Present completion and pin retirement.

**M-2 — Rejected cursor submissions do not exercise the new retirement gate**

The plan records commit results and prescribes failure handling, but none of the listed tests injects a rejected cursor-only request ([plan L41–42](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:41), [L73–77](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:73), [L127–140](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:127)). Hardware acceptance requires EBUSY to remain zero, so it cannot establish rejection recovery ([plan L171–173](/home/ariel_santangelo/Projects/yserver-atomic-cursor/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:171)).

Concrete undetected failure: bookkeeping marks a cursor commit in flight before an ioctl returns EBUSY. The rejected request produces no retirement event; the new shared gate then prevents scene and direct submission indefinitely. The successful-commit tests can all pass. Existing legacy move handling retains pending motion on EBUSY and handles disabling errors through hide rollback ([cursor.rs L615–646](/home/ariel_santangelo/Projects/yserver-atomic-cursor/crates/yserver/src/kms/render/platform/cursor.rs:615)); the new event-owned gate needs additional proof.

**Required correction:** make rejection outcomes scriptable and explicitly require in-flight state to represent accepted requests only. Add a Backend-driven rejection test asserting no phantom retirement dependency, the specified pending-position/fallback outcome, and subsequent presentation progress without a synthetic event or retry loop.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** skipped because no prior review exists.
- **Architecture:** assessed the 3b scene readiness boundary, per-device cursor ownership, grouped direct/unflip producers, copied-output sink routing, and retirement scheduling. B-1 and M-1 identify missing cross-task contracts.
- **Safety and failure semantics:** checked device-qualified old-event correlation, bounded draining and fail-stop handling, VT hide-before-discard ordering, scene acknowledgements, and direct-frame retention. The plan expressly preserves cursor bookkeeping until discard. Rejected-submit evidence remains insufficient as described in M-2.
- **Specification and verification:** checked the listed motion, topology, hidden-cursor and PRIME obligations against the proposed evidence. Formatting, clippy, workspace/integration tests, hardware validation and FreeBSD CI are assigned to implementation. Deterministic VT-release ordering is not separately tested; Task 5 supplies a manual restoration check.

**Source excerpts used: 23/24**, each at most 120 lines; the target plan was read once. Two initial locator outputs exceeded the requested 40-line search limit.

External spike artifacts, hardware scripts and physical driver behavior were not assessed and are not declared sound. No builds, tests, benchmarks, installs or compilation experiments were run. Rust/API correctness, actual test outcomes, portability builds and measured hardware acceptance remain implementation checks.

--- paste into the findings document, under the Result line ---

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `yserver-review` @ `111490f`;
model `gpt-6.1-sol`; reasoning effort `xhigh`; `codex-cli 0.160.0`.
Counts are comparable only to other reviews citing this same instrument SHA.
