use yserver_protocol::x11::{ResourceId, composite as x11composite, error::BAD_ACCESS};

use super::{RecordingHost, error_code};
use crate::{
    backend::recording::RecordedCall,
    resources::{COMPOSITE_OVERLAY_WINDOW, ROOT_WINDOW, SERVER_OWNER},
    rootless::{
        Geometry, GeometryFields, Rect, RootlessEvent, ToplevelId, ToplevelKind, WindowInfo,
    },
    server::{CompositeRedirectMode, RedirectRecord},
};

const MANUAL_BY_SERVER: RedirectRecord = RedirectRecord {
    mode: CompositeRedirectMode::Manual,
    owner: SERVER_OWNER,
};

fn redirect_subwindows_body(window: u32, update: u8) -> Vec<u8> {
    let mut body = window.to_le_bytes().to_vec();
    body.extend_from_slice(&[update, 0, 0, 0]);
    body
}

fn allocated_backings(host: &RecordingHost) -> Vec<u32> {
    host.backend
        .calls()
        .iter()
        .filter_map(|c| match c {
            RecordedCall::AllocateRedirectedBacking { host_window, .. } => Some(*host_window),
            _ => None,
        })
        .collect()
}

fn host_xid(host: &RecordingHost, window: u32) -> u32 {
    host.state
        .resources
        .window(ResourceId(window))
        .and_then(|w| w.host_xid)
        .expect("realized window")
        .as_raw()
}

/// Spec §4.1 startup: root children that exist before `enable` are
/// redirected Manual by the server, and mapped ones get a backing. The COW
/// gets no record (Xorg `compRedirectSubwindows` skips it).
/// Mutation: redirect only at map/create (skip existing children).
#[test]
fn enable_redirects_existing_root_children_manual_as_server() {
    let mut host = RecordingHost::rooted();
    host.request(
        1,
        144,
        x11composite::GET_OVERLAY_WINDOW,
        &ROOT_WINDOW.0.to_le_bytes(),
    );
    host.create_window(1, 0x0020_0001, ROOT_WINDOW.0, (0, 0), (50, 40), 0, false);
    host.create_window(1, 0x0020_0002, ROOT_WINDOW.0, (60, 0), (50, 40), 0, false);
    host.map(1, 0x0020_0001);
    host.map(1, 0x0020_0002);
    assert!(allocated_backings(&host).is_empty(), "rooted: no backing");

    host.enable();

    for w in [0x0020_0001, 0x0020_0002] {
        assert_eq!(
            host.state.composite_redirects.window_records(ResourceId(w)),
            &[MANUAL_BY_SERVER]
        );
    }
    assert!(
        host.state
            .composite_redirects
            .window_records(COMPOSITE_OVERLAY_WINDOW)
            .is_empty()
    );
    let mut allocated = allocated_backings(&host);
    allocated.sort_unstable();
    let mut expected = vec![host_xid(&host, 0x0020_0001), host_xid(&host, 0x0020_0002)];
    expected.sort_unstable();
    assert_eq!(allocated, expected);
}

/// A root child created after `enable` is redirected at creation, before
/// it is mapped (Xorg `compCreateWindow`, `composite/compwindow.c:579-589`).
/// Mutation: redirect at map.
#[test]
fn root_child_created_after_enable_is_redirected_at_creation() {
    let mut host = RecordingHost::new();
    host.create_window(1, 0x0020_0001, ROOT_WINDOW.0, (0, 0), (50, 40), 0, false);
    assert_eq!(
        host.state
            .composite_redirects
            .window_records(ResourceId(0x0020_0001)),
        &[MANUAL_BY_SERVER]
    );
}

/// Spec §6.5: a client compositor's `RedirectSubwindows(root, Manual)` gets
/// BadAccess (`compalloc.c:336-339`) and changes nothing; Automatic works.
#[test]
fn client_compositor_manual_on_root_gets_bad_access() {
    let mut host = RecordingHost::new();
    host.create_window(1, 0x0020_0001, ROOT_WINDOW.0, (0, 0), (50, 40), 0, false);

    let out = host.request(
        1,
        144,
        x11composite::REDIRECT_SUBWINDOWS,
        &redirect_subwindows_body(ROOT_WINDOW.0, 1),
    );
    assert_eq!(error_code(&out), BAD_ACCESS);
    assert_eq!(
        host.state
            .composite_redirects
            .subwindows_records(ROOT_WINDOW),
        &[MANUAL_BY_SERVER]
    );
    assert_eq!(
        host.state
            .composite_redirects
            .window_records(ResourceId(0x0020_0001)),
        &[MANUAL_BY_SERVER]
    );

    let out = host.request(
        1,
        144,
        x11composite::REDIRECT_SUBWINDOWS,
        &redirect_subwindows_body(ROOT_WINDOW.0, 0),
    );
    assert!(out.is_empty(), "Automatic succeeds: {out:?}");
}

/// The server's records survive any client's disconnect.
/// Mutation: `forget_client(SERVER_OWNER)` on any disconnect.
#[test]
fn client_disconnect_never_drops_server_records() {
    let mut host = RecordingHost::new();
    host.connect(2, yserver_protocol::x11::ClientByteOrder::LittleEndian);
    host.create_window(1, 0x0020_0001, ROOT_WINDOW.0, (0, 0), (50, 40), 0, false);
    host.create_window(2, 0x0040_0001, ROOT_WINDOW.0, (60, 0), (50, 40), 0, false);
    host.map(1, 0x0020_0001);
    host.map(2, 0x0040_0001);

    host.disconnect(2);

    assert!(
        host.state
            .resources
            .window(ResourceId(0x0040_0001))
            .is_none()
    );
    assert_eq!(
        host.state
            .composite_redirects
            .window_records(ResourceId(0x0020_0001)),
        &[MANUAL_BY_SERVER]
    );
    assert_eq!(
        host.state
            .composite_redirects
            .subwindows_records(ROOT_WINDOW),
        &[MANUAL_BY_SERVER]
    );
}

/// Plan Task 2: no backings means no rootless; a client's Manual on the root
/// is never overwritten.
#[test]
fn enable_refuses_without_redirect_activation_or_over_a_client_manual() {
    use crate::{
        backend::recording::RecordingBackend,
        rootless::{self, CoordinatePolicy, RootlessError},
        server::ServerState,
    };
    let mut state = ServerState::new();
    let mut backend = RecordingBackend::new();
    assert_eq!(
        rootless::enable(&mut state, &mut backend, CoordinatePolicy::Opaque),
        Err(RootlessError::NoRedirectActivation)
    );
    assert!(state.rootless.is_none());

    let mut host = RecordingHost::rooted();
    let out = host.request(
        1,
        144,
        x11composite::REDIRECT_SUBWINDOWS,
        &redirect_subwindows_body(ROOT_WINDOW.0, 1),
    );
    assert!(out.is_empty(), "{out:?}");
    assert_eq!(
        rootless::enable(&mut host.state, &mut host.backend, CoordinatePolicy::Opaque),
        Err(RootlessError::ManualTaken)
    );
    assert!(host.state.rootless.is_none());
    assert_eq!(
        host.state
            .composite_redirects
            .subwindows_records(ROOT_WINDOW),
        &[RedirectRecord {
            mode: CompositeRedirectMode::Manual,
            owner: yserver_protocol::x11::ClientId(1),
        }]
    );
}

// ---- Task 3: reconcile → top-level events (spec §4.1, §4.2) ----

const W: u32 = 0x0020_0001;
const W2: u32 = 0x0020_0002;
const W3: u32 = 0x0020_0003;

fn root_child(host: &mut RecordingHost, wid: u32, at: (i16, i16), size: (u16, u16)) {
    host.create_window(1, wid, ROOT_WINDOW.0, at, size, 0, false);
}

fn popup_parent(events: &[RootlessEvent]) -> Option<ToplevelId> {
    match events {
        [
            RootlessEvent::Mapped {
                kind: ToplevelKind::Popup { parent },
                ..
            },
        ] => *parent,
        other => panic!("expected one popup Mapped: {other:?}"),
    }
}

/// Spec §4.1 "why diff": create + map + configure in one iteration is one
/// `Mapped` with the final geometry. Mutation: emit per request.
#[test]
fn create_map_configure_in_one_iteration_is_one_mapped_with_final_geometry() {
    let mut host = RecordingHost::new();
    root_child(&mut host, W, (10, 10), (100, 50));
    host.map(1, W);
    // x y width height border-width
    host.configure(1, W, 0x1f, &[20, 30, 120, 60, 2]);
    let events = host.reconcile();
    let [
        RootlessEvent::Mapped {
            window,
            kind,
            geometry,
            depth,
            info,
            above,
            ..
        },
    ] = events.as_slice()
    else {
        panic!("expected one Mapped: {events:?}");
    };
    assert_eq!(*window, ResourceId(W));
    assert_eq!(*kind, ToplevelKind::Managed);
    assert_eq!(
        *geometry,
        Geometry {
            x: 20,
            y: 30,
            width: 120,
            height: 60,
            border: 2
        }
    );
    assert_eq!(*depth, 24);
    assert_eq!(*info, WindowInfo::default());
    assert_eq!(*above, None);
}

/// Spec §6.3: `Unmapped` means "destroy the native window"; an unmap is not a
/// destroy. Mutation: emit `Destroyed` on unmap.
#[test]
fn unmap_emits_only_unmapped() {
    let mut host = RecordingHost::new();
    root_child(&mut host, W, (0, 0), (50, 40));
    let id = host.map_and_reconcile(1, W);
    host.unmap(1, W);
    assert_eq!(host.reconcile(), vec![RootlessEvent::Unmapped { id }]);
}

/// Spec §4.2: an id names one mapped lifetime. Mutation: keep the id across
/// an unmap.
#[test]
fn remap_after_unmap_gets_a_fresh_id() {
    let mut host = RecordingHost::new();
    root_child(&mut host, W, (0, 0), (50, 40));
    let id = host.map_and_reconcile(1, W);
    host.unmap(1, W);
    host.reconcile();
    let id2 = host.map_and_reconcile(1, W);
    assert_ne!(id2, id);
}

#[test]
fn destroy_mapped_emits_unmapped_then_destroyed() {
    let mut host = RecordingHost::new();
    root_child(&mut host, W, (0, 0), (50, 40));
    let id = host.map_and_reconcile(1, W);
    host.destroy(1, W);
    assert_eq!(
        host.reconcile(),
        vec![
            RootlessEvent::Unmapped { id },
            RootlessEvent::Destroyed { id }
        ]
    );
}

/// The unmap retired the id; there is nothing left to destroy.
#[test]
fn destroy_after_unmap_emits_nothing() {
    let mut host = RecordingHost::new();
    root_child(&mut host, W, (0, 0), (50, 40));
    host.map_and_reconcile(1, W);
    host.unmap(1, W);
    host.reconcile();
    host.destroy(1, W);
    assert_eq!(host.reconcile(), vec![]);
}

#[test]
fn client_disconnect_emits_unmapped_then_destroyed() {
    let mut host = RecordingHost::new();
    host.connect(2, yserver_protocol::x11::ClientByteOrder::LittleEndian);
    host.create_window(2, 0x0040_0001, ROOT_WINDOW.0, (0, 0), (50, 40), 0, false);
    let id = host.map_and_reconcile(2, 0x0040_0001);
    host.disconnect(2);
    assert_eq!(
        host.reconcile(),
        vec![
            RootlessEvent::Unmapped { id },
            RootlessEvent::Destroyed { id }
        ]
    );
}

/// A top-level reparented under a non-root window stops being one; back
/// under the root it is a new mapped lifetime (spec §10).
#[test]
fn reparent_under_non_root_then_back() {
    let mut host = RecordingHost::new();
    root_child(&mut host, W, (0, 0), (200, 200));
    root_child(&mut host, W2, (10, 10), (50, 40));
    host.map_and_reconcile(1, W);
    let id = host.map_and_reconcile(1, W2);

    host.reparent(1, W2, W, (5, 5));
    assert_eq!(
        host.reconcile(),
        vec![
            RootlessEvent::Unmapped { id },
            RootlessEvent::Destroyed { id }
        ]
    );

    host.reparent(1, W2, ROOT_WINDOW.0, (30, 30));
    let events = host.reconcile();
    let [
        RootlessEvent::Mapped {
            id: id2, window, ..
        },
    ] = events.as_slice()
    else {
        panic!("expected one Mapped: {events:?}");
    };
    assert_eq!(*window, ResourceId(W2));
    assert_ne!(*id2, id);
}

/// Spec §4.2: ids never come from the XID. Mutation: derive the id from the
/// XID.
#[test]
fn xid_reuse_gets_a_new_toplevel_id() {
    let mut host = RecordingHost::new();
    root_child(&mut host, W, (0, 0), (50, 40));
    let id = host.map_and_reconcile(1, W);
    host.destroy(1, W);
    host.reconcile();
    root_child(&mut host, W, (0, 0), (50, 40));
    let id2 = host.map_and_reconcile(1, W);
    assert_ne!(id2, id);
}

/// Spec §10 popup parent order, the rungs RL0 has so far: containing
/// top-level, topmost Managed, none. Mutation: always pick the topmost.
#[test]
fn override_redirect_is_popup_with_parent_order() {
    let mut host = RecordingHost::new();
    host.create_window(1, 0x0020_0010, ROOT_WINDOW.0, (5, 5), (10, 10), 0, true);
    host.map(1, 0x0020_0010);
    assert_eq!(popup_parent(&host.reconcile()), None, "no Managed: none");

    root_child(&mut host, W, (0, 0), (50, 50));
    root_child(&mut host, W2, (100, 0), (50, 50));
    let a = host.map_and_reconcile(1, W);
    let b = host.map_and_reconcile(1, W2);

    host.create_window(1, 0x0020_0011, ROOT_WINDOW.0, (10, 10), (10, 10), 0, true);
    host.map(1, 0x0020_0011);
    assert_eq!(popup_parent(&host.reconcile()), Some(a), "containing");

    host.create_window(1, 0x0020_0012, ROOT_WINDOW.0, (300, 300), (10, 10), 0, true);
    host.map(1, 0x0020_0012);
    assert_eq!(popup_parent(&host.reconcile()), Some(b), "topmost");
}

/// A client raise (spec §4.2 `Restacked`, `from_client: true`).
#[test]
fn restack_raise_emits_restacked_from_client() {
    let mut host = RecordingHost::new();
    root_child(&mut host, W, (0, 0), (50, 50));
    root_child(&mut host, W2, (10, 10), (50, 50));
    let lower = host.map_and_reconcile(1, W);
    let upper = host.map_and_reconcile(1, W2);
    // stack-mode Above, no sibling
    host.configure(1, W, 0x40, &[0]);
    assert_eq!(
        host.reconcile(),
        vec![RootlessEvent::Restacked {
            id: lower,
            above: Some(upper),
            acked: None,
            from_client: true,
        }]
    );
}

/// A client move is a `Configured` carrying the fields it changed.
#[test]
fn client_move_emits_configured_with_its_fields() {
    let mut host = RecordingHost::new();
    root_child(&mut host, W, (0, 0), (50, 40));
    let id = host.map_and_reconcile(1, W);
    host.configure(1, W, 0x3, &[7, 9]);
    assert_eq!(
        host.reconcile(),
        vec![RootlessEvent::Configured {
            id,
            geometry: Geometry {
                x: 7,
                y: 9,
                width: 50,
                height: 40,
                border: 0
            },
            acked: None,
            client_fields: GeometryFields::X | GeometryFields::Y,
        }]
    );
}

/// Spec §4.0: shapes reach the host in backing pixels, `+bw`. Source: shape
/// is relative to the content origin (`dix/window.c:1735-1742`), the backing
/// origin is the outer corner (`compalloc.c:600-606`). Mutation: drop `+bw`.
#[test]
fn shape_reaches_host_in_backing_pixels() {
    let mut host = RecordingHost::new();
    host.create_window(1, W, ROOT_WINDOW.0, (0, 0), (50, 40), 5, false);
    let id = host.map_and_reconcile(1, W);
    host.shape_bounding(1, W, &[(0, 0, 10, 10)]);
    assert_eq!(
        host.reconcile(),
        vec![RootlessEvent::Reshaped {
            id,
            bounding: Some(vec![Rect {
                x: 5,
                y: 5,
                width: 10,
                height: 10
            }]),
            input: None,
        }]
    );
}

/// Spec §4.2.1 rule 7.
#[test]
fn unchanged_iteration_emits_nothing() {
    let mut host = RecordingHost::new();
    root_child(&mut host, W, (0, 0), (50, 40));
    root_child(&mut host, W3, (60, 0), (50, 40));
    host.map_and_reconcile(1, W);
    host.map_and_reconcile(1, W3);
    assert_eq!(host.reconcile(), vec![]);
}
