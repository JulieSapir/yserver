//! Plan RL0 Task 6: host requests and serial acks (spec §4.2.1, §4.4, §6.4).

use yserver_protocol::x11::ResourceId;

use super::{RecordingHost, assert_no_error};
use crate::{
    resources::ROOT_WINDOW,
    rootless::{
        CoordinatePolicy, Geometry, GeometryFields, HostOutput, HostRequest, HostSerial,
        RootlessEvent, ToplevelId,
    },
};

const W: u32 = 0x0020_0001;
const W2: u32 = 0x0020_0002;
const STRUCTURE_NOTIFY: u32 = 0x0002_0000;

fn mapped(host: &mut RecordingHost, wid: u32, size: (u16, u16), border: u16) -> ToplevelId {
    host.create_window(1, wid, ROOT_WINDOW.0, (10, 10), size, border, false);
    host.map_and_reconcile(1, wid)
}

fn moved(id: ToplevelId, x: i32, y: i32, serial: u64) -> HostRequest {
    HostRequest::Moved {
        id,
        x,
        y,
        serial: HostSerial(serial),
    }
}

/// The one `Configured` of a reconcile: (geometry, acked, client_fields).
fn configured(events: &[RootlessEvent]) -> (Geometry, Option<HostSerial>, GeometryFields) {
    match events {
        [
            RootlessEvent::Configured {
                geometry,
                acked,
                client_fields,
                ..
            },
        ] => (*geometry, *acked, *client_fields),
        other => panic!("expected one Configured: {other:?}"),
    }
}

fn geometry(host: &RecordingHost, wid: u32) -> (i16, i16, u16, u16) {
    let w = host.state.resources.window(ResourceId(wid)).unwrap();
    (w.x, w.y, w.width, w.height)
}

/// Rule 2 + ICCCM §4.1.5: a host move alone is acked with no client fields,
/// and the client gets the real ConfigureNotify and a synthetic one.
#[test]
fn host_move_alone_acks_with_no_client_fields_and_sends_synthetic_configure() {
    let mut host = RecordingHost::global();
    let id = mapped(&mut host, W, (50, 40), 0);
    let mut body = W.to_le_bytes().to_vec();
    body.extend_from_slice(&0x800u32.to_le_bytes());
    body.extend_from_slice(&STRUCTURE_NOTIFY.to_le_bytes());
    assert_no_error(&host.request(1, 2, 0, &body), "select");

    host.host(moved(id, 30, 40, 7));
    let (g, acked, fields) = configured(&host.reconcile());
    assert_eq!((g.x, g.y), (30, 40));
    assert_eq!(acked, Some(HostSerial(7)));
    assert!(fields.is_empty());
    let codes: Vec<u8> = host.read(1).chunks(32).map(|p| p[0]).collect();
    assert!(codes.contains(&22), "real ConfigureNotify: {codes:?}");
    assert!(
        codes.contains(&(22 | 0x80)),
        "synthetic ConfigureNotify: {codes:?}"
    );
}

/// Rule 3: host move 7 + client resize → acked 7, WIDTH|HEIGHT.
#[test]
fn host_move_plus_client_resize() {
    let mut host = RecordingHost::global();
    let id = mapped(&mut host, W, (50, 40), 0);
    host.host(moved(id, 30, 40, 7));
    host.configure(1, W, 0xc, &[60, 70]);
    let (_, acked, fields) = configured(&host.reconcile());
    assert_eq!(acked, Some(HostSerial(7)));
    assert_eq!(fields, GeometryFields::WIDTH | GeometryFields::HEIGHT);
}

/// Rule 3: a later host write of X|Y clears the client's.
#[test]
fn host7_client_move_host8() {
    let mut host = RecordingHost::global();
    let id = mapped(&mut host, W, (50, 40), 0);
    host.host(moved(id, 30, 40, 7));
    host.configure(1, W, 0x3, &[1, 2]);
    host.host(moved(id, 50, 60, 8));
    let (g, acked, fields) = configured(&host.reconcile());
    assert_eq!((g.x, g.y), (50, 60));
    assert_eq!(acked, Some(HostSerial(8)));
    assert!(fields.is_empty());
}

/// Rule 3: a client move after the host's is final. Mutation: drop client
/// fields when an ack is present (r0 `ChangeOrigin`).
#[test]
fn host7_then_client_move() {
    let mut host = RecordingHost::global();
    let id = mapped(&mut host, W, (50, 40), 0);
    host.host(moved(id, 30, 40, 7));
    host.configure(1, W, 0x3, &[1, 2]);
    let (g, acked, fields) = configured(&host.reconcile());
    assert_eq!((g.x, g.y), (1, 2));
    assert_eq!(acked, Some(HostSerial(7)));
    assert!(fields.contains(GeometryFields::X | GeometryFields::Y));
}

/// Rule 5 with ICCCM §4.1.2.3: min 10×17, inc 6×13, base 4×4;
/// `Resized(203, 100)` → 4 + ⌊199/6⌋·6 = 202, 4 + ⌊96/13⌋·13 = 95.
#[test]
fn host_resize_clamped_by_hints_reports_width_height() {
    let mut host = RecordingHost::global();
    host.create_window(1, W, ROOT_WINDOW.0, (10, 10), (50, 40), 0, false);
    host.change_property(
        1,
        W,
        "WM_NORMAL_HINTS",
        "WM_SIZE_HINTS",
        32,
        &[0x350, 0, 0, 0, 0, 10, 17, 0, 0, 6, 13, 0, 0, 0, 0, 4, 4, 1],
    );
    let id = host.map_and_reconcile(1, W);
    host.host(HostRequest::Resized {
        id,
        width: 203,
        height: 100,
        serial: HostSerial(3),
    });
    let (g, acked, fields) = configured(&host.reconcile());
    assert_eq!((g.width, g.height), (202, 95));
    assert_eq!(acked, Some(HostSerial(3)));
    assert!(fields.contains(GeometryFields::WIDTH | GeometryFields::HEIGHT));
}

/// Rule 2: a host request that changes nothing is still acked.
#[test]
fn noop_host_request_is_still_acked() {
    let mut host = RecordingHost::global();
    let id = mapped(&mut host, W, (50, 40), 0);
    host.host(moved(id, 10, 10, 4));
    let (g, acked, fields) = configured(&host.reconcile());
    assert_eq!((g.x, g.y), (10, 10));
    assert_eq!(acked, Some(HostSerial(4)));
    assert!(fields.is_empty());
}

/// Rule 6: a host restack is acked with `from_client: false`; a client
/// raise afterwards reports `from_client: true`.
#[test]
fn host_restack_acks_and_client_raise_sets_from_client() {
    let mut host = RecordingHost::global();
    let a = mapped(&mut host, W, (50, 40), 0);
    let b = mapped(&mut host, W2, (50, 40), 0);
    host.host(HostRequest::Restacked {
        id: a,
        above: Some(b),
        serial: HostSerial(5),
    });
    assert_eq!(
        host.reconcile(),
        vec![RootlessEvent::Restacked {
            id: a,
            above: Some(b),
            acked: Some(HostSerial(5)),
            from_client: false,
        }]
    );
    host.configure(1, W2, 0x40, &[0]);
    assert_eq!(
        host.reconcile(),
        vec![RootlessEvent::Restacked {
            id: b,
            above: Some(a),
            acked: None,
            from_client: true,
        }]
    );
}

/// Spec §4.0: a host size is native content, border included.
/// Mutation: drop `−2bw`.
#[test]
fn resized_on_bordered_window_subtracts_two_bw() {
    let mut host = RecordingHost::global();
    let id = mapped(&mut host, W, (50, 40), 5);
    host.host(HostRequest::Resized {
        id,
        width: 110,
        height: 60,
        serial: HostSerial(1),
    });
    assert_eq!((geometry(&host, W).2, geometry(&host, W).3), (100, 50));
}

fn set_protocols(host: &mut RecordingHost, names: &[&str]) {
    let atoms: Vec<u32> = names.iter().map(|n| host.atom(n)).collect();
    host.change_property(1, W, "WM_PROTOCOLS", "ATOM", 32, &atoms);
}

/// ICCCM §4.2.8.1: `WM_PROTOCOLS` ClientMessage `[WM_DELETE_WINDOW, time]`,
/// sent (SendEvent) to the window's client.
#[test]
fn close_requested_sends_delete_window_when_listed() {
    let mut host = RecordingHost::global();
    host.create_window(1, W, ROOT_WINDOW.0, (10, 10), (50, 40), 0, false);
    set_protocols(&mut host, &["WM_DELETE_WINDOW"]);
    let id = host.map_and_reconcile(1, W);
    host.read(1);
    host.host(HostRequest::CloseRequested { id });
    let out = host.read(1);
    let msg = out
        .chunks(32)
        .find(|p| p[0] == 33 | 0x80)
        .expect("a sent ClientMessage");
    let word = |at: usize| u32::from_le_bytes(msg[at..at + 4].try_into().unwrap());
    assert_eq!(msg[1], 32);
    assert_eq!(word(4), W);
    assert_eq!(word(8), host.atom("WM_PROTOCOLS"));
    assert_eq!(word(12), host.atom("WM_DELETE_WINDOW"));
    assert!(word(16) <= host.state.timestamp_now(), "the server time");
}

/// Decision 12: a close request never kills. Mutation: KillClient without
/// `WM_DELETE_WINDOW`.
#[test]
fn close_requested_without_protocol_never_kills() {
    let mut host = RecordingHost::global();
    let id = mapped(&mut host, W, (50, 40), 0);
    host.read(1);
    host.host(HostRequest::CloseRequested { id });
    assert!(host.state.clients.contains_key(&1));
    assert!(!host.read(1).chunks(32).any(|p| p[0] & 0x7f == 33));
}

/// Decision 12: only `ForceKill` kills.
#[test]
fn force_kill_disconnects_owner() {
    let mut host = RecordingHost::global();
    let id = mapped(&mut host, W, (50, 40), 0);
    host.host(HostRequest::ForceKill { id });
    assert!(!host.state.clients.contains_key(&1));
    assert!(host.state.resources.window(ResourceId(W)).is_none());
}

/// Spec §5: an Opaque host never positions windows.
#[test]
fn moved_is_dropped_under_opaque_policy() {
    let mut host = RecordingHost::new();
    assert_eq!(
        host.state.rootless.as_ref().unwrap().policy(),
        CoordinatePolicy::Opaque
    );
    let id = mapped(&mut host, W, (50, 40), 0);
    host.host(moved(id, 30, 40, 7));
    assert_eq!(geometry(&host, W), (10, 10, 50, 40));
    assert_eq!(host.reconcile(), vec![]);
}

/// Spec §4.2: an id names one mapped lifetime; requests carrying an earlier
/// one are dropped. Mutation: resolve ids by XID, or keep ids across an unmap.
#[test]
fn stale_id_requests_are_dropped() {
    let mut host = RecordingHost::global();
    host.create_window(1, W, ROOT_WINDOW.0, (10, 10), (50, 40), 0, false);
    set_protocols(&mut host, &["WM_DELETE_WINDOW"]);
    let id1 = host.map_and_reconcile(1, W);
    host.unmap(1, W);
    host.reconcile();
    let id2 = host.map_and_reconcile(1, W);
    host.read(1);

    host.host(HostRequest::CloseRequested { id: id1 });
    assert!(!host.read(1).chunks(32).any(|p| p[0] & 0x7f == 33));
    host.host(moved(id1, 30, 40, 9));
    host.host(HostRequest::Resized {
        id: id1,
        width: 99,
        height: 99,
        serial: HostSerial(9),
    });
    host.host(HostRequest::Restacked {
        id: id1,
        above: None,
        serial: HostSerial(9),
    });
    host.host(HostRequest::ForceKill { id: id1 });
    assert!(host.state.clients.contains_key(&1));
    assert_eq!(geometry(&host, W), (10, 10, 50, 40));
    assert_eq!(host.reconcile(), vec![], "no ack for a stale id");

    host.host(moved(id2, 30, 40, 10));
    let events = host.reconcile();
    assert!(
        matches!(events.as_slice(),
            [RootlessEvent::Configured { id, acked: Some(HostSerial(10)), .. }] if *id == id2),
        "{events:?}"
    );
    host.host(HostRequest::CloseRequested { id: id2 });
    assert!(host.read(1).chunks(32).any(|p| p[0] == 33 | 0x80));
}

/// D7: exactly one host output resizes the root and its RandR output.
#[test]
fn screen_layout_accepts_one_output() {
    let mut host = RecordingHost::global();
    host.host(HostRequest::ScreenLayout {
        outputs: vec![HostOutput {
            x: 0,
            y: 0,
            width: 1280,
            height: 800,
        }],
    });
    let root = host.state.resources.window(ROOT_WINDOW).unwrap();
    assert_eq!((root.width, root.height), (1280, 800));
    let output = &host.state.randr.outputs[0];
    assert_eq!((output.width, output.height), (1280, 800));
    assert_eq!(
        host.reconcile(),
        vec![RootlessEvent::ScreenChanged { root: (1280, 800) }]
    );
}

/// D7: any other count is rejected. Mutation: resize to the bounding box.
#[test]
fn screen_layout_rejects_two_outputs() {
    let mut host = RecordingHost::global();
    let root = host
        .state
        .resources
        .window(ROOT_WINDOW)
        .map(|w| (w.width, w.height));
    host.host(HostRequest::ScreenLayout {
        outputs: vec![
            HostOutput {
                x: 0,
                y: 0,
                width: 1280,
                height: 800,
            },
            HostOutput {
                x: 1280,
                y: 0,
                width: 1920,
                height: 1080,
            },
        ],
    });
    assert_eq!(
        host.state
            .resources
            .window(ROOT_WINDOW)
            .map(|w| (w.width, w.height)),
        root
    );
    assert_eq!(host.reconcile(), vec![]);
}
