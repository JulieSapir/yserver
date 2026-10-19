//! Property data byte order across big- and little-endian clients.
//!
//! Every expected value below was measured on Xorg 21.1.24 (vng, a raw
//! `B` client and a raw `l` client on the same window): Xorg swaps
//! format-16/32 data on the way in (`dix/swapreq.c:209-229`) and on the
//! way out (`dix/property.c:535-549`), so a property reads the same to
//! every client whatever byte order wrote it. Format 8 is never swapped.

use super::*;

const BE: u32 = 1;
const LE: u32 = 2;
const WIN: u32 = 0x0040_0001;
const CARDINAL: u32 = 6;
const INTEGER: u32 = 19;
const STRING: u32 = 31;

struct Pair {
    state: ServerState,
    be: UnixStream,
    le: UnixStream,
    seq: u16,
}

impl Pair {
    fn new() -> Self {
        let mut state = ServerState::new();
        let be = install_client(&mut state, BE);
        let le = install_client(&mut state, LE);
        state.clients.get_mut(&BE).expect("BE client").byte_order = ClientByteOrder::BigEndian;
        create_root_child(&mut state, WIN);
        Self {
            state,
            be,
            le,
            seq: 0,
        }
    }

    fn order(client: u32) -> ClientByteOrder {
        if client == BE {
            ClientByteOrder::BigEndian
        } else {
            ClientByteOrder::LittleEndian
        }
    }

    fn atom(&mut self, name: &str) -> u32 {
        self.state.atoms.intern(name, false).0
    }

    /// Send a request built in `client`'s byte order through the reader's
    /// swap and `process_request`; return what that client received.
    fn send(&mut self, client: u32, opcode: u8, data: u8, body: WireBody) -> Vec<u8> {
        let mut body = body.1;
        yserver_protocol::x11::request_swap::swap_request_body(
            opcode,
            data,
            Self::order(client),
            &mut body,
        );
        self.seq += 1;
        let mut backend = RecordingBackend::new();
        process_request(
            &mut self.state,
            &mut backend,
            ClientId(client),
            SequenceNumber(self.seq),
            RequestHeader {
                opcode,
                data,
                length_units: u32::try_from((4 + body.len()).div_ceil(4)).unwrap(),
            },
            &body,
            None,
        )
        .expect("request");
        self.read(client)
    }

    fn read(&mut self, client: u32) -> Vec<u8> {
        if client == BE {
            read_all_available(&mut self.be)
        } else {
            read_all_available(&mut self.le)
        }
    }

    fn change(&mut self, client: u32, mode: u8, prop: u32, ty: u32, format: u8, items: &[u32]) {
        let mut body = WireBody(Self::order(client), Vec::new())
            .u32(WIN)
            .u32(prop)
            .u32(ty)
            .bytes(&[format, 0, 0, 0])
            .u32(u32::try_from(items.len()).unwrap());
        for &item in items {
            body = match format {
                8 => body.bytes(&[u8::try_from(item).unwrap()]),
                16 => body.u16(u16::try_from(item).unwrap()),
                _ => body.u32(item),
            };
        }
        while !body.1.len().is_multiple_of(4) {
            body.1.push(0);
        }
        let reply = self.send(client, 18, mode, body);
        assert!(reply.is_empty(), "ChangeProperty answered {reply:02x?}");
    }

    fn get(&mut self, client: u32, prop: u32, ty: u32, offset: u32, length: u32) -> GetReply {
        self.get_with(client, prop, ty, offset, length, false)
    }

    fn get_with(
        &mut self,
        client: u32,
        prop: u32,
        ty: u32,
        offset: u32,
        length: u32,
        delete: bool,
    ) -> GetReply {
        let body = WireBody(Self::order(client), Vec::new())
            .u32(WIN)
            .u32(prop)
            .u32(ty)
            .u32(offset)
            .u32(length);
        let reply = self.send(client, 20, u8::from(delete), body);
        assert_eq!(reply[0], 1, "GetProperty reply, got {reply:02x?}");
        let word = |at: usize| {
            let b: [u8; 4] = reply[at..at + 4].try_into().unwrap();
            if client == BE {
                u32::from_be_bytes(b)
            } else {
                u32::from_le_bytes(b)
            }
        };
        let format = reply[1];
        let nitems = word(16);
        let len = usize::try_from(nitems).unwrap() * usize::from(format.max(8) / 8);
        GetReply {
            format,
            ty: word(8),
            bytes_after: word(12),
            nitems,
            raw: hex(&reply[32..32 + len]),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct GetReply {
    format: u8,
    ty: u32,
    bytes_after: u32,
    nitems: u32,
    raw: String,
}

fn reply(format: u8, ty: u32, nitems: u32, bytes_after: u32, raw: &str) -> GetReply {
    GetReply {
        format,
        ty,
        bytes_after,
        nitems,
        raw: raw.to_string(),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// RL0's `WM_NORMAL_HINTS` case in miniature: a BE client's CARDINAL/32
/// is stored in server order and reads as the same numbers to both.
#[test]
fn be_written_format32_reads_the_same_values_to_le_and_be() {
    let mut p = Pair::new();
    let prop = p.atom("PROBE_BE32");
    p.change(BE, 0, prop, CARDINAL, 32, &[0x0102_0304, 5]);

    let stored = p
        .state
        .resources
        .window_property(ResourceId(WIN), AtomId(prop))
        .expect("stored");
    assert_eq!(hex(&stored.data), "0403020105000000", "server order");

    assert_eq!(
        p.get(LE, prop, 0, 0, 1000),
        reply(32, CARDINAL, 2, 0, "0403020105000000")
    );
    assert_eq!(
        p.get(BE, prop, 0, 0, 1000),
        reply(32, CARDINAL, 2, 0, "0102030400000005")
    );
}

#[test]
fn le_written_format16_and_32_read_swapped_to_a_be_client() {
    let mut p = Pair::new();
    let p32 = p.atom("PROBE_LE32");
    let p16 = p.atom("PROBE_LE16");
    p.change(LE, 0, p32, CARDINAL, 32, &[0x0102_0304, 5]);
    p.change(LE, 0, p16, INTEGER, 16, &[0x0102, 0x0304, 7]);

    assert_eq!(
        p.get(BE, p32, 0, 0, 1000),
        reply(32, CARDINAL, 2, 0, "0102030400000005")
    );
    assert_eq!(
        p.get(BE, p16, 0, 0, 1000),
        reply(16, INTEGER, 3, 0, "010203040007")
    );
    assert_eq!(
        p.get(LE, p16, 0, 0, 1000),
        reply(16, INTEGER, 3, 0, "020104030700")
    );
}

#[test]
fn be_written_format16_reads_the_same_values_to_le_and_be() {
    let mut p = Pair::new();
    let prop = p.atom("PROBE_BE16");
    p.change(BE, 0, prop, INTEGER, 16, &[0x0102, 0x0304, 7]);
    assert_eq!(
        p.get(LE, prop, 0, 0, 1000),
        reply(16, INTEGER, 3, 0, "020104030700")
    );
    assert_eq!(
        p.get(BE, prop, 0, 0, 1000),
        reply(16, INTEGER, 3, 0, "010203040007")
    );
}

#[test]
fn format8_is_never_swapped() {
    let mut p = Pair::new();
    let prop = p.atom("PROBE_BE8");
    p.change(BE, 0, prop, STRING, 8, &[0x61, 0x62, 1, 2, 3]);
    assert_eq!(
        p.get(LE, prop, 0, 0, 1000),
        reply(8, STRING, 5, 0, "6162010203")
    );
    assert_eq!(
        p.get(BE, prop, 0, 0, 1000),
        reply(8, STRING, 5, 0, "6162010203")
    );
    assert_eq!(p.get(BE, prop, 0, 1, 1), reply(8, STRING, 1, 0, "03"));
}

/// Append and Prepend from both byte orders concatenate values, not bytes.
#[test]
fn mixed_order_append_and_prepend_concatenate_values() {
    let mut p = Pair::new();
    let prop = p.atom("PROBE_BE32");
    p.change(BE, 0, prop, CARDINAL, 32, &[0x0102_0304, 5]);
    p.change(BE, 2, prop, CARDINAL, 32, &[0x0A0B_0C0D]);
    p.change(BE, 1, prop, CARDINAL, 32, &[0x1122_3344]);
    p.change(LE, 2, prop, CARDINAL, 32, &[0x5566_7788]);

    assert_eq!(
        p.get(BE, prop, 0, 0, 1000),
        reply(
            32,
            CARDINAL,
            5,
            0,
            "1122334401020304000000050a0b0c0d55667788"
        )
    );
    assert_eq!(
        p.get(LE, prop, 0, 0, 1000),
        reply(
            32,
            CARDINAL,
            5,
            0,
            "4433221104030201050000000d0c0b0a88776655"
        )
    );
    // long_offset/long_length windows are swapped per item too.
    assert_eq!(
        p.get(BE, prop, 0, 1, 1),
        reply(32, CARDINAL, 1, 12, "01020304")
    );
    assert_eq!(
        p.get(LE, prop, 0, 1, 1),
        reply(32, CARDINAL, 1, 12, "04030201")
    );
    assert_eq!(
        p.get(BE, prop, 0, 2, 5),
        reply(32, CARDINAL, 3, 0, "000000050a0b0c0d55667788")
    );
}

#[test]
fn format16_windows_swap_each_item() {
    let mut p = Pair::new();
    let prop = p.atom("PROBE_BE16");
    p.change(BE, 0, prop, INTEGER, 16, &[0x0102, 0x0304, 7]);
    assert_eq!(
        p.get(BE, prop, 0, 0, 1),
        reply(16, INTEGER, 2, 2, "01020304")
    );
    assert_eq!(
        p.get(LE, prop, 0, 0, 1),
        reply(16, INTEGER, 2, 2, "02010403")
    );
    assert_eq!(p.get(BE, prop, 0, 1, 1), reply(16, INTEGER, 1, 0, "0007"));
    assert_eq!(p.get(LE, prop, 0, 1, 1), reply(16, INTEGER, 1, 0, "0700"));
}

#[test]
fn delete_reply_is_in_the_reader_order_and_notifies_both() {
    let mut p = Pair::new();
    let prop = p.atom("PROBE_LE32");
    p.change(LE, 0, prop, CARDINAL, 32, &[0x0102_0304, 5]);
    // PropertyChangeMask on the window for the LE client.
    let cwa = WireBody(ClientByteOrder::LittleEndian, Vec::new())
        .u32(WIN)
        .u32(0x800)
        .u32(0x0040_0000);
    p.send(LE, 2, 0, cwa);

    assert_eq!(
        p.get_with(BE, prop, 0, 0, 1000, true),
        reply(32, CARDINAL, 2, 0, "0102030400000005")
    );
    let event = p.read(LE);
    assert_eq!(event.len(), 32);
    assert_eq!(event[0], 28, "PropertyNotify");
    assert_eq!(u32::from_le_bytes(event[8..12].try_into().unwrap()), prop);
    assert_eq!(event[16], 1, "PropertyDelete");
    assert_eq!(p.get(LE, prop, 0, 0, 1000), reply(0, 0, 0, 0, ""));
}

#[test]
fn list_properties_atoms_are_in_the_reader_order() {
    let mut p = Pair::new();
    let prop = p.atom("PROBE_BE32");
    p.change(BE, 0, prop, CARDINAL, 32, &[1]);
    let reply = p.send(
        BE,
        21,
        0,
        WireBody(ClientByteOrder::BigEndian, Vec::new()).u32(WIN),
    );
    assert_eq!(u16::from_be_bytes(reply[8..10].try_into().unwrap()), 1);
    assert_eq!(u32::from_be_bytes(reply[32..36].try_into().unwrap()), prop);
}

/// RandR output properties have their own store; Xorg swaps them on input
/// (`SProcRRChangeOutputProperty`) and on output (`ProcRRGetOutputProperty`),
/// so a BE writer reads its own `INTEGER/32` back unchanged.
#[test]
fn randr_output_property_reads_the_same_values_to_le_and_be() {
    use yserver_protocol::x11::randr as x11randr;

    let mut p = Pair::new();
    let output = p.state.randr.outputs[0].output_id;
    let prop = p.atom("PROBE_EXT32");
    let change = WireBody(ClientByteOrder::BigEndian, Vec::new())
        .u32(output)
        .u32(prop)
        .u32(INTEGER)
        .bytes(&[32, 0, 0, 0])
        .u32(2)
        .u32(0x0102_0304)
        .u32(5);
    p.send(BE, 128, x11randr::RR_CHANGE_OUTPUT_PROPERTY, change);
    for (client, raw) in [(BE, "0102030400000005"), (LE, "0403020105000000")] {
        let get = WireBody(Pair::order(client), Vec::new())
            .u32(output)
            .u32(prop)
            .u32(0)
            .u32(0)
            .u32(100)
            .bytes(&[0, 0, 0, 0]);
        let reply = p.send(client, 128, x11randr::RR_GET_OUTPUT_PROPERTY, get);
        assert_eq!(reply[0], 1, "GetOutputProperty reply, got {reply:02x?}");
        assert_eq!(reply[1], 32);
        assert_eq!(hex(&reply[32..40]), raw, "client {client}");
    }
}

/// A type mismatch returns the property's type and format with no data,
/// and Xorg's `bytes_after` is the property's ITEM count
/// (`reply.bytesAfter = pProp->size`, `dix/property.c:492-501`), not the
/// byte length the protocol text describes.
#[test]
fn type_mismatch_bytes_after_counts_items() {
    let mut p = Pair::new();
    let prop = p.atom("PROBE_LE32");
    p.change(LE, 0, prop, CARDINAL, 32, &[1, 2, 3, 4, 5]);
    assert_eq!(
        p.get(LE, prop, INTEGER, 0, 1000),
        reply(32, CARDINAL, 0, 5, "")
    );
    let p16 = p.atom("PROBE_LE16");
    p.change(LE, 0, p16, INTEGER, 16, &[1, 2, 3]);
    assert_eq!(
        p.get(LE, p16, CARDINAL, 0, 1000),
        reply(16, INTEGER, 0, 3, "")
    );
}

// ── RotateProperties ──
//
// Expected values measured on Xorg 21.1 (`prop-order` vng golden): rotating
// by `delta` moves property i's type, format and data to property
// `(i + delta) mod n`; one PropertyNotify NewValue per listed property, in
// list order; nothing (no events) when `delta mod n == 0`. Errors are
// checked atom by atom (`dix/property.c:157-176`).

impl Pair {
    fn rotate(&mut self, client: u32, window: u32, atoms: &[u32], delta: i16) -> Vec<u8> {
        let mut body = WireBody(Self::order(client), Vec::new())
            .u32(window)
            .u16(u16::try_from(atoms.len()).unwrap())
            .u16(delta.cast_unsigned());
        for &atom in atoms {
            body = body.u32(atom);
        }
        self.send(client, 114, 0, body)
    }

    fn select_property_change(&mut self, client: u32) {
        let cwa = WireBody(Self::order(client), Vec::new())
            .u32(WIN)
            .u32(0x800)
            .u32(0x0040_0000);
        self.send(client, 2, 0, cwa);
    }

    /// The three `rotate` properties: CARDINAL/32, INTEGER/16, STRING/8.
    fn rotate_setup(&mut self) -> [u32; 3] {
        let r = [
            self.atom("PROBE_R1"),
            self.atom("PROBE_R2"),
            self.atom("PROBE_R3"),
        ];
        self.change(BE, 0, r[0], CARDINAL, 32, &[0x0102_0304]);
        self.change(BE, 0, r[1], INTEGER, 16, &[0x0102, 0x0304, 7]);
        self.change(BE, 0, r[2], STRING, 8, &[0x61, 0x62, 0x63]);
        self.read(LE);
        r
    }
}

fn r1_value() -> GetReply {
    reply(32, CARDINAL, 1, 0, "04030201")
}
fn r2_value() -> GetReply {
    reply(16, INTEGER, 3, 0, "020104030700")
}
fn r3_value() -> GetReply {
    reply(8, STRING, 3, 0, "616263")
}

/// `PropertyNotify` (atom, state) pairs in an LE client's event stream.
fn property_notifies(events: &[u8]) -> Vec<(u32, u8)> {
    assert!(events.len().is_multiple_of(32), "{events:02x?}");
    events
        .chunks_exact(32)
        .map(|ev| {
            assert_eq!(ev[0], 28, "PropertyNotify, got {ev:02x?}");
            assert_eq!(u32::from_le_bytes(ev[4..8].try_into().unwrap()), WIN);
            (u32::from_le_bytes(ev[8..12].try_into().unwrap()), ev[16])
        })
        .collect()
}

/// (code, value, major) of the single X error in `bytes`.
fn x_error(bytes: &[u8], order: ClientByteOrder) -> (u8, u32, u8) {
    assert_eq!(bytes.len(), 32, "one error, got {bytes:02x?}");
    assert_eq!(bytes[0], 0, "error, got {bytes:02x?}");
    let v: [u8; 4] = bytes[4..8].try_into().unwrap();
    let value = if order == ClientByteOrder::BigEndian {
        u32::from_be_bytes(v)
    } else {
        u32::from_le_bytes(v)
    };
    (bytes[1], value, bytes[10])
}

fn assert_values(p: &mut Pair, r: [u32; 3], want: [GetReply; 3]) {
    for (atom, want) in r.into_iter().zip(want) {
        assert_eq!(p.get(LE, atom, 0, 0, 1000), want);
    }
}

#[test]
fn rotate_plus_one_moves_each_value_to_the_next_property() {
    let mut p = Pair::new();
    p.select_property_change(LE);
    let r = p.rotate_setup();
    assert!(p.rotate(BE, WIN, &r, 1).is_empty());
    assert_eq!(
        property_notifies(&p.read(LE)),
        vec![(r[0], 0), (r[1], 0), (r[2], 0)]
    );
    // Type and format travel with the data.
    assert_values(&mut p, r, [r3_value(), r1_value(), r2_value()]);
    // -1 from the LE client (the selecting one) rotates back.
    assert_eq!(
        property_notifies(&p.rotate(LE, WIN, &r, -1)),
        vec![(r[0], 0), (r[1], 0), (r[2], 0)]
    );
    assert_values(&mut p, r, [r1_value(), r2_value(), r3_value()]);
}

#[test]
fn rotate_by_a_multiple_of_n_changes_nothing_and_sends_no_events() {
    let mut p = Pair::new();
    p.select_property_change(LE);
    let r = p.rotate_setup();
    for delta in [0, 3, -3, 6] {
        assert!(p.rotate(BE, WIN, &r, delta).is_empty());
        assert!(p.read(LE).is_empty(), "delta {delta} sent events");
        assert_values(&mut p, r, [r1_value(), r2_value(), r3_value()]);
    }
}

#[test]
fn rotate_by_n_plus_one_equals_plus_one_and_minus_n_plus_one_equals_minus_one() {
    let mut p = Pair::new();
    let r = p.rotate_setup();
    assert!(p.rotate(BE, WIN, &r, 4).is_empty());
    assert_values(&mut p, r, [r3_value(), r1_value(), r2_value()]);
    assert!(p.rotate(LE, WIN, &r, -4).is_empty());
    assert_values(&mut p, r, [r1_value(), r2_value(), r3_value()]);
}

#[test]
fn rotate_events_follow_the_request_list_order() {
    let mut p = Pair::new();
    p.select_property_change(LE);
    let r = p.rotate_setup();
    assert!(p.rotate(BE, WIN, &[r[2], r[0], r[1]], 1).is_empty());
    assert_eq!(
        property_notifies(&p.read(LE)),
        vec![(r[2], 0), (r[0], 0), (r[1], 0)]
    );
    // R1 <- R3, R2 <- R1, R3 <- R2: the same as rotating [R1, R2, R3] by 1.
    assert_values(&mut p, r, [r3_value(), r1_value(), r2_value()]);
}

/// A BE client's nPositions must be swapped: unswapped, 1 reads as 0x0100,
/// which is 0 mod 2 and would rotate nothing.
#[test]
fn rotate_two_properties_by_one_from_a_big_endian_client() {
    let mut p = Pair::new();
    let r4 = p.atom("PROBE_R4");
    let r5 = p.atom("PROBE_R5");
    p.change(BE, 0, r4, CARDINAL, 32, &[4]);
    p.change(BE, 0, r5, INTEGER, 16, &[5]);
    assert!(p.rotate(BE, WIN, &[r4, r5], 1).is_empty());
    assert_eq!(p.get(BE, r4, 0, 0, 1000), reply(16, INTEGER, 1, 0, "0005"));
    assert_eq!(
        p.get(BE, r5, 0, 0, 1000),
        reply(32, CARDINAL, 1, 0, "00000004")
    );
}

#[test]
fn rotate_an_empty_list_is_a_no_op() {
    let mut p = Pair::new();
    assert!(p.rotate(BE, WIN, &[], 1).is_empty());
}

#[test]
fn rotate_errors_match_xorg_and_change_nothing() {
    const BAD_WINDOW: u8 = 3;
    const BAD_ATOM: u8 = 5;
    const BAD_MATCH: u8 = 8;
    const BAD_ATOM_ID: u32 = 0x00FF_FFF0;
    const NO_WINDOW: u32 = 0x0060_0001;

    let mut p = Pair::new();
    p.select_property_change(LE);
    let r = p.rotate_setup();
    let miss = p.atom("PROBE_RMISSING");

    let cases = [
        (BE, WIN, vec![r[0], miss, r[1]], 1, (BAD_MATCH, miss)),
        // A duplicate of the FIRST atom: Xorg's errorValue is still the
        // window id the lookup left there.
        (LE, WIN, vec![r[0], r[1], r[0]], 1, (BAD_MATCH, WIN)),
        // A later duplicate: the previous atom's property lookup set it.
        (LE, WIN, vec![r[0], r[1], r[1]], 1, (BAD_MATCH, r[0])),
        (
            BE,
            WIN,
            vec![r[0], r[1], BAD_ATOM_ID],
            1,
            (BAD_ATOM, BAD_ATOM_ID),
        ),
        (BE, WIN, vec![r[0], 0], 1, (BAD_ATOM, 0)),
        (LE, WIN, vec![miss, BAD_ATOM_ID], 1, (BAD_MATCH, miss)),
        (LE, WIN, vec![BAD_ATOM_ID, miss], 1, (BAD_ATOM, BAD_ATOM_ID)),
        (BE, NO_WINDOW, r.to_vec(), 1, (BAD_WINDOW, NO_WINDOW)),
        (BE, NO_WINDOW, vec![], 1, (BAD_WINDOW, NO_WINDOW)),
        (LE, WIN, vec![r[0], miss], 0, (BAD_MATCH, miss)),
    ];
    for (client, window, atoms, delta, (code, value)) in cases {
        let got = p.rotate(client, window, &atoms, delta);
        assert_eq!(
            x_error(&got, Pair::order(client)),
            (code, value, 114),
            "{atoms:x?} on {window:#x}"
        );
        assert!(p.read(LE).is_empty(), "{atoms:x?} sent events");
        assert_values(&mut p, r, [r1_value(), r2_value(), r3_value()]);
    }
}
