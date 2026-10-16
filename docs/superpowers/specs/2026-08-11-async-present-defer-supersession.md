# Async Present Target Identity + Supersession Spec (delta)

Date: 2026-08-11. Status: **Amended after Warframe hardware validation on
2026-08-31.**

Related findings:
`docs/superpowers/findings/2026-08-11-cs2-fullscreen-novsync-pageflip-collapse.md`
(§4a, §4b).

## Problem

The original Phase A treated a clocked immediate async Present as if it had no
target: `effective_present_target_raw` discarded any effective MSC that was not
strictly after the current CRTC MSC. `classify_msc_due(None, _, true)` then
parked the request behind an in-flight flip, while
`supersede_covered_pending_presents` correctly refused to scrap two requests
whose targets appeared unknown.

Warframe's real uncapped async stream exposed the resulting deadlock in flow
control. Direct scanout stayed stable and retired exactly at 60 Hz, but all
async successors remained in core until each retirement. `present_skips`
stayed zero, the backend's bounded latest-wins successor slot received no work,
and the client exhausted its buffers and became vblank-limited at exactly
60 FPS instead of its normal roughly 200 FPS.

Xorg does not erase this identity. For an immediate async request with a valid
clock and `divisor == 0`, `present_get_target_msc` returns the current CRTC MSC.
Its scrap loop can therefore compare CRTC and target MSC even though the
request is due immediately.

## Design

### 1. Preserve the Xorg target for every clocked Present

`effective_present_target_raw` returns `Some(effective_target_msc)` whenever
the selected domain has a usable raw MSC. This includes the current MSC for an
immediate async request. `None` is reserved for a genuinely unclocked domain
such as headless, an Off CRTC, or pre-first-flip KMS.

Target identity and due state are distinct concepts: `Some(current_msc)` is a
known target that is already due, not an instruction to defer.

### 2. Never vblank-park an unclocked request

`classify_msc_due(None, _, _)` returns `ExecuteNow`. There is no clock against
which such a request could become due.

A clocked immediate async request enters as `Some(current_msc)`. The existing
`eff <= clock_msc` rule executes it immediately regardless of whether a flip
is in flight. Ready requests consequently reach `try_present_direct`, where
the backend retains one latest successor and immediately releases superseded
buffers. Requests still waiting for their acquire/source fence remain in core
until that fence signals and are never scrapped while waiting. Core
supersession scraps only requests parked for a future MSC. If a newer covered
same-target Present has executed by then, the older request completes as Skip
instead of being shown after it.

Synced next-vblank requests remain `Some(clock_msc + 1)` and retain the
existing in-flight-flip parking rule.

### 3. Keep supersession target-scoped

A successor may scrap a covered pending predecessor only when all of these are
equal:

- window;
- CRTC and CRTC epoch;
- known `effective_target_msc` (`Some(target)`).

The async option bit is not a grouping key. Different targets never
supersede, and `None`/`None` does not establish target equivalence. The
existing conservative coverage predicate remains an additional gate.

## Out of scope

- More than one hardware flip in flight per CRTC.
- Expanding the backend successor queue beyond its bounded latest-wins slot.
- GetImage / MIT-SHM readback latency.

## Acceptance

1. A clocked immediate async request retains `Some(current_msc)`.
2. It reaches the backend even while the preceding flip is in flight.
3. Same-target pending requests can supersede and release their buffers
   immediately; different or unknown targets cannot.
4. A genuinely unclocked request executes immediately.
5. The backend keeps exactly one direct successor and preserves ordered
   `Flip`/`Skip` completion delivery.
6. Hardware: direct scanout remains stable at refresh while an uncapped async
   client remains uncapped rather than collapsing to refresh.
7. CI: `cargo clippy --all-targets -- -D warnings` is clean.
