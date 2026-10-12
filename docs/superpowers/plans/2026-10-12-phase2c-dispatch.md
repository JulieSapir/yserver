# Phase 2c proposal: split `handle_xi2_request` / `handle_randr_request` by minor

Step 2c of `2026-10-08-source-layout-cleanup.md` ("split the arms … into
per-minor fns, `xi2/` into `{events, devices, grabs, props, focus}`; reviewed
as a refactor; HW XI smoke on eiger"). Branch `refactor/phase2c-dispatch`.
**Executed 2026-10-12** on that branch (results and final trees below;
xts XI/XIproto before/after and the eiger HW smoke pending); proposal
revised after codex review (tail params, binding check, drop order,
splitter gate, xts baseline; decisions below). Inventory measured on master
`4fdef034` with a
throwaway syn visitor over each match arm (not committed) and llvm-cov line
coverage of the yserver-core lib tests (1,683 tests, default features).

## Structure today

### `xi/dispatch.rs` (5,140 lines, one fn)

- **Signature** (3–11): `state, backend, origin, client_id, sequence, header,
  body` → `io::Result<RequestOutcome>`.
- **Preamble** (12–68): fn-local `use …::ClientByteOrder`; `let byte_order`,
  `let minor = header.data`; three BadLength gates (`return`); `let mut buf`.
- **`match minor`** (69–5134): 62 arms; 58 inline bodies (4,793 lines: 39 XI1
  arms 3,134 lines, 19 XI2 arms 1,659 lines). Four already delegate or error
  and stay: 41 → `focus_pointer::handle_xi_warp_pointer`, 43/49 →
  `xi/mod.rs` handlers, `_` → BadRequest.
- **After the match** (5135–5139): look the client up (`return Ok(Handled)`
  if gone), dead `let _byte_order`, `Ok(write_to_client(client, client_id,
  &buf))`. The match is a statement; its value is never used.
- **Control flow:** 13 arms end in a tail `return` (two of them, 37 and 57,
  in a tail `match` that returns in every branch); 45 can fall through (33
  write `buf`, 12 void requests write an empty `buf`, which still runs
  `write_or_buffer` and the telemetry); `?` in 5 arms (22, 24, 46, 48, 60),
  same return type as the fn. No `break`/`continue` leaves an arm, no labels.
- **Outer names used** (arms of 62): `client_id` 62, `sequence` 62, `state`
  60, `body` 57, `byte_order` 37, `minor` 35, outer `buf` 33, `backend` 14,
  `origin` 9, `header` 3 (`.length_units` in 1, `.opcode` in 46 and `_`),
  `ClientByteOrder` 1 (arm 48, from the fn-local `use`). No arm rebinds one
  (only closure params: `|state: &ServerState, ..|` in 19, `|buf, seq,
  order|` in 27 and 31: every `buf` in 31 is that closure param, so 31 does
  not touch the outer `buf`; 27 uses both). Outer `buf` forms:
  `buf.extend_from_slice` (30 arms) and `&mut buf` passed to helpers (11,
  22, 24, 25, 29); never assigned or moved.
- **Arm-local items** (move with the body): consts in 2, 3 (6), 5, 10, 31,
  33, 35 (`ALREADY_GRABBED` three times); a `use crate::core_loop::
  pointer_fanout::{…}` in 19.
- **Comments:** 251 lines between arms, each a lead-in to the next arm,
  plus two group headers (before 51: the XI2 grab block, 26 lines; before 4:
  "XI 1.x minors with request validation", 10 lines).
- **Logs / location:** 46 log calls, none with `target:`; no `line!`,
  `file!`, `Location`, `module_path!`, `macro_rules!`.
- **Unit coverage gap:** 16 extracted arms (1,190 lines) are never entered
  by the lib tests: 4, 5, 8, 9, 11, 13–19, 20, 26, 27 (XI1) and 44 (XI2).
  The XI1 ones are exercised only by xts `XI`/`XIproto` (xts5 has no XI2).

Params column: `be` backend, `o` origin, `h` header, `b` body, `bo`
byte_order, `m` minor (`state, client_id, sequence` implied unless noted).

| minor | request | span | lines | → file | params | end | `?` | unit cov: entered× / lines hit |
|---|---|---|---|---|---|---|---|---|
| 1 | GetExtensionVersion | 70–100 | 31 | devices | h b bo | falls through, writes buf |  | 1 / 12/16 |
| 2 | ListInputDevices | 1716–1822 | 107 | devices | bo | falls through, writes buf |  | 21 / 74/75 |
| 3 | OpenDevice | 2318–2421 | 104 | devices | b bo m | falls through, writes buf |  | 1 / 23/39 |
| 4 | CloseDevice | 2433–2449 | 17 | devices | b m | falls through, void (empty write) |  | 0 / 0/10 |
| 5 | SetDeviceMode | 2452–2496 | 45 | devices | b bo m | falls through, writes buf |  | 0 / 0/31 |
| 6 | SelectExtensionEvent | 1853–2022 | 170 | events | b m | return |  | 24 / 81/111 |
| 7 | GetSelectedExtensionEvents | 2500–2546 | 47 | events | b bo m | falls through, writes buf |  | 5 / 35/41 |
| 8 | ChangeDeviceDontPropagateList | 2549–2621 | 73 | events | b m | falls through, void (empty write) |  | 0 / 0/54 |
| 9 | GetDeviceDontPropagateList | 2630–2662 | 33 | events | b bo m | falls through, writes buf |  | 0 / 0/28 |
| 10 | GetDeviceMotionEvents | 2666–2717 | 52 | control | b bo m | falls through, writes buf |  | 4 / 35/43 |
| 11 | ChangeKeyboardDevice | 2724–2773 | 50 | devices | b bo m | falls through, writes buf |  | 0 / 0/41 |
| 12 | ChangePointerDevice | 2776–2778 | 3 | devices | m | return |  | 1 / 1/1 |
| 13 | GrabDevice | 2782–2918 | 137 | grabs | b bo m | falls through, writes buf |  | 0 / 0/107 |
| 14 | UngrabDevice | 2921–2958 | 38 | grabs | b m | falls through, void (empty write) |  | 0 / 0/26 |
| 15 | GrabDeviceKey | 2965–3095 | 131 | grabs | b m | falls through, void (empty write) |  | 0 / 0/108 |
| 16 | UngrabDeviceKey | 3098–3175 | 78 | grabs | b m | falls through, void (empty write) |  | 0 / 0/62 |
| 17 | GrabDeviceButton | 3179–3302 | 124 | grabs | b m | falls through, void (empty write) |  | 0 / 0/98 |
| 18 | UngrabDeviceButton | 3305–3372 | 68 | grabs | b m | falls through, void (empty write) |  | 0 / 0/54 |
| 19 | AllowDeviceEvents | 3375–3604 | 230 | grabs | be b m | falls through, void (empty write) |  | 0 / 0/158 |
| 20 | GetDeviceFocus | 3611–3640 | 30 | focus | b bo m | falls through, writes buf |  | 0 / 0/17 |
| 21 | SetDeviceFocus | 3647–3712 | 66 | focus | b m | falls through, void (empty write) |  | 2 / 23/45 |
| 22 | GetFeedbackControl | 3720–3756 | 37 | control | b bo m | falls through, writes buf | 1 | 2 / 21/27 |
| 23 | ChangeFeedbackControl | 3762–3964 | 203 | control | b m | falls through, void (empty write) |  | 3 / 87/161 |
| 24 | GetDeviceKeyMapping | 3967–4011 | 45 | control | be o b bo m | falls through, writes buf | 1 | 3 / 14/32 |
| 25 | ChangeDeviceKeyMapping | 4014–4116 | 103 | control | be b bo m | falls through, writes buf |  | 1 / 34/73 |
| 26 | GetDeviceModifierMapping | 4132–4172 | 41 | control | be o b bo m | falls through, writes buf |  | 0 / 0/29 |
| 27 | SetDeviceModifierMapping | 4175–4265 | 91 | control | be o b bo m | falls through, writes buf |  | 0 / 0/62 |
| 28 | GetDeviceButtonMapping | 4270–4313 | 44 | control | b bo m | falls through, writes buf |  | 27 / 24/31 |
| 29 | SetDeviceButtonMapping | 4317–4414 | 98 | control | b bo m | falls through, writes buf |  | 32 / 45/77 |
| 30 | QueryDeviceState | 4428–4495 | 68 | control | b bo m | falls through, writes buf |  | 6 / 44/58 |
| 31 | SendExtensionEvent | 4498–4698 | 201 | events | b bo m | falls through, void (empty write; its `buf` is the fanout closure's param) |  | 8 / 71/102 |
| 32 | DeviceBell | 4704–4756 | 53 | control | b m | return |  | 2 / 22/40 |
| 33 | SetDeviceValuators | 4760–4832 | 73 | control | b bo m | falls through, writes buf |  | 4 / 35/48 |
| 34 | GetDeviceControl | 4848–4911 | 64 | control | b bo m | falls through, writes buf |  | 8 / 39/52 |
| 35 | ChangeDeviceControl | 4915–5037 | 123 | control | b bo m | falls through, writes buf |  | 2 / 63/89 |
| 36 | ListDeviceProperties | 2029–2052 | 24 | props | b bo | falls through, writes buf |  | 2 / 13/14 |
| 37 | ChangeDeviceProperty | 2058–2143 | 86 | props | be b bo | return (tail match) |  | 7 / 31/50 |
| 38 | DeleteDeviceProperty | 2148–2213 | 66 | props | b | return |  | 1 / 22/43 |
| 39 | GetDeviceProperty | 2224–2303 | 80 | props | b bo | falls through, writes buf |  | 6 / 37/54 |
| 40 | XIQueryPointer | 739–864 | 126 | focus | be o b bo | falls through, writes buf |  | 20 / 44/46 |
| 41 | XIWarpPointer | 5113–5115 | 3 | kept in dispatcher | be o b | return |  | 26 / 1/1 |
| 42 | XIChangeCursor | 101–147 | 47 | focus | be o b | return |  | 2 / 18/22 |
| 43 | XIChangeHierarchy | 5116–5118 | 3 | kept in dispatcher | b bo | return |  | 25 / 1/1 |
| 44 | XISetClientPointer | 148–151 | 4 | devices | (no `state`) | return |  | 0 / 0/2 |
| 45 | XIGetClientPointer | 152–190 | 39 | devices | bo; `state` from the tail only | falls through, writes buf |  | 1 / 8/8 |
| 46 | XISelectEvents | 191–294 | 104 | events | be o h b | return | 1 | 47 / 66/77 |
| 47 | XIQueryVersion | 295–329 | 35 | devices | b bo | falls through, writes buf |  | 17 / 21/26 |
| 48 | XIQueryDevice | 330–394 | 65 | devices | b bo | falls through, writes buf | 1 | 11 / 42/49 |
| 49 | XISetFocus | 5119–5119 | 1 | kept in dispatcher | b | return |  | 14 / 1/1 |
| 50 | XIGetFocus | 5038–5070 | 33 | focus | b bo m | falls through, writes buf |  | 9 / 14/15 |
| 51 | XIGrabDevice | 891–1202 | 312 | grabs | be o b bo | falls through, writes buf |  | 32 / 182/190 |
| 52 | XIUngrabDevice | 1203–1359 | 157 | grabs | be o b | return |  | 16 / 98/112 |
| 53 | XIAllowEvents | 1360–1438 | 79 | grabs | be b | return |  | 15 / 24/33 |
| 54 | XIPassiveGrabDevice | 1439–1612 | 174 | grabs | b bo | falls through, writes buf |  | 21 / 94/106 |
| 55 | XIPassiveUngrabDevice | 1613–1710 | 98 | grabs | b | return |  | 8 / 48/63 |
| 56 | XIListProperties | 395–420 | 26 | props | b bo | falls through, writes buf |  | 2 / 13/14 |
| 57 | XIChangeProperty | 421–515 | 95 | props | be b bo | return (tail match) |  | 69 / 38/50 |
| 58 | XIDeleteProperty | 516–586 | 71 | props | b | return |  | 4 / 24/45 |
| 59 | XIGetProperty | 587–695 | 109 | props | b bo | falls through, writes buf |  | 8 / 43/59 |
| 60 | XIGetSelectedEvents | 696–738 | 43 | events | b bo | falls through, writes buf | 1 | 6 / 32/33 |
| 61 | XIBarrierReleasePointer | 5071–5112 | 42 | focus | b | falls through, void (empty write) |  | 1 / 10/26 |
| _ | _ (BadRequest) | 5122–5133 | 12 | kept in dispatcher | h m | return |  | 5 / 7/7 |

### `randr_ext.rs` (2,900 lines)

- **Lines 1–120:** `PendingCrtcConfig`, `CrtcConfigCompletion`,
  `CrtcConfigReply`; `active_monitors`, `send_root_config_notify`, the
  unsupported-request warn pair.
- **`handle_randr_request`** (121–2393): 6 params (no `origin`). Preamble
  129–201 holds fn-local **items**: `use {ClientByteOrder, randr as
  x11randr}`, `const RANDR_MAJOR_OPCODE`, and 6 helper fns
  (`crtc_is_leased`, `provider_relationship_protocol_error`, `request_xid`,
  `output_exists`, `crtc_exists`, `synthetic_output_property`), then `let
  byte_order`, `let minor`. `match minor` 202–2391, 39 arms, patterns are
  `x11randr::RR_*` consts. After the match: `Ok(RequestOutcome::Handled)`.
- **Control flow:** 36 of 39 arms end in a tail `return`; SelectInput and
  `other` fall through. No outer `buf` (arms build their own). No `?`, no
  escaping `break`/`continue`.
- **Fn-local items used by arms:** `RANDR_MAJOR_OPCODE` 34, `x11randr` 30,
  `request_xid` 12, `crtc_exists` 7, `output_exists` 7, 2 each for the rest.
  Arm-local items: `fn part` (GetCrtcTransform), `struct MonitorRow`
  (GetMonitors).
- **Lines 2394–2764:** `set_screen_config`, crtc-config start/complete/
  reply, encoder. **2765–2900:** `handle_xinerama_request` (its own
  fn-local `macro_rules! require_len`).
- **Logs:** 9 calls, none with `target:`. **Unit coverage:** every arm is
  entered except QueryVersion and `other`.

| minor | request | span | lines | → file | params | fn-local helpers used | end | unit cov: entered× / lines hit |
|---|---|---|---|---|---|---|---|---|
| 0 | QueryVersion | 203–231 | 29 | screen | b bo | — | return | 0 / 0/23 |
| 6 | GetScreenSizeRange | 232–254 | 23 | screen | b bo m | request_xid | return | 1 / 8/16 |
| 8 | GetScreenResources | 255–293 | 39 | screen | be h b bo m | request_xid | return | 1 / 8/24 |
| 25 | GetScreenResourcesCurrent | 294–316 | 23 | screen | b bo m | request_xid | return | 1 / 8/16 |
| 9 | GetOutputInfo | 317–368 | 52 | output | h b bo | — | return | 1 / 8/36 |
| 20 | GetCrtcInfo | 369–413 | 45 | crtc | h b bo | — | return | 37 / 26/32 |
| 26 | SetCrtcTransform | 414–480 | 67 | crtc | b m | crtc_exists crtc_is_leased | return | 29 / 47/49 |
| 27 | GetCrtcTransform | 481–529 | 49 | crtc | b bo m | request_xid crtc_exists | return | 7 / 30/32 |
| 10 | ListOutputProperties | 530–568 | 39 | output | be b bo m | request_xid output_exists | return | 3 / 20/27 |
| 11 | QueryOutputProperty | 569–634 | 66 | output | be b bo m | output_exists synthetic_output_property | return | 3 / 31/41 |
| 12 | ConfigureOutputProperty | 635–695 | 61 | output | b m | output_exists | return | 6 / 30/35 |
| 13 | ChangeOutputProperty | 696–873 | 178 | output | b m | output_exists | return | 22 / 79/105 |
| 14 | DeleteOutputProperty | 874–946 | 73 | output | b m | output_exists | return | 6 / 40/45 |
| 28 | GetPanning | 947–967 | 21 | crtc | b bo m | request_xid crtc_exists | return | 1 / 8/15 |
| 29 | SetPanning | 968–1017 | 50 | crtc | b bo m | crtc_exists | return | 4 / 31/32 |
| 30 | SetOutputPrimary | 1018–1066 | 49 | output | b m | request_xid output_exists | return | 8 / 24/30 |
| 31 | GetOutputPrimary | 1067–1087 | 21 | output | b bo m | request_xid | return | 2 / 14/15 |
| 32 | GetProviders | 1088–1127 | 40 | provider | b bo m | request_xid | return | 5 / 28/29 |
| 33 | GetProviderInfo | 1128–1182 | 55 | provider | b bo m | — | return | 5 / 39/40 |
| 34 | SetProviderOffloadSink | 1183–1228 | 46 | provider | b m | provider_relationship_protocol_error | return | 11 / 31/31 |
| 35 | SetProviderOutputSource | 1229–1291 | 63 | provider | be b m | provider_relationship_protocol_error | return | 17 / 41/41 |
| 42 | GetMonitors | 1292–1364 | 73 | monitor | b bo m | request_xid | return | 72 / 49/50 |
| 43 | SetMonitor | 1365–1425 | 61 | monitor | b m | — | return | 54 / 48/48 |
| 44 | DeleteMonitor | 1426–1458 | 33 | monitor | b m | — | return | 20 / 25/25 |
| 22 | GetCrtcGammaSize | 1459–1492 | 34 | crtc | be h b bo | crtc_exists | return | 3 / 16/22 |
| 23 | GetCrtcGamma | 1493–1524 | 32 | crtc | be h b bo | crtc_exists | return | 3 / 15/21 |
| 24 | SetCrtcGamma | 1525–1619 | 95 | crtc | be h b | crtc_exists crtc_is_leased | return | 256 / 49/66 |
| 15 | GetOutputProperty | 1620–1791 | 172 | output | be b bo m | output_exists synthetic_output_property | return | 7 / 92/120 |
| 4 | SelectInput | 1792–1815 | 24 | screen | b m | — | falls through | 1 / 8/19 |
| 5 | GetScreenInfo | 1816–1888 | 73 | screen | b bo m | request_xid | return | 3 / 51/63 |
| 7 | SetScreenSize | 1889–2004 | 116 | screen | be h b m | — | return | 16 / 62/73 |
| 2 | SetScreenConfig | 2005–2007 | 3 | kept in dispatcher | be b bo | — | return | 16 / 1/1 |
| 21 | SetCrtcConfig | 2008–2171 | 164 | crtc | be h b bo | — | return | 33 / 98/118 |
| 16, 45 | CreateMode, CreateLease | 2172–2193 | 22 | randr_ext.rs (root) | m | — | return | 2 / 5/5 |
| 36–41 | *ProviderProperty (36–41) | 2194–2320 | 127 | provider | b m | request_xid | return | 12 / 73/74 |
| 46 | FreeLease | 2321–2345 | 25 | randr_ext.rs (root) | b m | — | return | 1 / 10/10 |
| 1, 3 | unassigned (BadRequest) | 2353–2363 | 11 | kept in dispatcher | h m | — | return | 2 / 6/6 |
| ≥47 | other ≥ RANDR_REQUEST_COUNT (BadRequest) | 2364–2374 | 11 | kept in dispatcher | h | — | return | 1 / 7/7 |
| 17–19 | other (void no-op + warn) | 2375–2390 | 16 | kept in dispatcher | — | — | falls through | 0 / 0/13 |

## Mechanics (recommended)

**Shape: one self-contained fn per minor, in the same module first.**

```rust
fn handle_xi1_get_device_focus(
    state: &mut ServerState,
    client_id: ClientId,
    sequence: SequenceNumber,
    body: &[u8],
    byte_order: ClientByteOrder,
    minor: u8,
) -> io::Result<RequestOutcome> {
    let mut buf: Vec<u8> = Vec::with_capacity(64); // iff the arm uses buf
    {
        /* the arm's block, token-identical */
    }
    xi_write_reply(state, client_id, buf) // iff the arm can fall through
}
```

Three body forms: an arm that cannot fall through (13 XI, 35 RANDR) *is*
the fn body; a void fall-through arm is `{ <arm> } <tail>`; a buf arm is
`<prelude> { <arm> } <tail>`. The arm's locals drop at the end of its inner
block, before the tail runs, as today; `buf` lives outside it, as it did
outside the match.

- **Params** = the outer names used free by the arm **plus by the inserted
  tail** (`xi_write_reply(state, client_id, buf)` needs `state`,
  `client_id`), minus what the prelude binds (`buf`); same names, outer order
  (`state, backend, origin, client_id, sequence, header, body, byte_order,
  minor`), outer types (`byte_order: ClientByteOrder`, `minor: u8` for the
  two `let`s). All passed as today (`&mut` reborrows, `Copy` for header,
  origin, byte_order, minor). 2–8 params; `too_many_arguments` is allowed
  workspace-wide. Measured effect of the tail: only XIGetClientPointer (45)
  gains a param (`state`; the arm itself does not use it); it becomes a
  fixture. **Return type** = the dispatcher's, so every `return` and
  `?` keeps its meaning.
- **Dispatcher:** preamble unchanged except `let mut buf` goes; the match
  becomes the fn's tail; an extracted arm is `N => handle_…(args),`. Kept
  arms lose `return`/`;` (XI 41, 43, 49, `_`; RANDR SetScreenConfig, `1 |
  3`, the `>= RANDR_REQUEST_COUNT` guard); RANDR `other` gets the old tail
  `Ok(RequestOutcome::Handled)`.
- **XI tail:** the 3 post-match statements move verbatim into `fn
  xi_write_reply(state: &mut ServerState, client_id: ClientId, buf:
  Vec<u8>) -> io::Result<RequestOutcome>` (`&buf` stays as written; a
  `&[u8]` param would make it a clippy `needless_borrow`). The 12 void arms
  call `xi_write_reply(state, client_id, Vec::new())`, keeping today's
  empty write. **RANDR tail:** `Ok(RequestOutcome::Handled)` appended to
  SelectInput.
- **Comments:** each lead-in (and the two group headers) goes above the fn
  it precedes, still `//`.
- **Names:** XI2 `handle_xi_<name>` (as the existing `handle_xi_warp_pointer`
  / `_set_focus` / `_change_hierarchy`), XI1 `handle_xi1_<name>`, RANDR
  `handle_randr_<name>`; multi-minor arms `handle_randr_provider_property`
  (36–41), `handle_randr_create_mode_or_lease` (16, 45),
  `handle_randr_free_lease` (46). None exists today (measured).
- **Extract, then move:** the extract commit keeps every new fn in the
  dispatcher's module, so name resolution and log targets cannot change; the
  move into files is then a plain `split apply` (existing tool).
- **Drop order:** preserved by construction (the inner block above), not by
  an argument about which types implement `Drop`.

### Where a body cannot stay token-identical

| what | where | change | why |
|---|---|---|---|
| fn-local `use` / items | XI 48 (`ClientByteOrder`); 35 of 39 RANDR arm bodies, and every `x11randr::RR_*` pattern | prep commit hoists them to module scope; bodies then identical | a separate fn cannot see another fn's local items. Name check: none of the 8 names is defined elsewhere in the crate except a private `RANDR_MAJOR_OPCODE` in `nested.rs` (unreachable from here) |
| outer `buf` | 33 XI | prepend the removed `let mut buf…` verbatim | a local `buf` keeps `buf.extend_from_slice` and `&mut buf` (5 arms) identical; a `&mut Vec<u8>` param would turn `&mut buf` into `&mut &mut Vec<u8>` |
| fall-through | 45 XI, RANDR SelectInput | arm block kept as a block statement, tail appended after it | the old fall-through left the arm (dropping its locals) and continued into the post-match code |
| tail `return` | 13 XI, 35 RANDR (the arms that cannot fall through) | `#[allow(clippy::needless_return)]` on the fn; body identical | measured in a scratch crate: clippy flags a final `return x;` and returns in a tail match's arms; CI runs `-D warnings`. In a fall-through fn the arm's returns sit in a non-tail block, so no lint |
| kept arms | XI 4, RANDR 3 | drop `return`/`;` | the match is now the tail (same lint) |
| RANDR `other` | 1 | append `Ok(RequestOutcome::Handled)` | same as fall-through |

Nothing else: no escaping `break`/`continue`, no labels, no rebinding of an
outer name, no location-sensitive macro. The compiler settles each tail: a
missing one is a type error (`()` vs `io::Result`), a superfluous one is
`unreachable_code` under `-D warnings`. The syntactic count (45) can only
shrink.

**Rejected shapes:** `buf: &mut Vec<u8>` param plus an `Option<…>` return
(rewrites ~250 `return` sites); a request-context struct (`state` →
`req.state` at ~1,700 sites); a module-wide `#![allow(needless_return)]`
(`split` tracks lint attributes per leaf, so every child `mod` declaration
would then need it too; a per-fn attribute moves with its fn).

## Verification

### What `tools/split` checks today

`apply` + `verify --manifest`: whole-item moves; per-leaf token hash
(comments separate, std-macro trailing commas normalised), effective
cfg/attributes, visibility against the manifest, name/path/trait/macro
resolution in the new module, include paths, `line!`/`Location` positions,
log targets (descendant rule); `--tests` maps test lists. `--delegate`:
a trait body → `Self::<prefix>…(self, params)` with the helper equal but
for name and visibility. **Not covered:** a body moving out of a match arm
into a new fn, and fn-local items becoming module items. `scope::free_names`
works on a body's tokens against module scope; it does not model `let`
bindings, closure/pattern params or block-local items and imports, so it
cannot by itself say what a name inside an arm was bound to.

### Proposed: `split extract` + `verify --extract`

Manifest section (per handler):

```toml
[extract]
fn = "handle_xi2_request"
scrutinee = "minor"
locals = { byte_order = "ClientByteOrder", minor = "u8" }
prelude = "buf"               # the outer `let` that moves into users
tail = "xi_write_reply"       # new fn = the old post-match statements
hoist = []                    # RANDR prep: the fn-local items
keep = ["41", "43", "49", "_"]
[extract.arms]
"1" = "handle_xi1_get_extension_version"
# … one line per extracted arm
```

`verify --extract` checks, per arm and for the dispatcher:
1. **Dispatcher:** old fn = new fn after removing the prelude and post-match
   statements, making the match the tail, and replacing each listed arm body
   by its call; kept arms equal up to a leading `return` / trailing `;` (and
   the RANDR tail on `other`).
2. **Call shape:** same pattern and guard tokens; `callee(a1, …)` with plain
   ident args equal to the callee's param names in order; callee is new
   (absent from `<rev>^`, as `prior_names` does for `--delegate`), in the
   same module, same cfg context, private, return type = dispatcher's.
3. **Binding equivalence (bounded lexical resolver).** For every
   identifier occurrence in the arm (and in the tail), resolve its binding
   twice: in the original fn and in the extracted fn. Each pair must name the
   same binding: an arm-internal binding ↔ the same arm-internal binding; an
   outer param/`let` ↔ the callee param of that name (outer type tokens, or
   the `locals` type); a fn-local item/import ↔ the hoisted module item with
   identical tokens; anything else ↔ the same module-scope resolution
   (existing `Names` model). Params must equal the outer names so resolved,
   from arm **and** tail, minus prelude bindings. Token equality plus a
   successful build does **not** prove this: an outer local the generator
   forgot can compile against a same-named module item or glob import.
   The resolver is **bounded**: it models only the binding forms that occur
   in these two fns, each with a fixture:
   - `let` (plain and with patterns); `let … else` (initializer and else
     block are outside the binding's scope);
   - `if let` / `while let` and `&&`-chained `let`s (a binding scopes over
     the later conditions and the then-body, not the else);
   - `for` patterns; `match` arm patterns and guards, including inside
     `matches!` (`matches!(body.get(5), Some(&p) if p != 0)`, minor 31);
   - closure params (arms 19, 27, 31);
   - block-local items and `use`s (arm consts, the `use` in 19, `fn part`,
     `struct MonitorRow`), and nested-fn params (RANDR's fn-local helpers);
   - implicit format-string captures (`"{dev}"`) in `format!`-family and
     log macros, resolved like a plain identifier at the macro site.
   Any other binding form (or a macro whose arguments it cannot parse as
   expressions) makes verify **refuse the arm**: it is extracted by hand and
   reviewed, and listed in the manifest as such.
4. **Body:** one of the three forms above (`<arm block>`; `{ <arm block> }
   <tail>`; `<prelude> { <arm block> } <tail>`), token-compared as today;
   the prelude is present iff the arm uses its binding; the tail is
   `<tail>(state, client_id, buf | Vec::new())` or the old post-match
   statement; the only extra attribute allowed is
   `#[allow(clippy::needless_return)]`, and only on the first form.
5. **Tail fn:** body = the old post-match statements verbatim.
6. **Comments:** the arm's lead-in + inner comments = the callee's.
7. **Hoist (prep commits):** each listed fn-local item reappears as a
   module item with identical tokens, and every other leaf's resolution is
   unchanged (the existing check), which catches a hoisted name shadowing a
   glob import.

**Fixtures (red→green):** one per supported binding form above; a
tail-only param (XIGetClientPointer, 45); closure-param shadowing
(`|state: &ServerState, ..|`, arm 19; `|buf, ..|`, arm 31, which must
generate **no** prelude and the empty-write tail); an unsupported binding
form (must refuse); an arm-local `use`/const (arms 19, 2); a fn-local import/helper
used by an arm (RANDR); a hoisted name shadowing a glob import (must fail);
a forgotten param that compiles against a same-named module item (must
fail); a tail inside the arm block or a missing inner block (must fail).

`split extract <manifest>` generates the code by source-text slicing (keeps
comments and layout) and runs the used-names pass to write the params. It is
not trusted; `verify --extract` is.

**Tool work:** verify ~400 lines, bounded resolver ~350 (the forms above,
explicit refusal for the rest; module scope stays the existing model),
generator ~200, fixtures ~500 (≈15 cases). About 1.45k lines, 3–4× the
delegate mode.

### Gate

- **Splitter** (standalone workspace, not covered by the repo gate), on the
  tool commit and whenever it changes: `cargo test --manifest-path
  tools/split/Cargo.toml --locked`, `cargo clippy --manifest-path
  tools/split/Cargo.toml --all-targets -- -D warnings`, `cargo +nightly fmt
  --manifest-path tools/split/Cargo.toml -- --check`.
- **Static:** `verify --extract` on each prep/extract commit; `verify
  --manifest` + `--tests` on each move commit; `cargo +nightly fmt`; `cargo
  clippy --all-targets -- -D warnings` ×3 feature configs.
- **Unit:** workspace tests ×3 configs, lavapipe (rule 5). Direct coverage:
  `process_request/tests/` `xi1` 36, `xi2` 60, `xi2_allow` 28, `xi2_grabs`
  23, `xi_config` 45, `xi_hotplug` 3 (195 XI tests); `randr_crtc` 34,
  `randr_output` 46 (80 RANDR tests). They call `handle_xi2_request` /
  `handle_randr_request` and none names an arm, so they run unchanged
  against the new dispatcher. Per-arm coverage: tables above.
- **xts:** `just xts-yserver XI` and `XIproto` (vng), run on the branch
  base (`4fdef034`) and again after the XI move, same harness and binary
  build mode; compared per (case, test purpose) with
  `tools/xts-vs-baseline.py diff <base-journal> <after-journal>`, not by
  totals. Both runs must be complete and have identical (case, purpose) key
  sets; a missing purpose makes the run inconclusive (rerun), never a pass.
  Every verdict difference, in either direction, is investigated (rerun on
  both sides first; runs are nondeterministic).
  **Script change needed** (in tool commit 0b below): today `cmd_diff`
  (`tools/xts-vs-baseline.py:86–122`) only prints `only in baseline/
  candidate` counts and exits 1 solely on PASS→non-PASS. Add a strict mode
  that exits non-zero when the key sets differ, when either journal is
  incomplete (measured record codes in a real XI journal: each TC start
  `10` has a TC end `80`, each TP start `200` a result `220`, one end
  record `900`), or on any verdict mismatch. These are the
  only tests that enter the 15 untested XI1 arms.
- **HW on eiger** (plan): XI2 grabs via GTK popups/menus (51–55), scrolling
  and touchpad (46), `xinput list` / `list-props` / `set-prop` (36–39,
  56–59, 2, 48), `xinput test-xi2`, `tools/barrier-smoke.c` (61); RANDR:
  `xrandr --query`, mode/rotate/scale, `--setmonitor`, `--gamma`.

## Final trees (after `cargo +nightly fmt`)

**XI**: under `xi/dispatch/`, so every log target stays a descendant of
`process_request::xi::dispatch` (zero `[log_targets]` entries).

```
xi/mod.rs              575  unchanged (XI1 helpers, handle_xi_set_focus,
                            handle_xi_change_hierarchy, tail checks)
xi/dispatch.rs         216  //! module map, handle_xi2_request (length
                            gates + match), xi_write_reply, child mod decls
                            + globs, the hoisted ClientByteOrder import
xi/dispatch/
  devices.rs           649  1, 47 version; 2, 48 enumeration; 3, 4, 5
                            open/close/mode; 11, 12 change keyboard/pointer
                            device; 44, 45 client pointer
  control.rs          1307  10 motion history; 22, 23 feedback; 24–29
                            key/modifier/button maps; 30 device state;
                            32 bell; 33 valuators; 34, 35 device control
  events.rs            787  6, 7 XI1 selection; 8, 9 dont-propagate;
                            31 SendExtensionEvent; 46, 60 XI2 selection
  grabs.rs            1772  13–19 XI1 grabs + AllowDeviceEvents;
                            51–55 XI2 grabs + XIAllowEvents
  props.rs             649  36–39 XI1 properties; 56–59 XI2 properties
  focus.rs             421  20, 21, 50 focus; 40 query pointer;
                            42 change cursor; 61 barrier release
```

`control` (14 arms) and `xi/dispatch/` instead of `xi2/` as proposed:
the arms are XI1 and XI2, and the descendant rule. `grabs`/`props` repeat
the basenames of `process_request/{grabs,props}.rs`; no body uses a
`grabs::` or `props::` path, so resolution is unaffected.

**RANDR**: under `randr_ext/` (descendants of `process_request::randr_ext`).

```
randr_ext.rs     387  //! module map, types, shared helpers (active_monitors,
                      send_root_config_notify, unsupported warn pair,
                      hoisted request_xid / RANDR_MAJOR_OPCODE / x11randr),
                      handle_randr_request, the 16/45 and 46 stubs
randr_ext/
  screen.rs      611  0, 4, 5, 6, 7, 8, 25; set_screen_config + encoder (2),
                      randr_client_knows_rates
  output.rs      815  9, 30, 31; 10–15 output properties; output_exists,
                      synthetic_output_property
  crtc.rs        790  20, 21; 22–24 gamma; 26, 27 transform; 28, 29
                      panning; crtc-config start/complete/reply;
                      crtc_exists, crtc_is_leased
  provider.rs    384  32–35; 36–41 provider properties;
                      provider_relationship_protocol_error
  monitor.rs     191  42–44
  xinerama.rs    138  handle_xinerama_request: one [exceptions] entry
                      (fn-local require_len! moves module; its expansion
                      names resolve the same through `use super::*`)
```

Helpers used by one child moved with it; shared ones stay in the root
(children see ancestor privates). `send_root_config_notify` (only `monitor`
calls it) stays in the root as listed in the approved tree. Root globs:
`pub(crate) use crtc::*` keeps `complete_crtc_config` (run.rs) reachable as
`process_request::complete_crtc_config`; `pub(super) use xinerama::*` keeps
`handle_xinerama_request` reachable from the core dispatch (its visibility
goes from `pub(super)` in `randr_ext` to `pub(in
crate::core_loop::process_request)` in `xinerama`: same reach). Visibility
delta, all `pub(super)` (= today's reach) except that one: 58 XI callees;
33 RANDR callees + `set_screen_config`, its encoder and
`start_crtc_config` (called across groups) + `handle_xinerama_request` = 37.

## Results

Commits on `refactor/phase2c-dispatch` (FF, distinct): `a6351fda`
decisions 10–11, `0a571f85` splitter, `def9bef6` xts strict diff,
`e4552e71` XI prep, `a0cc43da` XI extract, `ea82d009` XI move, `cd5d7798`
RANDR prep, `9dfcc2e6` RANDR extract, `e35594e1` RANDR move, `88c1ff67`
root docs, `c492f39c` blame-ignore for the six prep/extract/move commits. Manifests:
`tools/split/manifests/{xi,randr}_2c_{prep,extract,move}.toml` (+ `.paths`).

- **Splitter:** `bind.rs` 781 lines, `extract.rs` 1,833, fixtures
  `tests_extract.rs` 448 (26 tests: one per modelled binding form, the
  refusals, and the end-to-end cases listed above); 115 splitter tests,
  clippy and nightly fmt clean. About 2× the 1.45k estimate, mostly
  rustfmt's vertical layout of the checks.
- **Prep (`verify --extract`, hoist):** XI 1 item, RANDR 8 items (9 names)
  → OK. Two refinements were needed for RANDR, both narrowing a false
  positive of the existing per-leaf check, not the check itself: a leaf
  that binds a hoisted name itself (`set_screen_config`'s and
  `handle_xinerama_request`'s own fn-local imports) is not compared on that
  name (`extract::binds_locally`: an item at the top of the fn body, the
  name absent from the signature); and the hoisted
  `ClientByteOrder`/`x11randr` imports added to every leaf's traits in scope
  are accepted only when proven non-traits by walking `yserver_protocol`'s
  module tree to an enum / a module (`extract::non_trait_hoists`).
- **Extract (`verify --extract`):** XI 58 arms → 13 returning, 12 void +
  tail, 33 prelude + tail (exactly the inventory's 13/12/33), 4 kept,
  `xi_write_reply`; 1,644 outer → param, 38 prelude, 1,289 module-scope
  name occurrences checked. RANDR 35 arms → 34 returning, 1 void + tail
  (SelectInput), 4 kept; 688 outer → param, 842 module scope. **No arm was
  refused**: none extracted by hand (`by_hand` empty).
- **Move (`verify --manifest --tests`):** XI 60/60 leaves identical, 58
  visibility changes, 42 leaves with log calls moved to descendants; RANDR
  57/57, 37 visibility changes, 1 audited exception, 7 log-target
  descendants. Tests mapped 4013 / 4018 / 4126 (default / tcp-transport /
  xdmcp), unchanged.
- **Gate per commit:** nightly fmt, clippy `--workspace --all-targets -D
  warnings` ×3 configs, workspace tests ×3 configs (core lib 1683 / 1683 /
  1699, yserver lib 1619 / 1624 / 1626 + 116 ignored, protocol 391 / 391 /
  481) green on every code commit; logs `target/gate-2c-<step>.log`.
- **Pending (jos):** xts `XI` and `XIproto` on `4fdef034` and on this branch,
  compared with `tools/xts-vs-baseline.py diff --strict`; eiger HW smoke.

## Commit sequence

0. `feat(tools/split): extract arms into per-minor fns, verify --extract`
   (+ bounded resolver, fixtures); splitter test/clippy/fmt gate.
0b. `feat(tools): xts-vs-baseline strict diff (key sets, complete journals,
   any mismatch fails)`.
1. `refactor(core): hoist handle_xi2_request's fn-local import (prep)`.
2. `refactor(core): one fn per XI minor (extract)`: `split extract` + fmt,
   `verify --extract`, gate. `dispatch.rs` is ~5.8k lines until 3
   (the existing exception).
3. `refactor(core): split XI dispatch into devices/control/events/grabs/
   props/focus (move)`: `split apply` + fmt, `verify --manifest --tests`,
   gate, xts XI/XIproto.
4. `refactor(core): hoist handle_randr_request's fn-local items (prep)`.
5. `refactor(core): one fn per RANDR minor (extract)`.
6. `refactor(core): split randr_ext into screen/output/crtc/provider/
   monitor/xinerama (move)`.
7. `docs(core): XI dispatch and randr_ext root docs: ownership line + module
   map`.
8. `chore: blame-ignore` for 1–6 (the extract commits re-indent every arm
   line, so they matter most).
9. This doc and the plan updated to the final trees; the `xi/dispatch.rs`
   exception leaves the size check.

HW smoke on eiger after 6 (or after 3 and 6). FF merge (distinct commits,
bisectable). Optional 10, own commit: drop the tail `return`s with `clippy
--fix` and remove the allows.

## Risks

| risk | mitigation |
|---|---|
| a body changes meaning when it leaves the arm | binding-equivalence check (3), body forms (4); same module in the extract commit; compiler for tails and types |
| the splitter itself is wrong | its own test/clippy/fmt gate; red→green fixtures for every check |
| 16 XI arms (1,190 lines) have no unit coverage | mechanical check + xts XI/XIproto + eiger smoke |
| in-flight XI branches (`fix/xts-xi-winnable`, `codex/freeze-state-unification`) edit grab arms 13–19 / 51–55 | not waited for (decision 5); re-applied by hand afterwards (plan decision 7) |
| ~48 `needless_return` allows are noise | optional commit 10 |
| void XI1 arms still write an empty buffer | kept on purpose (2c changes no behaviour; decision 11) |
| line estimates off | measured after fmt in the results section, as in earlier phases |

## Decisions (jos via coordinator, 2026-10-12, after codex review)

1. Six XI groups under `xi/dispatch/` (`devices, control, events, grabs,
   props, focus`) and six RANDR files under `randr_ext/`, as above.
2. `grabs`/`props` repeating `process_request/` basenames is fine.
3. The already delegated handlers (`handle_xi_set_focus`,
   `handle_xi_change_hierarchy` in `xi/mod.rs`, `handle_xi_warp_pointer` in
   `process_request/focus_pointer.rs`) stay where they are this phase.
4. Per-fn `#[allow(clippy::needless_return)]` with identical bodies;
   optional cleanup commit 10 later.
5. Do not wait for `fix/xts-xi-winnable` or
   `codex/freeze-state-unification`.
6. Body shape `<prelude> { <arm> } <tail>` (codex): the arm's locals drop
   before the tail, as today.
7. Params include the tail's dependencies; binding equivalence is checked by
   a bounded lexical resolver (listed forms, refusal otherwise), not by token
   equality plus compilation.
8. The splitter's own test/clippy/fmt runs are part of the gate; xts
   XI/XIproto run before and after, compared per test purpose.
9. Merge: fast-forward (distinct, bisectable steps, as every earlier phase-2
   branch); codex suggested squash, the standing rule wins.
10. Lead-in `//` comments stay `//` on the new fns in this branch (bodies
    and comments stay token-identical); a `///` conversion, if wanted, is a
    later separate commit.
11. Void XI1 requests keep the empty reply write (`xi_write_reply(state,
    client_id, Vec::new())`): no behaviour change in a refactor.

## Open questions

**For codex (next round):**
1. Grouping: does any arm sit in the wrong file by responsibility?
