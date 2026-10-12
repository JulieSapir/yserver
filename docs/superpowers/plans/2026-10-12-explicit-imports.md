# Proposal: explicit imports, narrower visibility, three placement fixes

Follow-up to `2026-10-08-source-layout-cleanup.md` phase 2. Branch
`refactor/explicit-imports` (from master `97aeeb24`). Proposal only.
Verification is deliberately light (maintainer, 2026-10-12): rustc in every
config, the normal test gate, a FreeBSD check by jos before merge, and one
cheap diff script. There is no new `split verify` mode.

## How it was measured

On a throwaway copy of `97aeeb24` (scratch, not committed):
1. A throwaway `split globs` subcommand (about 300 lines on top of the
   splitter's `Names` model). For each `use super::*;` it lists the names the
   file uses (`free_names` of every leaf and impl header, plus macro
   invocations) that the glob supplies. It records where each name comes
   from: defined in an ancestor, imported by an ancestor, or re-exported by a
   sibling. It also adds every trait the glob supplied as `use T as _;`.
   Then it writes these imports in place of the glob, using crate-absolute
   paths. An import the ancestor made is copied with the ancestor's own path
   and alias.
2. `cargo check --workspace --lib` (normal and `--profile test`) in all three
   feature configs. Any generated import that rustc reports unused in all six
   units is dropped. An import that is unused in only some units is reported
   as cfg-dependent.
3. The result compiles in all six units. It needed one hand fix, described
   below. Then `rustfmt` (repo config, `imports_granularity = "Crate"`).

rustc decides what stays, so the counts below are exact for Linux. They are
not exact for FreeBSD. A trait counts as needed only if rustc used it for
method resolution, so it is "used" by definition. Macros need no special
handling: the only macros the globs supply are `log::{debug,trace}`, used
in 34 files. When a macro import is missing, compilation fails.

## 1. Inventory: `use super::*` in non-test code

121 files. That is 118 that start with the glob, plus 3 `#[cfg(test)]`
helper files (`backend/`, `engine/` and `scene/for_tests.rs`), which clippy
also flags. All of them come from the phase-1/2 splits; the earlier steps
2.1 and 2.3–2.5 were not executed. Test modules (`tests/*.rs`, about 100
files with `use super::*`) are out of scope.

| family (under `crates/`) | files | names | defined in ancestor | ancestor's import | sibling re-export | trait `as _` | named trait | log macro | names min/med/max | fmt lines (med/max) |
|---|---|---|---|---|---|---|---|---|---|---|
| core `process_request/*` | 25 | 460 | 160 | 274 | 0 | 0 | 0 | 26 | 11/18/44 | 454 (17/39) |
| core `process_request/randr_ext/*` | 6 | 105 | 43 | 61 | 0 | 0 | 0 | 1 | 10/18/23 | 92 (18/20) |
| core `process_request/xi/dispatch/*` | 6 | 161 | 91 | 64 | 0 | 0 | 0 | 6 | 19/29/35 | 134 (24/29) |
| core `process_request/xi/*` | 2 | 33 | 14 | 18 | 0 | 0 | 0 | 1 | 16/17/17 | 40 |
| core `resources/*` | 7 | 73 | 32 | 41 | 0 | 0 | 0 | 0 | 1/9/22 | 47 (6/17) |
| `kms/render/backend/kms/*` | 9 | 160 | 52 | 94 | 5 | 7 | 2 | 0 | 0/17/34 | 145 (13/34) |
| `kms/render/backend/portable/*` | 14 | 224 | 69 | 149 | 1 | 5 | 0 | 0 | 0/16/33 | 197 (15/25) |
| `kms/render/backend/trait_impl.rs` | 1 | 40 | 1 | 37 | 2 | 0 | 0 | 0 | 40 | 18 |
| `kms/render/engine/*` | 15 | 258 | 143 | 115 | 0 | 0 | 0 | 0 | 2/17/30 | 214 (16/23) |
| `kms/render/platform/*` | 11 | 188 | 81 | 99 | 0 | 7 | 1 | 0 | 3/14/38 | 164 (15/30) |
| `kms/render/scene/*` | 12 | 277 | 168 | 106 | 0 | 0 | 3 | 0 | 8/22/63 | 219 (18/47) |
| `kms/vk/scanout/*` | 10 | 227 | 154 | 65 | 0 | 6 | 2 | 0 | 5/20/61 | 230 (18/51) |
| **total** (118; `for_tests.rs` ×3 not measured) | 118 | **2,206** | 1,008 | 1,123 | 8 | 25 | 8 | 34 | median 18, p90 33 | **1,954** |

- **Size.** After formatting, 118 glob lines become 1,954 lines of `use`
  blocks (median 16 per file). The largest are `xi/dispatch.rs`, `scene/tick.rs`
  (47), `scanout/copied.rs` (51) and `process_request/windows.rs` (39).
  Example result (`portable/draw.rs`, 33 names):
  `use crate::kms::{cpu_types::{…}, render::{backend::{…, portable::{clip::{…},
  text::text_advance}}, engine::…, submit_trace::{Flags as SubmitFlags, …},
  target::{…}}};`.
- **Half the names come from the ancestor's own imports** (1,123: `std`,
  `ash`, `yserver_protocol`, `crate::server`, …). Of the rest, 1,008 names
  are items defined in an ancestor.
- **Traits that only take part in method resolution:** 25 `as _` imports:
  `yserver_core::backend::Backend` ×10 (child impl blocks call trait methods
  on `KmsBackend`), `std::os::fd::{AsFd ×7, AsRawFd ×3, FromRawFd ×2,
  IntoRawFd}`, `drm::Device`, `drm::control::Device`. There are 8 more
  in-tree traits that are also named in the file.
- **Pass-through `mod.rs` files.** `backend/portable/mod.rs` and
  `backend/kms/mod.rs` need no imports: their glob only fed their children.
- **Root import diet (second-order effect).** Once the children import
  explicitly, the roots' imports that existed only to feed children become
  unused. That is 163 names unused in all units: `process_request.rs` 39,
  `resources/mod.rs` 24, `backend.rs` 57, `engine.rs` 6, `platform.rs` 11,
  `scene.rs` 14, `scanout.rs` 12. Another 59 names are still used, but only by
  the `tests/` modules through their `use super::*`. They move into the test
  modules that use them (decision 2); the test files' `use super::*` globs
  stay.
  The private downward globs in the kms roots (for example `scanout.rs`
  `use bo::*` ×7) served only sibling access, so they become unused too. This
  diet must be in the same commit as the children, or `-D warnings` fails.
- **Test modules below a rewritten file:** one break. `scanout/copied/two_gpu_tests.rs`
  names the trait `DisposableProbeAttempt`, which now reaches `copied.rs`
  only as `as _`. It needs one added `use` (a loud E0405).
- **cfg-dependent imports:** one. `platform/fence.rs` `std::rc::Weak` is used
  only by its `#[cfg(test)]` items, so its `use` gets `#[cfg(test)]`.

## 2. Visibility

**Method.** A Python scan over the `yserver`, `yserver-core` and
`yserver-protocol` sources looked at every non-test declaration with
`pub(crate)`, `pub(super)` or `pub(in …)`: 2,851 of them. Struct fields are
excluded. The users of a declaration are the files where its identifier
occurs; the needed reach is the lowest common ancestor of its module and
those users. Name collisions add false users, so the counts err toward
"keep". This is an estimate: rustc settles each change (E0603, E0624).

| tree | narrowable | → private | → `pub(super)` | → `pub(in …)` | assoc fn named like a trait method (exclude) |
|---|---|---|---|---|---|
| `kms/render/backend` | 140 | 44 | 79 | 17 | 0 |
| `kms/render/engine` | 97 | 10 | 18 | 69 | 5 |
| `kms/render/platform` | 135 | 20 | 23 | 92 | 3 |
| `kms/render/scene` | 40 | 4 | 5 | 31 | 0 |
| `kms/vk/scanout` | 54 | 11 | 2 | 41 | 2 |
| core `process_request` | 49 | 8 | 5 | 36 | 0 |
| core `resources` | 2 | 0 | 2 | 0 | 0 |
| **split trees** | **517** | 97 | 134 | 286 | 10 |
| rest of `yserver` / core | 923 | 224 | 556 | 143 | 6 |

Most of this predates the splits: `pub(crate)` items that were always
wider than needed. In the split trees, the `pub(crate) → pub(in …)` cases are
mostly engine, platform and scanout methods that scene or backend call.

**Proposed rule.**
- In split trees only, narrow to **private** (every user is in the item's
  module subtree; children see their ancestors' private items) or to
  **`pub(super)`**. That is 231 items.
- Do not add new `pub(in path)` (286). Phase 3 moves `kms/render` and `kms/vk`
  into `yserver-render`: every `pub(in crate::kms::render…)` path would have
  to be rewritten, and many of these items become `pub` across the crate
  edge anyway.
- Leave the rest of the crate (923) for later, if wanted.

**Do not touch:**
- `pub` items: cross-crate API from core and protocol, and the `yserver` lib
  used by `render_acceptance`.
- Re-exports that keep old paths working: `pub(crate) use crtc::*`,
  `pub(super) use xinerama::*`, `pub use wire::*`, the `mod.rs` re-export
  globs.
- `*_for_tests` items and `for_tests.rs`.
- Struct fields.
- Any inherent method that shares a name with a trait method (16). rustc's
  method probe skips an inaccessible inherent candidate and can pick a
  same-named trait method without an error. `KmsBackend`/`Backend` is exactly
  this shape; the `--delegate` naming rule exists because of it.

**Order: imports before visibility.** While a glob is still in place, an
item that becomes invisible simply drops out of the glob, and the name can
then resolve to another glob or to the prelude without an error. With
explicit imports, the same change fails loudly at the `use`.

## 3. Mechanics and verification

**Generation.** Commit the throwaway generator as
`split globs <root> <module> [--write]`, about 300 lines. Its output is not
trusted; rustc is the check. Workflow per subtree:
1. Run `split globs --write`.
2. `cargo check` the six units (lib and `--profile test` × three configs).
3. Run `tools/split/prune-unused-imports.py` (about 40 lines). It drops
   imports that rustc reports unused in all six units, and lists the ones
   unused in only some units.
4. Hand-fix the listed cases with `#[cfg]` or by moving the import into the
   test module that uses it.
5. Run the root import diet the same way.
6. `cargo +nightly fmt`.

Doing 2,206 names by hand from rustc's "consider importing" hints is
error-prone (the hints are often ambiguous), so the generator is worth its
size.

**Token identity: `tools/use-only-diff.py`, ~25 lines.** For every `.rs` file
the commit changes, it deletes **module-level** `use` items (column 0, with
their attributes) and `pub(...)` keywords from the before and after text,
collapses whitespace, and requires the two to be equal. Function-local and
block-local `use` items are indented, so they are not stripped and must
compare equal (codex). A `use` inside an inline indented `mod {}` is also
compared, which is stricter than needed; none exists in the 121 files.
**Regression fixture:** a function-local `use std::cmp::min as pick;`
changed to `use std::cmp::max as pick;` must FAIL. The prototype stripped
every `use`; it passed on 117 files of the scratch rewrite and failed as
soon as one token in a body changed. The column-0 version goes into the
tool commit together with the fixture. It covers the import commits and the
visibility commits.

**The silent case rustc does not catch.** A name that the glob supplied, and
that is missing after the edit, can resolve to something else without an
error. In this tree:
- **`drm`:** the module `crate::drm` against the `drm` crate. Five files use
  `drm::…` paths that mean `crate::drm` through the glob:
  `backend/kms/session.rs` and `platform/{connectors, init, power, qualify}.rs`.
  If `use crate::drm;` is dropped, the paths fall through to the extern crate.
  Both have a `Device`, for example. **Guard (one grep):** every file with an
  unqualified `drm::` path also has `use crate::drm` (or `crate::drm::…`).
- **Prelude names** (`Result`, `Error`, `Box`, …) supplied through a glob:
  measured **0**.
- **Traits:** dropping a used trait import is silent only if another trait in
  scope has an applicable method with the same name. Removing only what
  rustc reports unused rules this out, because an unused import took no part
  in resolution. **Hand edits must keep every generated trait import unless
  rustc reports it unused** (codex).

With the generator, a missing name cannot happen: every name the glob
supplied and the file mentions gets an explicit import first. The risk is in
hand edits.

**FreeBSD.** The kms code compiles on FreeBSD
(`any(target_os = "linux", "freebsd")`), so an import used only under
`cfg(target_os = "linux")` is an unused-import error there. **Static guard:**
when writing imports, any name whose uses in the file are all inside
`cfg(target_os…)`-gated items gets the same cfg on its `use` (or the import
moves into the gated item).

Measured on the rewrite: 6 files have target_os gates (13 sites):
`process_request/misc.rs`, `backend/kms/{session, randr}.rs`,
`trait_impl.rs`, `platform/{init, flip_events}.rs`.

| family | files with target_os gates | names used only under a gate |
|---|---|---|
| core `process_request` | 1 | 0 |
| `backend/kms` | 2 | 0 |
| `backend/trait_impl.rs` | 1 | 0 |
| `platform` | 2 | 0 |
| all other families | 0 | 0 |

The gated bodies use full paths or their own fn-local `use`, so no name is
gated-only. Names can be checked this way, but `as _` trait imports cannot.
Those exist in 4 of the 6 files (`Backend` in `session.rs` and `randr.rs`;
`AsFd`/`AsRawFd` in `init.rs` and `flip_events.rs`): review them by eye. The
script repeats this table on every commit, and its `as _` list comes along.

**Splitter bug found (fix first).** `scope::free_names` reads a name followed
by `!=` as a macro invocation, so it skips the name (the path check does the
same). In this tree: `options & !PRESENT_ALL_OPTIONS != 0` (`present_ext.rs`)
and `action[1] & XKB_SA_LOCK_NO_UNLOCK != 0` (`portable/devices.rs`).
Consequence: earlier `split verify` runs never resolution-checked names in
that position. rustc compiled those commits and the moves kept module depth,
so the risk is low, but this is a soundness gap in the existing check. The
fix is two lines plus a fixture: `!` followed by `=` is not a macro bang.

**Gate per commit** (all cargo via `env -u RUSTC_WRAPPER`; full output kept
via `tee target/gate-imports-<step>.log`):
- `cargo +nightly fmt -- --check`
- the AGENTS.md pre-commit command exactly: `cargo clippy --all-targets --
  -D warnings`
- `F ∈ {"", --features tcp-transport, --features xdmcp}`:
  `cargo clippy --workspace --all-targets $F --locked -- -D warnings` and
  `cargo test --workspace --all-targets $F --locked`
- lavapipe, as CI runs it plus the ICD pin:
  `VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json
  YSERVER_ALLOW_SOFTWARE_VULKAN=1 cargo test -p yserver --locked --features
  xdmcp --no-fail-fast -- --ignored`
- `tools/use-only-diff.py <base>`, the `drm` grep, and the target_os cfg
  table
- splitter, whenever it changes: `cargo test`, `cargo clippy --all-targets
  -- -D warnings` and `cargo +nightly fmt -- --check`, each with
  `--manifest-path tools/split/Cargo.toml`
- **jos, once per PR before merge:** `cargo check` and clippy on his FreeBSD
  install

**Tool work:** generator ~300 lines (port of the throwaway), prune script ~40,
diff check ~25 + its fixture, `!=` fix + fixture ~30. About half a day. No new verify mode.

## 4. Enforcement afterwards

Add `wildcard_imports = "warn"` to `[workspace.lints.clippy]` in `Cargo.toml`;
CI's `-D warnings` makes it an error. Measured on the rewritten tree with
clippy `--all-targets --features xdmcp`:
- **0 hits on test modules.** clippy exempts `use super::*` in test modules,
  including the `tests/<topic>.rs` files: nothing to annotate.
- **0 hits on `pub`/`pub(..)` re-export globs.**
- **27 remaining hits:** the 3 `for_tests.rs` files and 24 private downward
  globs (`process_request.rs` 14, `xi/dispatch.rs` 6, `randr_ext.rs` 4).
  These are the dispatcher roots pulling in their children's handlers. Make
  them explicit in their own commit (same generator idea, a list of child
  items, pruned by rustc), then enable the lint.

No `#[allow]` list is needed if those 27 are done. A CI grep is unnecessary
once the lint is on.

## 5. Placement fixes (checked against the code)

| claim | real? | evidence | fix |
|---|---|---|---|
| VT acquire / vblank arm code sits in `backend/trait_impl.rs` | **yes** | `on_vt_acquire` (trait_impl.rs:336, 53 lines: VT_ACKACQ, drmSetMaster retry, resume) and `arm_present_absolute_vblank` (:245, 48 lines: `queue_crtc_sequence`, EOPNOTSUPP latch) are full bodies. 172 of 229 trait fns delegate. Three more bodies over 20 lines also stayed: `promote_pixmap_exportable` (:659), `release_redirected_backing` (:834), `clear_area` (:1199) | delegate commit with the existing `verify --delegate backend_`: `kms/session.rs` `backend_session_on_vt_acquire`, `kms/present.rs` `backend_present_arm_present_absolute_vblank`; the other three into `portable/{redirect, draw}.rs` too (decision 6) |
| crtc_config bits live in `backend/kms/randr.rs` | **yes** | the crtc-config lifecycle `backend_randr_{begin, drain_ready, finish, cancel, apply}_crtc_config(s)` is randr.rs:2144–2809 (~665 of 2,873 lines). Its helpers (`quiesce_…`, `recover_failed_crtc_config`, token/epoch, probes) are in `kms/crtc_config.rs` (489) | move commit (`split apply` + `verify --manifest`) of those five into `crtc_config.rs` (→ ~1,150; randr.rs → ~2,200). Names unchanged (decision 5) |
| text code inside `backend/portable/draw.rs` | **yes** | `backend_draw_{poly_text8, poly_text16, image_text8, image_text16}` (draw.rs:3189–3339, ~150 lines) parse TEXTITEMs and call `portable/text.rs` (`render_text_chars`, `image_text_common`, `text_advance`), which already holds the font helpers | move into `text.rs` (→ ~1,300; draw.rs → ~3,190); names stay `backend_draw_*` (decision 5) |

Placement commits are pure moves or delegations: names are unchanged, as
`split verify --manifest` / `--delegate` check. A rename would be its own
reviewed commit, and this branch has none.

**Do the placement fixes before the import rewrite.** `split apply` adds no
imports to a target file and relies on the globs. After the rewrite, every
move would need hand-written imports in the destination file.

## 6. Commit sequence (FF merge, distinct commits)

1. `fix(tools/split): a name before != is a name, not a macro call` (+ fixture).
2. `refactor(kms): delegate on_vt_acquire / arm_present_absolute_vblank to
   kms/session and kms/present (delegate)`, then `refactor(kms): delegate
   promote_pixmap_exportable / release_redirected_backing / clear_area to
   portable/redirect and draw (delegate)`.
3. `refactor(kms): move crtc-config lifecycle from kms/randr into kms/crtc_config
   (move)`.
4. `refactor(kms): move core text helpers from portable/draw into portable/text
   (move)`.
5. `feat(tools/split): globs generator, prune script, use-only diff check`.
6. One commit per subtree. Each contains the children's explicit imports,
   the root import diet, the test-only names moved into the test modules that
   use them, and the now-dead private downward globs dropped:
   `refactor(core): explicit imports in process_request/*` (25 files +
   `xi/*`), `… randr_ext/* and xi/dispatch/*`, `… resources/*`, `refactor(kms):
   … backend/{portable,kms}/*, trait_impl, for_tests`, `… engine/*`,
   `… scene/*`, `… platform/*`, `… vk/scanout/*`. That is 8 commits of
   roughly 100–500 changed lines each.
7. `refactor(core): explicit child imports in the process_request, xi/dispatch,
   randr_ext dispatchers`.
8. `chore(lints): deny clippy::wildcard_imports`.
9. Visibility, one commit per split tree (`refactor(<tree>): narrow
   visibility to private / pub(super)`), 231 items.
10. `chore: blame-ignore` for 2–4, 6, 7 and 9.
11. Plan doc and `docs/status.md` updated.

Merge: fast-forward. This is jos's standing rule for multi-commit refactor
branches whose SHAs are in `.git-blame-ignore-revs`, and AGENTS.md now says
PRs whose commits serve distinct purposes are fast-forward merged.

HW smoke is not needed for 5–9 (use-only diff plus compile). For 2–4, use
the phase-2 rule: lavapipe, and a short VT-switch and RANDR smoke on bee
after 2–3.

## Risks

| risk | mitigation |
|---|---|
| a name silently resolves elsewhere after the rewrite | generator imports every name it finds; prune only what rustc reports unused; `drm` grep; prelude case measured 0 |
| FreeBSD-only unused or missing import | target_os cfg guard (0 names today; 4 files with `as _` traits to review by eye); jos's FreeBSD check per PR |
| visibility narrowing switches method dispatch | exclude assoc fns named like a trait method (16) and fields; imports first |
| churn against in-flight branches (`fix/xts-xi-winnable`, `codex/freeze-state-unification` edit XI and grab files) | the use-only diff means conflicts happen only in `use` blocks; re-apply by hand (plan decision 7) |
| 1,954 import lines read as noise | crate-granularity fmt already groups them; style choice below |

## Decisions (jos via coordinator, 2026-10-12, after codex review)

1. New imports use crate-absolute `crate::…` paths (the generator's output).
   Aliases the parent defined (`Flags as SubmitFlags`) are kept, so bodies
   stay token-identical.
2. The 59 test-only root imports move into the test modules that use them.
3. Re-export globs (`mod.rs` `pub(super) use child::*`, the roots'
   `pub(crate) use child::*`; the lint does not flag them) are not converted
   into explicit lists. Ones that become unused after the explicit-import
   rewrite are removed (clippy requires it).
4. Visibility narrowing is limited to the proposed 231 items (private /
   `pub(super)`) in the split trees. No new `pub(in …)`; the rest of the
   crate is untouched.
5. No helper renames on move. Placement commits keep names
   (`split verify --manifest` / `--delegate`); any rename would be a separate
   reviewed commit, and this branch has none.
6. The three shorter bodies left in `trait_impl.rs`
   (`promote_pixmap_exportable`, `release_redirected_backing`,
   `clear_area`) are delegated too.
7. Codex notes adopted: the diff guard ignores only module-level `use` items
   (fixture above); hand edits keep generated trait imports unless rustc
   reports them unused; the exact AGENTS.md clippy command is in the gate.
   Merge is FF (see the commit sequence).
8. (2026-10-12, during execution) Decision 6 is dropped:
   `promote_pixmap_exportable`, `release_redirected_backing` and
   `clear_area` are already one-call forwards (the "over 20 lines" counted
   signatures and docs), so a delegate would only add a layer.
9. (2026-10-12) A placement commit that empties a re-export glob gets a prep
   commit with a temporary `#[allow(unused_imports, reason = …)]` on it, so
   `split verify` sees unchanged imports; the backend import commit removes
   the annotations. Moves between two existing sibling files are verified
   with an audited `[[sibling_moves]]` manifest list (item, exact old and new
   file); unlisted, misdirected and stale entries fail.

## Open questions (for codex, next round)

1. Is "prune only what rustc reports unused in all six units" a sound
   argument that method resolution is unchanged, given glob-imported traits
   are replaced by explicit `as _` imports?
2. Are there silent-shadowing cases besides `drm` and prelude names, for
   example macro textual scope against path-imported macros?
3. Should the inaccessible-inherent-method fallback be pinned by a scratch
   test before the visibility commits?
