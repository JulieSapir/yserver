## Verdict

**1 blocking, 2 major, 1 minor**

**Coverage: INCOMPLETE**

This is a design-review result. It does not establish compilation, passing tests, or implementation approval.

References below: **T** = [target spec](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/specs/2026-10-14-tearing-on-demand-design.md), **D** = [authoritative direct-scanout spec](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/specs/2026-08-05-present-direct-scanout-design.md). References include line numbers.

## Incorporation audit

| Prior findings | Status |
|---|---|
| None; this is the first review. | Check 1 skipped as instructed. |

## Findings

### Blocking

**B-1 — The proposed cursor transition cannot escape held direct scanout**

T:93–101 relies on the scene tick assigning `Sw`, which then makes the direct gate fail. But consider an already directly scanned window sending its first qualifying Async Present with a visible hardware cursor. In [scanout.rs:2165](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver/src/kms/render/backend/kms/scanout.rs:2165), the current `Hw` mode requests no unflip, and `hold_direct` returns before the scene tick at lines 2190–2212. Disabling a strategy inside that tick cannot cause the tick to become reachable. The primary-only async flip can succeed while the hardware cursor remains bound, violating T:35–36.

There is another entrance to the same violation: after hidden-cursor direct tearing, XFIXES ShowCursor directly rebinds the hardware plane through [cursor.rs:484–492](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver/src/kms/render/backend/kms/cursor.rs:484), bypassing the proposed scene strategy.

**Required correction:** define an explicit handoff before enabling composed async flips: request the grouped unflip, materialize the retained direct source into the COW, wait for replacement retirement, and drive cursor detachment. Gate direct cursor binding/show/refresh during this handoff and while tearing.

D:184–190 and 222–231 require retaining direct ownership until replacement retires. Cursor detachment also needs success proof: commit `a6f8909c`, [docs/status.md:1059–1062](/home/ariel_santangelo/Projects/yserver-tearing/docs/status.md:1059), states “successful hide permits SW only on the following frame.” The existing hide runs **at retirement**, through a separately fallible operation ([scene/cursor.rs:529–533](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver-render/src/kms/render/scene/cursor.rs:529)). One accepted vsync frame is therefore insufficient proof.

Tests must start from held direct scanout and cover hide failure and hidden→visible transitions.

### Major

**M-1 — Probation has no ownership contract compatible with composed tearing**

T:74–85 includes existing probation in tearing eligibility, while T:95–99 deliberately makes actual direct eligibility fail for software cursors.

The existing admission counter advances only after the real direct gate succeeds. An ineligible authoritative-root Present resets it before admission ([present.rs:738–785](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver/src/kms/render/backend/kms/present.rs:738)). Once tearing requires software composition, that path resets probation on successive Presents. For an initially software-rendered cursor, it never accumulates admission at all.

Consequently, reusing the current counter makes entry unreachable or destroys admission during composed tearing. Bypassing it would silently discard its original purpose. Commit `9c4ef8f0`, [M2 plan:117–120](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-08-05-present-direct-scanout-m2.md:117), requires eight consecutive eligible root Presents and explains the reset: alternating region-limited/full-root compositor frames must not cause repeated entry/unflip.

**Required correction:** specify who advances and resets tearing probation, independently of the actual cursor-dependent direct gate; define whether probation applies only on entry. Preserve resets for genuinely ineligible authoritative successors. Test initial SW mode, sustained composed tearing, and alternating full/region-limited Presents.

**M-2 — Composed Present timestamps promise an event without an association contract**

T:163 says composed frames retain today’s `Copy` behavior, but T:164–165 says MSC/UST come from the async flip event.

Today, Copy completion entries attach to paint work ([present.rs:1370–1427](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver/src/kms/render/backend/kms/present.rs:1370)); their backend readiness follows GPU completion, without identifying a corresponding scene flip ([present.rs:154–181, 247–260](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver/src/kms/render/backend/kms/present.rs:154)).

Concrete sequence: Copy A and Copy B finish while another CRTC commit is pending; the eventual scene frame contains B and produces one async event. The spec does not decide which event timestamps A, whether both share that event, or whether existing Copy completion timing remains authoritative. A planner would have to invent this contract. Any new waiting scheme must also preserve per-window ordering across Copy, Skip, Flip and unflip, required by D:191.

**Required correction:** either limit the event-derived timestamp promise to direct flips and explicitly preserve Copy completion semantics, or define Copy-to-scene-event association, coalescing and cancellation. Add a test with multiple ready Copies and one scene flip, including direct→Copy ordering.

### Minor

**m-1 — The exit assertion requires an unreachable HW state**

T:103–111 includes DPMS off, VT release and loss of eligibility among exits, yet T:210–211 requires the cursor plane to be back in HW mode after leaving.

HW restoration cannot occur while KMS submission is disabled ([scanout.rs:2142–2156](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver/src/kms/render/backend/kms/scanout.rs:2142)). A transformed topology requires software cursor policy ([scene/cursor.rs:545–549](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver-render/src/kms/render/scene/cursor.rs:545)); an XFIXES-hidden cursor should remain hidden.

**Required correction:** require restoration of the normal cursor policy appropriate to the exit state. Parameterize exit tests for HW, SW, Hidden and suspended outputs.

## Coverage and implementation checks

| Check | Coverage and limitations |
|---|---|
| **1. Incorporation** | No prior review; skipped. |
| **2. Architecture/contracts** | Checked direct admission, held-frame arbitration, grouped unflip, scene strategy/assignment, atomic submission and capability production. Source files match master `a1fa1df6`; the workspace adds only the target document. Cursor and probation integration defects are above. |
| **3. Safety/ownership/failure** | Checked authoritative COW materialization, whole-output replacement and retention requirements, retire-time cursor actions, and atomic rejection attribution. Full pool recycling with no out-fence, copied transport and teardown were not established. |
| **4. Compliance/evidence** | Backend-driven recording tests and mutation requirements are useful, but do not presently specify the ordering/failure evidence identified above. Kernel source confirms capability rejection and restrictions on changed properties. Its non-primary hook check is conditional on a changed value (`drm_atomic_uapi.c:1269–1285`), narrower than T:60’s wording; this does not invalidate the conservative cursor-omission decision. Driver-specific restrictions and hardware acceptance remain unverified. No builds, tests or experiments ran. |
| **5. Provenance** | Traced cursor states/strategy to `e118bc3e` and the atomic-cursor plan, and two-phase ownership to `a6f8909c` plus status lines 1059–1062. Traced probation/unflip to `9c4ef8f0`, the M2 plan and D. Traced capability fields to `363dd7d7` and the Phase 4.2 spec; T explicitly changes advertisement. Traced commit kinds/attribution to `e118bc3e`/`d83df86a`; cursor-free Async preserves the attribution boundary. Traced BO phases/fences to `51e0612d` and the Phase 4.1 spec. Searched production producers for cursor states, probation, capabilities, commit kinds and fence transitions. Exhaustive BO producer/recycling coverage remains incomplete. The later no-fence change `e0b8a206` changed no design/status document; its rationale is not established. |

**Reading budget:** target read once; **24/24** regular excerpts and **12/12** provenance excerpts. Investigation stopped at exhaustion.

**Specific unresolved scope:** establish that async `out_fence_fd = -1` preserves replacement-event ownership and GPU-reader completion before BO reuse, including copied-scanout and lifecycle reset. This ground is unverified.

Implementation must run the real compiler/tests, `cargo clippy --all-targets -- -D warnings`, nightly formatting, and Linux glibc/musl plus FreeBSD portability gates. Hardware must establish actual async acceptance and retirement behavior.