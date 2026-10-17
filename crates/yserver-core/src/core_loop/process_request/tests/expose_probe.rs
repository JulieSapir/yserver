//! Request-level replays of `tools/expose-order-probe.c`: one client drives
//! the requests and watches the events, printed the probe's way so each
//! expectation is a line of the Xorg log.

use super::*;

const EXPOSURE_AND_SUBSTRUCTURE: u32 = 0x0000_8000 | 0x0008_0000;
const DAMAGE_MAJOR: u8 = 143;
const COMPOSITE_MAJOR: u8 = 144;
const DAMAGE_RAW: u8 = 0;
const DAMAGE_NON_EMPTY: u8 = 3;

/// A recorded fill: x, y, width, height.
pub(super) type Fill = (i16, i16, u16, u16);

/// Background of a probe window.
#[derive(Clone, Copy)]
pub(super) enum Bg {
    None,
    ParentRelative,
    Pixel(u32),
}

pub(super) struct Probe {
    pub(super) state: ServerState,
    pub(super) backend: RecordingBackend,
    peer: UnixStream,
    names: Vec<(u32, &'static str)>,
    damages: Vec<(u32, bool)>,
    next_xid: u32,
    sequence: u16,
}

impl Probe {
    /// The probe's observer: SubstructureNotify on root.
    pub(super) fn new() -> Self {
        let mut state = ServerState::new();
        let peer = install_client(&mut state, 1);
        let mut probe = Self {
            state,
            backend: RecordingBackend::new(),
            peer,
            names: vec![(ROOT_WINDOW.0, "root")],
            damages: Vec::new(),
            next_xid: 0x0020_0001,
            sequence: 0,
        };
        probe.backend.redirect_activation_supported = true;
        probe.select(ROOT_WINDOW.0, 0x0008_0000);
        probe
    }

    /// ChangeWindowAttributes(event-mask).
    pub(super) fn select(&mut self, window: u32, mask: u32) {
        let mut body = window.to_le_bytes().to_vec();
        body.extend_from_slice(&0x0800u32.to_le_bytes());
        body.extend_from_slice(&mask.to_le_bytes());
        self.request(2, 0, &body);
    }

    pub(super) fn request(&mut self, opcode: u8, data: u8, body: &[u8]) {
        self.try_request(opcode, data, body)
            .expect("request through core dispatcher");
    }

    pub(super) fn try_request(
        &mut self,
        opcode: u8,
        data: u8,
        body: &[u8],
    ) -> io::Result<RequestOutcome> {
        self.sequence = self.sequence.wrapping_add(1);
        process_request(
            &mut self.state,
            &mut self.backend,
            ClientId(1),
            SequenceNumber(self.sequence),
            RequestHeader {
                opcode,
                data,
                length_units: u32::try_from((4 + body.len()).div_ceil(4)).unwrap(),
            },
            body,
            None,
        )
    }

    fn xid(&mut self) -> u32 {
        let id = self.next_xid;
        self.next_xid += 1;
        id
    }

    /// The probe's `mk`: an override-redirect `InputOutput` child selecting
    /// Exposure and SubstructureNotify.
    pub(super) fn create(
        &mut self,
        name: &'static str,
        parent: u32,
        geometry: (i16, i16, u16, u16),
        bg: Bg,
    ) -> u32 {
        let id = self.xid();
        let (x, y, w, h) = geometry;
        let mut body = Vec::new();
        for word in [id, parent] {
            body.extend_from_slice(&word.to_le_bytes());
        }
        for half in [
            x.to_le_bytes(),
            y.to_le_bytes(),
            w.to_le_bytes(),
            h.to_le_bytes(),
        ] {
            body.extend_from_slice(&half);
        }
        body.extend_from_slice(&0u16.to_le_bytes()); // border
        body.extend_from_slice(&1u16.to_le_bytes()); // InputOutput
        body.extend_from_slice(&0u32.to_le_bytes()); // CopyFromParent visual
        let (mask, first) = match bg {
            Bg::None => (0x0001u32, 0u32),
            Bg::ParentRelative => (0x0001, 1),
            Bg::Pixel(p) => (0x0002, p),
        };
        body.extend_from_slice(&(mask | 0x0200 | 0x0800).to_le_bytes());
        for word in [first, 1, EXPOSURE_AND_SUBSTRUCTURE] {
            body.extend_from_slice(&word.to_le_bytes());
        }
        self.request(1, 0, &body);
        self.names.push((id, name));
        id
    }

    pub(super) fn map(&mut self, window: u32) {
        self.request(8, 0, &window.to_le_bytes());
    }

    pub(super) fn map_subwindows(&mut self, window: u32) {
        self.request(9, 0, &window.to_le_bytes());
    }

    /// COMPOSITE `RedirectSubwindows(window, Manual)`.
    pub(super) fn redirect_subwindows(&mut self, window: u32) {
        let mut body = window.to_le_bytes().to_vec();
        body.extend_from_slice(&[1, 0, 0, 0]);
        self.request(COMPOSITE_MAJOR, 2, &body);
    }

    /// COMPOSITE `UnredirectSubwindows(window, Manual)`.
    pub(super) fn unredirect_subwindows(&mut self, window: u32) {
        let mut body = window.to_le_bytes().to_vec();
        body.extend_from_slice(&[1, 0, 0, 0]);
        self.request(COMPOSITE_MAJOR, 4, &body);
    }

    /// COMPOSITE `GetOverlayWindow(root)` (minor 7) or `ReleaseOverlayWindow` (8).
    pub(super) fn overlay(&mut self, minor: u8) {
        self.request(COMPOSITE_MAJOR, minor, &ROOT_WINDOW.0.to_le_bytes());
    }

    /// How `window`'s background was painted on realize: `Some(true)` with
    /// its storage or, deferred, after a redirect backing was allocated;
    /// `Some(false)` never; `None` when not realized.
    pub(super) fn realized_painted(&self, window: u32) -> Option<bool> {
        let host = self
            .state
            .resources
            .window(ResourceId(window))?
            .host_xid?
            .as_raw();
        let calls = self.backend.calls.lock().unwrap();
        let (mut realized, mut deferred, mut backing) = (None, false, false);
        for call in calls.iter() {
            match call {
                RecordedCall::RealizeWindowStorage(x) if *x == host => realized = Some(true),
                RecordedCall::RealizeWindowStorageUnpainted(x) if *x == host => {
                    realized = Some(false);
                }
                RecordedCall::RealizeWindowStorageDeferred(x) if *x == host => {
                    (realized, deferred, backing) = (Some(false), true, false);
                }
                RecordedCall::AllocateRedirectedBacking { .. } => backing = true,
                RecordedCall::PaintWindowBackgroundRect { host_xid, .. }
                    if *host_xid == host && deferred && backing =>
                {
                    realized = Some(true);
                }
                _ => {}
            }
        }
        realized
    }

    pub(super) fn reparent(&mut self, window: u32, parent: u32, x: i16, y: i16) {
        let mut body = Vec::new();
        for word in [window, parent] {
            body.extend_from_slice(&word.to_le_bytes());
        }
        body.extend_from_slice(&x.to_le_bytes());
        body.extend_from_slice(&y.to_le_bytes());
        self.request(7, 0, &body);
    }

    pub(super) fn clear(&mut self, window: u32, rect: (i16, i16, u16, u16)) {
        self.try_clear(window, rect)
            .expect("request through core dispatcher");
    }

    pub(super) fn try_clear(
        &mut self,
        window: u32,
        rect: (i16, i16, u16, u16),
    ) -> io::Result<RequestOutcome> {
        let mut body = window.to_le_bytes().to_vec();
        for half in [
            rect.0.to_le_bytes(),
            rect.1.to_le_bytes(),
            rect.2.to_le_bytes(),
            rect.3.to_le_bytes(),
        ] {
            body.extend_from_slice(&half);
        }
        self.try_request(61, 1, &body)
    }

    /// The probe's `watch_damage` at one level: `raw` RawRectangles, else NonEmpty.
    pub(super) fn watch_damage(&mut self, drawable: u32, raw: bool) {
        let id = self.xid();
        let mut body = Vec::new();
        for word in [id, drawable] {
            body.extend_from_slice(&word.to_le_bytes());
        }
        body.push(if raw { DAMAGE_RAW } else { DAMAGE_NON_EMPTY });
        body.extend_from_slice(&[0, 0, 0]);
        self.request(DAMAGE_MAJOR, 1, &body);
        self.damages.push((id, raw));
    }

    /// The fills recorded on `window`'s storage since the last call.
    pub(super) fn take_fills(&mut self, window: u32) -> Vec<Fill> {
        let host = self
            .state
            .resources
            .window(ResourceId(window))
            .and_then(|w| w.host_xid)
            .map(|h| h.as_raw());
        let calls = std::mem::take(&mut *self.backend.calls.lock().unwrap());
        calls
            .iter()
            .filter_map(|call| match call {
                RecordedCall::FillRectangle {
                    host_xid,
                    x,
                    y,
                    width,
                    height,
                    ..
                } if Some(*host_xid) == host => Some((*x, *y, *width, *height)),
                _ => None,
            })
            .collect()
    }

    fn nm(&self, id: u32) -> &'static str {
        self.names
            .iter()
            .rev()
            .find(|(x, _)| *x == id)
            .map_or("?", |(_, n)| n)
    }

    /// The events since the last step, formatted like the probe's line;
    /// NonEmpty damage is then subtracted as the probe does.
    pub(super) fn step(&mut self) -> String {
        let bytes = read_all_available(&mut self.peer);
        let mut out = String::new();
        let u16_at = |e: &[u8], at: usize| u16::from_le_bytes([e[at], e[at + 1]]);
        for e in bytes.chunks_exact(32) {
            let line = match e[0] & 0x7f {
                12 => format!(
                    " Expose({} {},{} {}x{} c{})",
                    self.nm(le_u32(e, 4)),
                    u16_at(e, 8),
                    u16_at(e, 10),
                    u16_at(e, 12),
                    u16_at(e, 14),
                    u16_at(e, 16)
                ),
                19 => format!(" Map({}@{})", self.nm(le_u32(e, 8)), self.nm(le_u32(e, 4))),
                18 => format!(
                    " Unmap({}@{})",
                    self.nm(le_u32(e, 8)),
                    self.nm(le_u32(e, 4))
                ),
                21 => format!(
                    " Reparent({}@{} ->{} {},{})",
                    self.nm(le_u32(e, 8)),
                    self.nm(le_u32(e, 4)),
                    self.nm(le_u32(e, 12)),
                    u16_at(e, 16) as i16,
                    u16_at(e, 18) as i16
                ),
                c if c == crate::nested::DAMAGE_FIRST_EVENT => {
                    let damage = le_u32(e, 8);
                    let raw = self
                        .damages
                        .iter()
                        .find(|(d, _)| *d == damage)
                        .map(|(_, r)| *r);
                    format!(
                        " Dmg{}({} {},{} {}x{})",
                        match raw {
                            Some(true) => "R",
                            Some(false) => "N",
                            None => "?",
                        },
                        self.nm(le_u32(e, 4)),
                        u16_at(e, 16) as i16,
                        u16_at(e, 18) as i16,
                        u16_at(e, 20),
                        u16_at(e, 22)
                    )
                }
                0 => format!(" Error({})", e[1]),
                _ => continue,
            };
            out.push_str(&line);
        }
        let non_empty: Vec<u32> = self
            .damages
            .iter()
            .filter(|(_, raw)| !raw)
            .map(|(d, _)| *d)
            .collect();
        for d in non_empty {
            let mut body = d.to_le_bytes().to_vec();
            body.extend_from_slice(&[0; 8]);
            self.request(DAMAGE_MAJOR, 3, &body);
        }
        let _ = read_all_available(&mut self.peer);
        out.trim_start().to_string()
    }
}

/// Case 4 of the probe: ClearArea on `V`, whose bottom-right `S` covers;
/// each step's events and the fills on `V`.
fn clear_area_case(v_bg: Bg, raw: bool) -> Vec<(String, Vec<Fill>)> {
    let mut p = Probe::new();
    let t = p.create("T", ROOT_WINDOW.0, (50, 50, 400, 300), Bg::Pixel(0x40_4040));
    let v = p.create("V", t, (20, 20, 200, 150), v_bg);
    p.create("S", t, (120, 95, 150, 100), Bg::Pixel(0x00_ff00));
    p.map_subwindows(t);
    p.map(t);
    p.watch_damage(t, raw);
    let _ = p.step();
    let _ = p.take_fills(v);
    [
        (0, 0, 0, 0),
        (0, 0, 60, 50),
        (40, 30, 120, 80),
        (120, 90, 50, 40),
        (150, 0, 50, 150),
    ]
    .into_iter()
    .map(|r| {
        p.clear(v, r);
        (p.step(), p.take_fills(v))
    })
    .collect()
}

/// `miClearToBackground` clips the rect to the clip list, paints and
/// exposes per clip-list box (`mi/miwindow.c:84-112`, `mi/miexpose.c:365`).
fn events(case: Vec<(String, Vec<Fill>)>) -> Vec<String> {
    case.into_iter().map(|(e, _)| e).collect()
}

#[test]
fn clear_area_exposes_and_damages_only_the_clip_list() {
    assert_eq!(
        events(clear_area_case(Bg::Pixel(0x80_8080), true)),
        [
            "DmgR(T 20,20 200x75) DmgR(T 20,95 100x75) Expose(V 0,0 200x75 c1) Expose(V 0,75 100x75 c0)",
            "DmgR(T 20,20 60x50) Expose(V 0,0 60x50 c0)",
            "DmgR(T 60,50 120x45) DmgR(T 60,95 60x35) Expose(V 40,30 120x45 c1) Expose(V 40,75 60x35 c0)",
            "",
            "DmgR(T 170,20 50x75) Expose(V 150,0 50x75 c0)",
        ]
    );
    assert_eq!(
        events(clear_area_case(Bg::Pixel(0x80_8080), false)),
        [
            "DmgN(T 0,0 400x300) Expose(V 0,0 200x75 c1) Expose(V 0,75 100x75 c0)",
            "DmgN(T 0,0 400x300) Expose(V 0,0 60x50 c0)",
            "DmgN(T 0,0 400x300) Expose(V 40,30 120x45 c1) Expose(V 40,75 60x35 c0)",
            "",
            "DmgN(T 0,0 400x300) Expose(V 150,0 50x75 c0)",
        ]
    );
    assert_eq!(
        events(clear_area_case(Bg::None, true)),
        [
            "Expose(V 0,0 200x75 c1) Expose(V 0,75 100x75 c0)",
            "Expose(V 0,0 60x50 c0)",
            "Expose(V 40,30 120x45 c1) Expose(V 40,75 60x35 c0)",
            "",
            "Expose(V 150,0 50x75 c0)",
        ]
    );
}

/// A failed fill fails the ClearArea and damages nothing it did not paint.
#[test]
fn clear_area_fill_failure_propagates_without_damage() {
    let mut p = Probe::new();
    let t = p.create("T", ROOT_WINDOW.0, (50, 50, 400, 300), Bg::Pixel(0x40_4040));
    let v = p.create("V", t, (20, 20, 200, 150), Bg::Pixel(0x80_8080));
    p.map(v);
    p.map(t);
    p.watch_damage(t, true);
    let _ = p.step();
    p.backend.fail_fill_rectangle = true;
    assert!(p.try_clear(v, (0, 0, 0, 0)).is_err());
    assert_eq!(p.step(), "");
}

/// `miPaintWindow` fills only the clipped region (`mi/miwindow.c:110-116`).
#[test]
fn clear_area_paints_only_the_clip_list() {
    let fills: Vec<_> = clear_area_case(Bg::Pixel(0x80_8080), true)
        .into_iter()
        .map(|(_, f)| f)
        .collect();
    assert_eq!(
        fills,
        [
            vec![(0, 0, 200, 75), (0, 75, 100, 75)],
            vec![(0, 0, 60, 50)],
            vec![(40, 30, 120, 45), (40, 75, 60, 35)],
            vec![],
            vec![(150, 0, 50, 75)],
        ]
    );
    assert!(
        clear_area_case(Bg::None, true)
            .iter()
            .all(|(_, f)| f.is_empty())
    );
}

/// Case 2 of the probe in comp mode: ParentRelative or pixel `K` (with
/// ParentRelative `G`) under `P`, whose subwindows a Manual redirect holds;
/// root's are redirected too. The map steps' events, and whether P, K, G
/// were realized with their background paint.
fn manual_parent_case(
    k_bg: Bg,
    map_parent_last: bool,
    raw: bool,
) -> (Vec<String>, [Option<bool>; 3]) {
    let mut p = Probe::new();
    p.redirect_subwindows(ROOT_WINDOW.0);
    let pw = p.create("P", ROOT_WINDOW.0, (50, 50, 300, 200), Bg::Pixel(0x80_8080));
    let k = p.create("K", pw, (20, 20, 120, 100), k_bg);
    let g = p.create("G", k, (10, 10, 50, 40), Bg::ParentRelative);
    p.map(g);
    p.redirect_subwindows(pw);
    p.watch_damage(pw, raw);
    p.watch_damage(k, raw);
    let _ = p.step();
    let order = if map_parent_last { [k, pw] } else { [pw, k] };
    let lines = order
        .into_iter()
        .map(|w| {
            p.map(w);
            p.step()
        })
        .collect();
    (lines, [pw, k, g].map(|w| p.realized_painted(w)))
}

/// `compRedirectSubwindows` sets `inhibitBGPaint` on P (`composite/compalloc.c:404-411`)
/// and `miPaintWindow` follows K's ParentRelative chain to it (`mi/miexpose.c:425-436`):
/// neither is painted nor damaged. K draws into its own pixmap, so its paint
/// never damages P.
#[test]
fn manual_redirect_parent_inhibits_parent_relative_children() {
    const K_EXPOSES: &str = "Expose(K 0,0 120x10 c3) Expose(K 0,10 10x40 c2) \
                             Expose(K 60,10 60x40 c1) Expose(K 0,50 120x50 c0)";
    let unpainted = [Some(false), Some(false), Some(false)];
    assert_eq!(
        manual_parent_case(Bg::ParentRelative, false, true),
        (
            vec![
                "Map(P@root) Expose(P 0,0 300x200 c0)".to_string(),
                format!("Map(K@P) {K_EXPOSES} Expose(G 0,0 50x40 c0)"),
            ],
            unpainted
        )
    );
    assert_eq!(
        manual_parent_case(Bg::ParentRelative, true, true),
        (
            vec![
                "Map(K@P)".to_string(),
                format!("Map(P@root) Expose(P 0,0 300x200 c0) {K_EXPOSES} Expose(G 0,0 50x40 c0)"),
            ],
            unpainted
        )
    );
    let k_raw = "DmgR(K 0,0 120x10) DmgR(K 0,10 10x40) DmgR(K 60,10 60x40) DmgR(K 0,50 120x50)";
    let painted_k = [Some(false), Some(true), Some(true)];
    assert_eq!(
        manual_parent_case(Bg::Pixel(0x20_50a0), false, true),
        (
            vec![
                "Map(P@root) Expose(P 0,0 300x200 c0)".to_string(),
                format!("Map(K@P) {k_raw} {K_EXPOSES} DmgR(K 10,10 50x40) Expose(G 0,0 50x40 c0)"),
            ],
            painted_k
        )
    );
    assert_eq!(
        manual_parent_case(Bg::Pixel(0x20_50a0), true, true).0[1],
        format!(
            "Map(P@root) Expose(P 0,0 300x200 c0) {k_raw} {K_EXPOSES} DmgR(K 10,10 50x40) \
             Expose(G 0,0 50x40 c0)"
        )
    );
    assert_eq!(
        manual_parent_case(Bg::Pixel(0x20_50a0), false, false).0[1],
        format!("Map(K@P) DmgN(K 0,0 120x100) {K_EXPOSES} Expose(G 0,0 50x40 c0)")
    );
    assert_eq!(
        manual_parent_case(Bg::Pixel(0x20_50a0), true, false).0[1],
        format!(
            "Map(P@root) Expose(P 0,0 300x200 c0) DmgN(K 0,0 120x100) {K_EXPOSES} \
             Expose(G 0,0 50x40 c0)"
        )
    );
}

/// Case 3 of the probe in comp mode: W1 (with child W1c) and W2 mapped under
/// the overlay, then ReleaseOverlayWindow and UnredirectSubwindows(root) in
/// either order. The unredirect step's events, root's size.
fn overlay_release_case(bg: Bg, release_first: bool) -> (String, (u16, u16)) {
    let mut p = Probe::new();
    p.redirect_subwindows(ROOT_WINDOW.0);
    p.select(ROOT_WINDOW.0, EXPOSURE_AND_SUBSTRUCTURE);
    p.overlay(7);
    let cow = crate::resources::COMPOSITE_OVERLAY_WINDOW.0;
    p.names.push((cow, "COW"));
    p.select(cow, EXPOSURE_AND_SUBSTRUCTURE);
    let w1 = p.create("W1", ROOT_WINDOW.0, (50, 50, 200, 150), bg);
    let w2 = p.create("W2", ROOT_WINDOW.0, (150, 100, 200, 150), bg);
    let w1c = p.create("W1c", w1, (10, 10, 60, 40), bg);
    p.map(w1c);
    for w in [w1, w2] {
        p.watch_damage(w, false);
        p.watch_damage(w, true);
    }
    p.map(w1);
    p.map(w2);
    let _ = p.step();
    if release_first {
        p.overlay(8);
        let _ = p.step();
    }
    p.unredirect_subwindows(ROOT_WINDOW.0);
    let line = p.step();
    let root = p.state.resources.window(ROOT_WINDOW).unwrap();
    (line, (root.width, root.height))
}

/// `compFreeClientSubwindows` unredirects the children bottom-most first
/// (`composite/compalloc.c:450`), each `compRestoreWindow` copying its old
/// backing back with IncludeInferiors (`:232-256`): DAMAGE over the window
/// while the ones above are still redirected, nothing under the overlay.
#[test]
fn unredirect_after_overlay_release_damages_the_restored_windows() {
    for bg in [Bg::None, Bg::Pixel(0x80_8080)] {
        let (line, (w, h)) = overlay_release_case(bg, true);
        assert_eq!(
            line,
            format!(
                "Expose(root 0,0 {w}x{h} c0) DmgN(W1 0,0 200x150) DmgR(W1 0,0 200x150) \
                 DmgN(W2 0,0 200x150) DmgR(W2 0,0 200x150)"
            )
        );
        assert_eq!(overlay_release_case(bg, false).0, "");
    }
}

/// Case 1a of the probe: MapSubwindows on T with A, B (over A) and C.
fn map_subwindows_case(child_bg: Bg) -> String {
    let mut p = Probe::new();
    let t = p.create("T", ROOT_WINDOW.0, (50, 50, 300, 200), Bg::None);
    p.create("A", t, (10, 10, 80, 80), child_bg);
    p.create("B", t, (60, 60, 80, 80), child_bg);
    p.create("C", t, (190, 10, 80, 80), child_bg);
    p.watch_damage(t, true);
    p.map(t);
    let _ = p.step();
    p.map_subwindows(t);
    p.step()
}

/// `MapSubwindows` delivers MapNotify from firstChild down (`dix/window.c:2739`);
/// the exposures follow, top-most first.
#[test]
fn map_subwindows_notifies_top_most_first() {
    const EXPOSES: &str = "Expose(C 0,0 80x80 c0) Expose(B 0,0 80x80 c0) \
                           Expose(A 0,0 80x50 c1) Expose(A 0,50 50x30 c0)";
    assert_eq!(
        map_subwindows_case(Bg::None),
        format!("Map(C@T) Map(B@T) Map(A@T) {EXPOSES}")
    );
    assert_eq!(
        map_subwindows_case(Bg::Pixel(0x80_8080)),
        "Map(C@T) Map(B@T) Map(A@T) DmgR(T 190,10 80x80) Expose(C 0,0 80x80 c0) \
         DmgR(T 60,60 80x80) Expose(B 0,0 80x80 c0) DmgR(T 10,10 80x50) DmgR(T 10,60 50x30) \
         Expose(A 0,0 80x50 c1) Expose(A 0,50 50x30 c0)"
    );
}

/// Case 1b of the probe with both damage levels on P and R, NonEmpty then
/// RawRectangles each, as the probe creates them.
fn reparent_case(comp: bool) -> Vec<String> {
    let mut p = Probe::new();
    if comp {
        p.redirect_subwindows(ROOT_WINDOW.0);
    }
    let bg = Bg::Pixel(0x80_8080);
    let pw = p.create("P", ROOT_WINDOW.0, (50, 50, 300, 200), bg);
    let r = p.create("R", ROOT_WINDOW.0, (400, 60, 120, 100), bg);
    let k = p.create("Rk", r, (10, 10, 50, 40), bg);
    p.map(k);
    for w in [pw, r] {
        p.watch_damage(w, false);
        p.watch_damage(w, true);
    }
    let _ = p.step();
    p.map(pw);
    p.map(r);
    let mut lines = vec![p.step()];
    p.reparent(r, pw, 20, 30);
    lines.push(p.step());
    p.reparent(r, ROOT_WINDOW.0, 400, 60);
    lines.push(p.step());
    lines
}

/// Xorg reports each damage object in its pixmap's list order: new objects
/// are prepended (`miext/damage/damage.c:1482`), and a window given a pixmap,
/// by compositing or a reparent, moves its own to the head oldest first
/// (`damageSetWindowPixmap`, `:1547-1575`). Each reports its whole part of
/// the op before the next (`damageRegionProcessPending`, `:285-300`).
#[test]
fn damage_notifies_follow_the_damage_list_order() {
    const R_EXPOSES: &str = "Expose(R 0,0 120x10 c3) Expose(R 0,10 10x40 c2) \
                             Expose(R 60,10 60x40 c1) Expose(R 0,50 120x50 c0)";
    const R_RAW: &str =
        "DmgR(R 0,0 120x10) DmgR(R 0,10 10x40) DmgR(R 60,10 60x40) DmgR(R 0,50 120x50)";
    const P_RAW: &str =
        "DmgR(P 20,30 120x10) DmgR(P 20,40 10x40) DmgR(P 80,40 60x40) DmgR(P 20,80 120x50)";
    const INTO: &str = "Unmap(R@root) Reparent(R@root ->P 20,30) Reparent(R@P ->P 20,30) Map(R@P)";
    const OUT: &str = "Reparent(R@P ->root 400,60) Reparent(R@root ->root 400,60) Map(R@root)";
    const RK: &str = "DmgR(R 10,10 50x40) Expose(Rk 0,0 50x40 c0)";
    assert_eq!(
        reparent_case(false),
        [
            format!(
                "Map(P@root) DmgR(P 0,0 300x200) DmgN(P 0,0 300x200) Expose(P 0,0 300x200 c0) \
                 Map(R@root) {R_RAW} DmgN(R 0,0 120x100) {R_EXPOSES} {RK}"
            ),
            format!(
                "{INTO} DmgN(R 0,0 120x100) {R_RAW} {P_RAW} DmgN(P 0,0 300x200) {R_EXPOSES} \
                 DmgR(R 10,10 50x40) DmgR(P 30,40 50x40) Expose(Rk 0,0 50x40 c0)"
            ),
            format!(
                "Unmap(R@P) DmgR(P 20,30 120x100) DmgN(P 0,0 300x200) Expose(P 20,30 120x100 c0) \
                 {OUT} DmgN(R 0,0 120x100) {R_RAW} {R_EXPOSES} {RK}"
            ),
        ]
    );
    assert_eq!(
        reparent_case(true),
        [
            format!(
                "Map(P@root) DmgN(P 0,0 300x200) DmgR(P 0,0 300x200) Expose(P 0,0 300x200 c0) \
                 Map(R@root) DmgN(R 0,0 120x100) {R_RAW} {R_EXPOSES} {RK}"
            ),
            format!(
                "{INTO} DmgN(R 0,0 120x100) {R_RAW} DmgN(P 0,0 300x200) {P_RAW} {R_EXPOSES} \
                 DmgR(R 10,10 50x40) DmgR(P 30,40 50x40) Expose(Rk 0,0 50x40 c0)"
            ),
            format!(
                "Unmap(R@P) DmgN(P 0,0 300x200) DmgR(P 20,30 120x100) Expose(P 20,30 120x100 c0) \
                 {OUT} DmgN(R 0,0 120x100) {R_RAW} {R_EXPOSES} {RK}"
            ),
        ]
    );
}
