## Verdict

**0 blocking, 0 major, 0 minor**

**Coverage: INCOMPLETE**

No new design defect was demonstrated in the assessed paths. The unresolved ownership and provenance scope below prevents treating this as a clean review. This result establishes neither compilation, passing tests, nor implementation approval.

References: **T** = [revised tearing spec](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/specs/2026-10-14-tearing-on-demand-design.md); **D** = [authoritative direct-scanout spec](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/specs/2026-08-05-present-direct-scanout-design.md). Numbers following these abbreviations are line numbers.

## Incorporation audit

| Prior finding | Status | Assessment |
|---|---|---|
| B-1: Cursor transition cannot escape held direct scanout | **APPLIED** | T:103–123 specifies an explicit grouped unflip, COW materialization, retained direct ownership, successful-hide proof, and suppression of cursor rebind paths. T:265–275 assigns held-direct entry, hide-failure and hidden→visible evidence. |
| M-1: Probation incompatible with composed tearing | **APPLIED** | T:125–140 gives tearing an independent eight-Present counter, defines its resets and entry-only application, and preserves genuine eligibility failures and direct re-entry probation. T:276–281 assigns the relevant scenarios. |
| M-2: Composed timestamps lack an event association | **APPLIED** | T:208–214 limits event-derived timestamps to direct flips and explicitly preserves GPU-copy readiness for Copy completions. T:282–283 covers coalesced Copies and direct→Copy ordering. |
| m-1: Exit assertion requires unreachable HW restoration | **APPLIED** | T:148–155 defines restoration according to HW, SW, Hidden or suspended policy. T:284–286 parameterizes the exit assertion accordingly. |

These are incorporated **design contracts**, not claims that their implementation already exists.

## Findings

### Blocking

None demonstrated.

### Major

None demonstrated.

### Minor

None demonstrated.

## Coverage and implementation checks

| Check | Coverage and limitations |
|---|---|
| **1. Incorporation** | Assessed all four prior findings against the revised requirements and proposed evidence. No unresolved prior design finding was carried forward. |
| **2. Architecture and contracts** | Verified cursor-dependent admission, held-direct arbitration, grouped unflip and retirement, scene pending-commit gates, capability encoding and commit-kind producers. Source changes relative to `a1fa1df6` are confined to the review/spec documents. Composed pacing remains explicitly assigned to implementation at T:159–164 and 319–320; the complete wake/deadline integration was not established. |
| **3. Safety, ownership and failures** | Checked D:184–191’s retention and completion ordering, D:198–211’s grouped retirement, and D:222–231’s unflip requirements. Current code frees the previous pool BO at a replacement event ([flip_events.rs:564](/home/ariel_santangelo/Projects/yserver-tearing/crates/yserver/src/kms/render/platform/flip_events.rs:564)); copied destinations additionally record that replacement boundary. Shared lifecycle reset waits for device idle; copied reset quiesces both devices. These observations do **not** establish every GPU reader, reset/error path or stale-event handoff. |
| **4. Compliance and verification** | The revised Backend-driven tests address the prior ordering and failure gaps. Local DRM source confirms capability rejection and the permitted changing primary-plane properties ([drm_atomic_uapi.c:1269](/home/ariel_santangelo/Projects/linux/drivers/gpu/drm/drm_atomic_uapi.c:1269), [1641](/home/ariel_santangelo/Projects/linux/drivers/gpu/drm/drm_atomic_uapi.c:1641)). T:60 overstates the non-primary hook requirement: the UAPI consults it only for a changed permitted property. This does not undermine the conservative cursor-omission contract. Driver-specific acceptance claims and hardware retirement behavior remain unverified. |
| **5. Provenance** | Traced the principal changed gates, states and classifications as detailed below. Producer searches were performed, but complete definition-to-producer assessment was not achieved for every cursor recovery and BO ownership path. Missing document provenance is identified explicitly; it is not treated as sound. |

Provenance assessed:

- **Cursor assignments, aggregate modes and retirement transitions:** `a6f8909c` and [docs/status.md:1057](/home/ariel_santangelo/Projects/yserver-tearing/docs/status.md:1057) require actual ownership tracking and software reveal only after successful hide. T:103–123 preserves that requirement. `e118bc3e`’s [atomic-cursor plan:10](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-10-13-atomic-cursor-wlroots-model.md:10) requires one commit stream per CRTC. T:182–206 explicitly removes cursor properties from async requests while retaining serialization. Complete recovery-producer coverage remains unfinished.
- **Direct admission and probation:** `9c4ef8f0`, [M2 plan:117](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/plans/2026-08-05-present-direct-scanout-m2.md:117), explains the eight-frame probation as protection against alternating region-limited/full-root thrash. T:125–140 knowingly separates tearing probation while preserving that purpose.
- **BO phases and release fences:** `51e0612d`, [Phase 4.1 spec:166](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/specs/2026-05-07-phase4-1-vulkan-compositor-design.md:166), requires replacement retirement and completion of GPU readers before reuse. T:216–220 retains those requirements explicitly; their full execution proof remains open. `e0b8a206` changed no design/status document, so its documented rationale was unavailable.
- **Copied transport:** `a6f8909c`, [reverse-PRIME spec:291](/home/ariel_santangelo/Projects/yserver-tearing/docs/superpowers/specs/2026-07-14-reverse-prime-copy-design.md:291), preserves producer synchronization separately from KMS retirement. The inspected copied submission/retirement paths maintain that distinction. Complete ownership-enum producer tracing remains unfinished.
- **PresentCaps and atomic classification:** traced to `363dd7d7`’s Phase 4.2 spec and `e118bc3e`’s atomic-cursor plan. T:224–227 explicitly changes advertisement independently of `flip_path`. The later attribution correction `d83df86a` changed no design/status document; document provenance for that correction was unavailable.

**Reading budget:** target and prior review read once; **24/24 regular excerpts, 12/12 provenance excerpts**. Investigation stopped at exhaustion. One provenance excerpt inadvertently exceeded the 120-line bound: 140 lines.

**Specific unresolved question:** does shared/copied BO recycling preserve replacement-event ownership **and every GPU-reader/quiescence requirement** when `release_fence_fd=None`, including lifecycle reset and failure paths?

Implementation still requires the real compiler/tests, `cargo clippy --all-targets -- -D warnings`, `cargo +nightly fmt`, Linux glibc/musl and FreeBSD portability checks, and the specified hardware validation. No builds, tests, installs or experiments ran.