//! Plan RL0 Task 5: the built-in WM policy (spec §6).

use yserver_protocol::x11::{ClientByteOrder, ResourceId, error::BAD_ACCESS};

use super::{RecordingHost, assert_no_error, error_code};
use crate::{
    resources::{ROOT_WINDOW, ROOTLESS_WM_CHECK_WINDOW},
    rootless::{HostRequest, Layer, NetWmState, ResizeEdge, RootlessEvent, ToplevelId, WmRequest},
};

const W: u32 = 0x0020_0001;
const W2: u32 = 0x0020_0002;
const BE: u32 = 2;
const LISTENER: u32 = 3;
const PROPERTY_CHANGE: u32 = 0x0040_0000;
const SUBSTRUCTURE_NOTIFY: u32 = 0x0008_0000;
const SUBSTRUCTURE_REDIRECT: u32 = 0x0010_0000;

/// (type name, format-32 words) of a stored property.
fn words(host: &RecordingHost, window: ResourceId, name: &str) -> Option<(String, Vec<u32>)> {
    let atom = host.state.atoms.id_for(name)?;
    let v = host.state.resources.window_property(window, atom)?;
    let ty = host.state.atoms.name(v.r#type).unwrap_or("?").to_owned();
    let w = v
        .data
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    Some((ty, w))
}

fn atom(host: &RecordingHost, name: &str) -> u32 {
    host.state.atoms.id_for(name).expect(name).0
}

fn select_input(host: &mut RecordingHost, client: u32, window: u32, mask: u32) -> Vec<u8> {
    let mut body = window.to_le_bytes().to_vec();
    body.extend_from_slice(&0x800u32.to_le_bytes()); // CWEventMask
    body.extend_from_slice(&mask.to_le_bytes());
    host.request(client, 2, 0, &body)
}

/// Core event codes (sent bit cleared) in a client's output.
fn event_codes(bytes: &[u8]) -> Vec<u8> {
    bytes.chunks(32).map(|p| p[0] & 0x7f).collect()
}

/// SendEvent(root, SubstructureRedirect|SubstructureNotify) of a format-32
/// ClientMessage, encoded in `client`'s byte order (EWMH §3 / ICCCM §4.1.4).
fn root_message(host: &mut RecordingHost, client: u32, window: u32, ty: &str, l: [u32; 5]) {
    let be = host.state.clients[&client].byte_order == ClientByteOrder::BigEndian;
    let word = |v: u32| if be { v.to_be_bytes() } else { v.to_le_bytes() };
    let ty = host.atom(ty);
    let mut body = Vec::new();
    body.extend_from_slice(&word(ROOT_WINDOW.0));
    body.extend_from_slice(&word(SUBSTRUCTURE_REDIRECT | SUBSTRUCTURE_NOTIFY));
    body.extend_from_slice(&[33, 32, 0, 0]);
    body.extend_from_slice(&word(window));
    body.extend_from_slice(&word(ty));
    for v in l {
        body.extend_from_slice(&word(v));
    }
    let out = host.request(client, 25, 0, &body);
    assert_no_error(&out, "SendEvent");
}

/// EWMH §3.1, §3.13; ICCCM §2.8.
#[test]
fn enable_announces_ewmh() {
    let mut host = RecordingHost::new();
    let check = ROOTLESS_WM_CHECK_WINDOW;
    assert_eq!(
        words(&host, ROOT_WINDOW, "_NET_SUPPORTING_WM_CHECK"),
        Some(("WINDOW".to_owned(), vec![check.0]))
    );
    assert_eq!(
        words(&host, check, "_NET_SUPPORTING_WM_CHECK"),
        Some(("WINDOW".to_owned(), vec![check.0]))
    );
    let name = host.state.atoms.id_for("_NET_WM_NAME").unwrap();
    let v = host.state.resources.window_property(check, name).unwrap();
    assert_eq!(host.state.atoms.name(v.r#type), Some("UTF8_STRING"));
    assert_eq!(v.data, b"yserver");
    let expected: Vec<u32> = [
        "_NET_SUPPORTED",
        "_NET_SUPPORTING_WM_CHECK",
        "_NET_WM_NAME",
        "_NET_ACTIVE_WINDOW",
        "_NET_CLIENT_LIST",
        "_NET_CLIENT_LIST_STACKING",
        "_NET_WM_STATE",
        "_NET_WM_STATE_FULLSCREEN",
        "_NET_WM_STATE_MAXIMIZED_VERT",
        "_NET_WM_STATE_MAXIMIZED_HORZ",
        "_NET_WM_STATE_ABOVE",
        "_NET_WM_STATE_BELOW",
        "_NET_WM_STATE_HIDDEN",
        "_NET_WM_MOVERESIZE",
        "_NET_CLOSE_WINDOW",
        "_NET_FRAME_EXTENTS",
        "_NET_REQUEST_FRAME_EXTENTS",
    ]
    .iter()
    .map(|n| atom(&host, n))
    .collect();
    assert_eq!(
        words(&host, ROOT_WINDOW, "_NET_SUPPORTED"),
        Some(("ATOM".to_owned(), expected))
    );
    let wm_s0 = host.atom("WM_S0");
    let reply = host.request(1, 23, 0, &wm_s0.to_le_bytes());
    assert_eq!(reply[0], 1, "a reply");
    assert_eq!(
        u32::from_le_bytes(reply[8..12].try_into().unwrap()),
        check.0
    );
}

/// The check window is the server's, never a top-level, even when a client
/// maps it (decision 3 of the review).
#[test]
fn wm_check_window_is_never_a_toplevel() {
    let mut host = RecordingHost::new();
    host.map(1, ROOTLESS_WM_CHECK_WINDOW.0);
    assert_eq!(host.reconcile(), vec![]);
    // Excluded by identity, not only by being InputOnly.
    host.state
        .resources
        .window_mut(ROOTLESS_WM_CHECK_WINDOW)
        .unwrap()
        .class = crate::resources::WindowClass::InputOutput;
    assert_eq!(host.reconcile(), vec![]);
}

/// ICCCM §4.1.3.1: `WM_STATE` is type `WM_STATE`, format 32, `[state, icon]`;
/// Normal = 1 on map. EWMH §3.2 `_NET_CLIENT_LIST`. Mutation: skip WM_STATE.
#[test]
fn map_sets_wm_state_normal_and_client_list() {
    let mut host = RecordingHost::new();
    host.create_window(1, W, ROOT_WINDOW.0, (0, 0), (50, 40), 0, false);
    assert_no_error(&select_input(&mut host, 1, W, PROPERTY_CHANGE), "select");
    host.map_and_reconcile(1, W);
    assert_eq!(
        words(&host, ResourceId(W), "WM_STATE"),
        Some(("WM_STATE".to_owned(), vec![1, 0]))
    );
    assert_eq!(
        words(&host, ROOT_WINDOW, "_NET_CLIENT_LIST"),
        Some(("WINDOW".to_owned(), vec![W]))
    );
    let wm_state = atom(&host, "WM_STATE");
    let out = host.read(1);
    assert!(
        out.chunks(32)
            .any(|p| p[0] & 0x7f == 28
                && u32::from_le_bytes(p[8..12].try_into().unwrap()) == wm_state),
        "PropertyNotify(WM_STATE) to the client: {out:?}"
    );
}

/// `_NET_CLIENT_LIST` keeps map order; `_NET_CLIENT_LIST_STACKING` is
/// bottom → top (EWMH §3.2).
#[test]
fn client_list_stacking_follows_restack() {
    let mut host = RecordingHost::new();
    host.create_window(1, W, ROOT_WINDOW.0, (0, 0), (50, 40), 0, false);
    host.create_window(1, W2, ROOT_WINDOW.0, (0, 0), (50, 40), 0, false);
    host.map_and_reconcile(1, W);
    host.map_and_reconcile(1, W2);
    host.configure(1, W, 0x40, &[0]); // raise W
    host.reconcile();
    assert_eq!(
        words(&host, ROOT_WINDOW, "_NET_CLIENT_LIST").unwrap().1,
        vec![W, W2]
    );
    assert_eq!(
        words(&host, ROOT_WINDOW, "_NET_CLIENT_LIST_STACKING")
            .unwrap()
            .1,
        vec![W2, W]
    );
}

/// ICCCM §4.1.3.1: a withdrawn window gets `WithdrawnState` (0).
#[test]
fn unmap_sets_withdrawn() {
    let mut host = RecordingHost::new();
    host.create_window(1, W, ROOT_WINDOW.0, (0, 0), (50, 40), 0, false);
    host.map_and_reconcile(1, W);
    host.unmap(1, W);
    host.reconcile();
    assert_eq!(
        words(&host, ResourceId(W), "WM_STATE").unwrap().1,
        vec![0, 0]
    );
    assert_eq!(
        words(&host, ROOT_WINDOW, "_NET_CLIENT_LIST").unwrap().1,
        Vec::<u32>::new()
    );
}

/// ICCCM §4.1.10: override-redirect windows are not managed.
#[test]
fn popup_gets_no_wm_state() {
    let mut host = RecordingHost::new();
    host.create_window(1, W, ROOT_WINDOW.0, (0, 0), (50, 40), 0, true);
    host.map_and_reconcile(1, W);
    assert_eq!(words(&host, ResourceId(W), "WM_STATE"), None);
    host.unmap(1, W);
    host.reconcile();
    assert_eq!(words(&host, ResourceId(W), "WM_STATE"), None);
}

/// Spec §6.1: nobody holds SubstructureRedirect on the root, so map and
/// configure apply directly and a root SubstructureNotify listener sees
/// MapNotify/ConfigureNotify, never MapRequest/ConfigureRequest.
#[test]
fn client_map_and_configure_apply_without_request_events() {
    let mut host = RecordingHost::new();
    host.connect(LISTENER, ClientByteOrder::LittleEndian);
    assert_no_error(
        &select_input(&mut host, LISTENER, ROOT_WINDOW.0, SUBSTRUCTURE_NOTIFY),
        "select",
    );
    host.create_window(1, W, ROOT_WINDOW.0, (0, 0), (50, 40), 0, false);
    host.map(1, W);
    host.configure(1, W, 0x0f, &[5, 6, 70, 80]);
    let codes = event_codes(&host.read(LISTENER));
    assert!(codes.contains(&19) && codes.contains(&22), "{codes:?}");
    assert!(!codes.contains(&20) && !codes.contains(&23), "{codes:?}");
    let w = host.state.resources.window(ResourceId(W)).unwrap();
    assert_eq!((w.x, w.y, w.width, w.height), (5, 6, 70, 80));
}

/// Spec §6.5: SubstructureRedirect on the root is BadAccess in rootless mode,
/// with Xorg's partial apply up to CWEventMask (`dix/window.c:1372-1377`).
/// Rooted control: the same request succeeds. Mutation: drop the check.
#[test]
fn substructure_redirect_on_root_is_bad_access() {
    let mut host = RecordingHost::new();
    let mut body = ROOT_WINDOW.0.to_le_bytes().to_vec();
    body.extend_from_slice(&(0x2 | 0x800u32).to_le_bytes()); // CWBackPixel | CWEventMask
    body.extend_from_slice(&0x0012_3456u32.to_le_bytes());
    body.extend_from_slice(&(SUBSTRUCTURE_REDIRECT | SUBSTRUCTURE_NOTIFY).to_le_bytes());
    let out = host.request(1, 2, 0, &body);
    assert_eq!(error_code(&out), BAD_ACCESS);
    assert!(
        !host.state.clients[&1]
            .event_masks
            .contains_key(&ROOT_WINDOW)
    );
    assert_eq!(
        host.state
            .resources
            .window(ROOT_WINDOW)
            .unwrap()
            .background_pixel,
        0x0012_3456,
        "attributes below CWEventMask are applied"
    );
    let out = select_input(&mut host, 1, ROOT_WINDOW.0, SUBSTRUCTURE_NOTIFY);
    assert!(out.is_empty(), "{out:?}");
    assert_eq!(
        host.state.clients[&1].event_masks.get(&ROOT_WINDOW),
        Some(&SUBSTRUCTURE_NOTIFY)
    );

    let mut rooted = RecordingHost::rooted();
    let out = select_input(&mut rooted, 1, ROOT_WINDOW.0, SUBSTRUCTURE_REDIRECT);
    assert!(out.is_empty(), "rooted: {out:?}");
}

/// Each root ClientMessage, from an LE and a BE sender, becomes one
/// `WmRequest` and still reaches another root SubstructureNotify listener.
fn assert_message(ty: &str, l: impl Fn(&mut RecordingHost) -> [u32; 5], expected: &[WmRequest]) {
    for sender in [1, BE] {
        let mut host = RecordingHost::new();
        host.connect(BE, ClientByteOrder::BigEndian);
        host.connect(LISTENER, ClientByteOrder::LittleEndian);
        host.create_window(1, W, ROOT_WINDOW.0, (0, 0), (50, 40), 0, false);
        let id: ToplevelId = host.map_and_reconcile(1, W);
        assert_no_error(
            &select_input(&mut host, LISTENER, ROOT_WINDOW.0, SUBSTRUCTURE_NOTIFY),
            "select",
        );
        let l = l(&mut host);
        root_message(&mut host, sender, W, ty, l);
        let wanted: Vec<RootlessEvent> = expected
            .iter()
            .map(|r| RootlessEvent::WmRequest { id, request: *r })
            .collect();
        assert_eq!(host.reconcile(), wanted, "{ty} from client {sender}");
        assert!(
            event_codes(&host.read(LISTENER)).contains(&33),
            "{ty}: delivered to the root listener"
        );
    }
}

/// EWMH §5.7 `_NET_WM_STATE` add FULLSCREEN. Mutation: decode the template LE.
#[test]
fn net_wm_state_add_fullscreen_maps_to_wm_request() {
    assert_message(
        "_NET_WM_STATE",
        |h| [1, h.atom("_NET_WM_STATE_FULLSCREEN"), 0, 1, 0],
        &[WmRequest::SetFullscreen(true)],
    );
}

/// Toggle of both maximized atoms on a window with neither set.
#[test]
fn net_wm_state_toggle_maximized_maps_to_wm_request() {
    assert_message(
        "_NET_WM_STATE",
        |h| {
            [
                2,
                h.atom("_NET_WM_STATE_MAXIMIZED_VERT"),
                h.atom("_NET_WM_STATE_MAXIMIZED_HORZ"),
                1,
                0,
            ]
        },
        &[WmRequest::SetMaximized(true)],
    );
}

/// Toggle resolves against the current `_NET_WM_STATE`: ABOVE set → off.
#[test]
fn net_wm_state_toggle_above_reads_current_state() {
    for sender in [1, BE] {
        let mut host = RecordingHost::new();
        host.connect(BE, ClientByteOrder::BigEndian);
        host.create_window(1, W, ROOT_WINDOW.0, (0, 0), (50, 40), 0, false);
        let above = host.atom("_NET_WM_STATE_ABOVE");
        host.change_property(1, W, "_NET_WM_STATE", "ATOM", 32, &[above]);
        let id = host.map_and_reconcile(1, W);
        root_message(&mut host, sender, W, "_NET_WM_STATE", [2, above, 0, 1, 0]);
        assert_eq!(
            host.reconcile(),
            vec![RootlessEvent::WmRequest {
                id,
                request: WmRequest::SetLayer(Layer::Normal)
            }]
        );
    }
}

/// ICCCM §4.1.4: `WM_CHANGE_STATE` with IconicState (3).
#[test]
fn wm_change_state_iconic_maps_to_minimize() {
    assert_message(
        "WM_CHANGE_STATE",
        |_| [3, 0, 0, 0, 0],
        &[WmRequest::Minimize],
    );
}

/// EWMH §4.3: direction 8 = move, l3 = button.
#[test]
fn net_wm_moveresize_move_maps_to_begin_move() {
    assert_message(
        "_NET_WM_MOVERESIZE",
        |_| [100, 100, 8, 1, 1],
        &[WmRequest::BeginMove { button: 1 }],
    );
}

/// EWMH §4.3: direction 4 = bottom-right.
#[test]
fn net_wm_moveresize_edge_maps_to_begin_resize() {
    assert_message(
        "_NET_WM_MOVERESIZE",
        |_| [100, 100, 4, 1, 1],
        &[WmRequest::BeginResize {
            edge: ResizeEdge::BottomRight,
            button: 1,
        }],
    );
}

/// EWMH §3.5, §4.1.
#[test]
fn net_active_and_close_window_map_to_wm_requests() {
    assert_message(
        "_NET_ACTIVE_WINDOW",
        |_| [1, 0, 0, 0, 0],
        &[WmRequest::Activate],
    );
    assert_message(
        "_NET_CLOSE_WINDOW",
        |_| [0, 1, 0, 0, 0],
        &[WmRequest::Close],
    );
}

/// Requests for windows that are not mapped top-levels are dropped.
#[test]
fn wm_request_for_unmapped_window_is_dropped() {
    let mut host = RecordingHost::new();
    host.create_window(1, W, ROOT_WINDOW.0, (0, 0), (50, 40), 0, false);
    root_message(&mut host, 1, W, "_NET_CLOSE_WINDOW", [0, 1, 0, 0, 0]);
    assert_eq!(host.reconcile(), vec![]);
}

// ---- Task 8: transient placement (spec §6.1, Global only) ----

const PARENT: u32 = 0x0020_0010;
const DIALOG: u32 = 0x0020_0011;

/// Parent (100,100) 400×300; a 200×100 transient dialog, no hints.
fn transient_setup(host: &mut RecordingHost, flags: Option<u32>) {
    host.create_window(1, PARENT, ROOT_WINDOW.0, (100, 100), (400, 300), 0, false);
    host.map_and_reconcile(1, PARENT);
    host.create_window(1, DIALOG, ROOT_WINDOW.0, (0, 0), (200, 100), 0, false);
    host.change_property(1, DIALOG, "WM_TRANSIENT_FOR", "WINDOW", 32, &[PARENT]);
    if let Some(flags) = flags {
        let mut hints = [0u32; 18];
        hints[0] = flags;
        host.change_property(1, DIALOG, "WM_NORMAL_HINTS", "WM_SIZE_HINTS", 32, &hints);
    }
    host.map(1, DIALOG);
}

fn mapped_origin(events: &[RootlessEvent]) -> (i32, i32) {
    match events {
        [RootlessEvent::Mapped { geometry, .. }] => (geometry.x, geometry.y),
        other => panic!("expected one Mapped: {other:?}"),
    }
}

/// Spec §6.1: a transient without USPosition/PPosition (`Xutil.h:102-104`) is
/// centred on its parent's outer rect before `Mapped`: (100 + (400−200)/2,
/// 100 + (300−100)/2) = (200, 200). Mutation: place after `Mapped`.
#[test]
fn transient_without_position_is_centred_on_parent() {
    let mut host = RecordingHost::global();
    transient_setup(&mut host, None);
    assert_eq!(mapped_origin(&host.reconcile()), (200, 200));
    let w = host.state.resources.window(ResourceId(DIALOG)).unwrap();
    assert_eq!((w.x, w.y), (200, 200));
}

/// PPosition (bit 2): the client chose its position.
#[test]
fn transient_with_pposition_is_left_alone() {
    let mut host = RecordingHost::global();
    transient_setup(&mut host, Some(1 << 2));
    assert_eq!(mapped_origin(&host.reconcile()), (0, 0));
}

/// Spec §5: an Opaque host keeps X positions as clients set them.
#[test]
fn opaque_policy_never_places() {
    let mut host = RecordingHost::new();
    transient_setup(&mut host, None);
    assert_eq!(mapped_origin(&host.reconcile()), (0, 0));
}

// ---- _NET_FRAME_EXTENTS (EWMH §5.11, _NET_REQUEST_FRAME_EXTENTS §4.3) ----

fn frame_extents(host: &RecordingHost, window: u32) -> Option<(String, Vec<u32>)> {
    words(host, ResourceId(window), "_NET_FRAME_EXTENTS")
}

fn property_notifies(out: &[u8], atom: u32) -> usize {
    out.chunks(32)
        .filter(|p| p[0] & 0x7f == 28 && u32::from_le_bytes(p[8..12].try_into().unwrap()) == atom)
        .count()
}

/// EWMH `_NET_REQUEST_FRAME_EXTENTS`: a client may ask before mapping and
/// waits for the property; native decorations sit outside the X window, so
/// the extents are zero. Mutation: answer only for mapped windows.
#[test]
fn request_frame_extents_before_map_is_answered_at_once() {
    let mut host = RecordingHost::new();
    host.connect(BE, ClientByteOrder::BigEndian);
    host.create_window(1, W, ROOT_WINDOW.0, (0, 0), (50, 40), 0, false);
    assert_no_error(&select_input(&mut host, 1, W, PROPERTY_CHANGE), "select");
    // Sent by another (big-endian) client so client 1's output stays unread.
    root_message(&mut host, BE, W, "_NET_REQUEST_FRAME_EXTENTS", [0; 5]);
    assert_eq!(
        frame_extents(&host, W),
        Some(("CARDINAL".to_owned(), vec![0, 0, 0, 0]))
    );
    let atom = atom(&host, "_NET_FRAME_EXTENTS");
    assert_eq!(property_notifies(&host.read(1), atom), 1);
    let w = host.state.resources.window(ResourceId(W)).unwrap();
    assert_eq!(w.map_state, crate::resources::MapState::Unmapped);
}

/// A managed window that never asked gets the extents at map.
#[test]
fn mapped_window_gets_zero_frame_extents() {
    let mut host = RecordingHost::new();
    host.create_window(1, W, ROOT_WINDOW.0, (0, 0), (50, 40), 0, false);
    host.map_and_reconcile(1, W);
    assert_eq!(
        frame_extents(&host, W),
        Some(("CARDINAL".to_owned(), vec![0, 0, 0, 0]))
    );
}

/// The request is advertised (GTK checks `_NET_SUPPORTED` before asking).
#[test]
fn request_frame_extents_is_supported() {
    let host = RecordingHost::new();
    let supported = words(&host, ROOT_WINDOW, "_NET_SUPPORTED").unwrap().1;
    assert!(supported.contains(&atom(&host, "_NET_REQUEST_FRAME_EXTENTS")));
}

/// Rooted control: with rootless off nothing answers the request.
#[test]
fn rooted_request_frame_extents_is_unanswered() {
    let mut host = RecordingHost::rooted();
    host.create_window(1, W, ROOT_WINDOW.0, (0, 0), (50, 40), 0, false);
    root_message(&mut host, 1, W, "_NET_REQUEST_FRAME_EXTENTS", [0; 5]);
    assert_eq!(frame_extents(&host, W), None);
}

/// The core computes a toggle from `_NET_WM_STATE` and never writes it for a
/// client's request, so the host's `StateChanged` must be what the next
/// toggle reads (plan RL2 Task 6). Mutation: drop the `set_words` in
/// `StateChanged`.
#[test]
fn toggle_after_state_changed_uses_the_new_state() {
    let mut host = RecordingHost::new();
    host.create_window(1, W, ROOT_WINDOW.0, (0, 0), (50, 40), 0, false);
    let id = host.map_and_reconcile(1, W);
    let vert = host.atom("_NET_WM_STATE_MAXIMIZED_VERT");
    let horz = host.atom("_NET_WM_STATE_MAXIMIZED_HORZ");
    let above = host.atom("_NET_WM_STATE_ABOVE");
    let below = host.atom("_NET_WM_STATE_BELOW");
    let toggle = |host: &mut RecordingHost, a: u32, b: u32| {
        root_message(host, 1, W, "_NET_WM_STATE", [2, a, b, 1, 0]);
        host.reconcile()
            .into_iter()
            .filter_map(|e| match e {
                RootlessEvent::WmRequest { id: got, request } if got == id => Some(request),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let mut state = NetWmState::default();
    let report = |host: &mut RecordingHost, state: NetWmState| {
        host.host(HostRequest::StateChanged { id, state });
        host.reconcile();
    };

    assert_eq!(
        toggle(&mut host, vert, horz),
        [WmRequest::SetMaximized(true)]
    );
    state.maximized_vert = true;
    state.maximized_horz = true;
    report(&mut host, state);
    assert_eq!(
        toggle(&mut host, vert, horz),
        [WmRequest::SetMaximized(false)]
    );

    assert_eq!(
        toggle(&mut host, above, 0),
        [WmRequest::SetLayer(Layer::Above)]
    );
    state.above = true;
    report(&mut host, state);
    assert_eq!(
        toggle(&mut host, above, 0),
        [WmRequest::SetLayer(Layer::Normal)]
    );

    state.above = false;
    report(&mut host, state);
    assert_eq!(
        toggle(&mut host, below, 0),
        [WmRequest::SetLayer(Layer::Below)]
    );
    state.below = true;
    report(&mut host, state);
    assert_eq!(
        toggle(&mut host, below, 0),
        [WmRequest::SetLayer(Layer::Normal)]
    );
}
