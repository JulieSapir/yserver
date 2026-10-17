//! Request-level replays of `tools/expose-order-probe.c`: one client drives
//! the requests and watches the events, printed the probe's way so each
//! expectation is a line of the Xorg log.

use super::*;

const EXPOSURE_AND_SUBSTRUCTURE: u32 = 0x0000_8000 | 0x0008_0000;
const DAMAGE_MAJOR: u8 = 143;
const DAMAGE_RAW: u8 = 0;
const DAMAGE_NON_EMPTY: u8 = 3;

/// A recorded fill: x, y, width, height.
pub(super) type Fill = (i16, i16, u16, u16);

/// Background of a probe window.
#[derive(Clone, Copy)]
pub(super) enum Bg {
    None,
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
    pub(super) fn new() -> Self {
        let mut state = ServerState::new();
        let peer = install_client(&mut state, 1);
        Self {
            state,
            backend: RecordingBackend::new(),
            peer,
            names: vec![(ROOT_WINDOW.0, "root")],
            damages: Vec::new(),
            next_xid: 0x0020_0001,
            sequence: 0,
        }
    }

    pub(super) fn request(&mut self, opcode: u8, data: u8, body: &[u8]) {
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
        .expect("request through core dispatcher");
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

    pub(super) fn clear(&mut self, window: u32, rect: (i16, i16, u16, u16)) {
        let mut body = window.to_le_bytes().to_vec();
        for half in [
            rect.0.to_le_bytes(),
            rect.1.to_le_bytes(),
            rect.2.to_le_bytes(),
            rect.3.to_le_bytes(),
        ] {
            body.extend_from_slice(&half);
        }
        self.request(61, 1, &body);
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
