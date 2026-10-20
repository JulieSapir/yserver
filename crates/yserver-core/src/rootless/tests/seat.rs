//! Plan RL0 Task 7: local-coordinate input, pointer scope, activation
//! (spec §4.4, §6.2).

use yserver_protocol::x11::ResourceId;

use super::RecordingHost;
use crate::{
    backend::recording::RecordedCall,
    core_loop::{HostInputEvent, InputOrigin, handle_host_input},
    resources::ROOT_WINDOW,
    rootless::{HostRequest, RootlessEvent, RootlessInput, ToplevelId, ToplevelKind},
};

const T: u32 = 0x0020_0001;
const A: u32 = 0x0020_0002;
const B: u32 = 0x0020_0003;
const ORIGIN: InputOrigin = InputOrigin::NestedHost;

fn input(host: &mut RecordingHost, input: RootlessInput) {
    handle_host_input(
        &mut host.state,
        &mut host.backend,
        HostInputEvent::Rootless(input),
    );
}

fn motion_local(toplevel: ToplevelId, x: f64, y: f64) -> RootlessInput {
    RootlessInput::PointerMotionLocal {
        origin: ORIGIN,
        toplevel,
        local: (x, y),
        time: 1,
    }
}

fn pointer_calls(host: &RecordingHost) -> Vec<RecordedCall> {
    host.backend
        .calls()
        .into_iter()
        .filter(|c| matches!(c, RecordedCall::HostPointer { .. }))
        .collect()
}

fn last_motion(host: &RecordingHost) -> Option<(i32, i32)> {
    pointer_calls(host).iter().rev().find_map(|c| match c {
        RecordedCall::HostPointer {
            x, y, button: 0, ..
        } => Some((*x, *y)),
        _ => None,
    })
}

fn bordered(host: &mut RecordingHost, override_redirect: bool) -> ToplevelId {
    host.create_window(
        1,
        T,
        ROOT_WINDOW.0,
        (40, 30),
        (100, 50),
        5,
        override_redirect,
    );
    host.map_and_reconcile(1, T)
}

/// Spec §4.4: local (0,0) is the outer corner, so it lands on the border,
/// inside the window's hit box (`mi/miwindow.c:756-760`), at event
/// coordinates relative to the content origin (`dix/window.c:888`).
/// Mutation: no `−bw`.
#[test]
fn local_origin_on_bordered_toplevel_hits_the_border() {
    let mut host = RecordingHost::new();
    let id = bordered(&mut host, false);
    input(&mut host, motion_local(id, 0.0, 0.0));
    assert_eq!(last_motion(&host), Some((40, 30)));
    assert_eq!(
        host.state.root_pointer_target_at(40, 30),
        Some((ResourceId(T), -5, -5))
    );
}

#[test]
fn local_bw_bw_is_content_origin() {
    let mut host = RecordingHost::new();
    let id = bordered(&mut host, false);
    input(&mut host, motion_local(id, 5.0, 5.0));
    assert_eq!(last_motion(&host), Some((45, 35)));
    assert_eq!(
        host.state.root_pointer_target_at(45, 35),
        Some((ResourceId(T), 0, 0))
    );
}

/// Spec §10: popups map the same way (SetZero's likely border offset).
#[test]
fn bordered_popup_maps_the_same_way() {
    let mut host = RecordingHost::new();
    let id = bordered(&mut host, true);
    input(&mut host, motion_local(id, 0.0, 0.0));
    assert_eq!(last_motion(&host), Some((40, 30)));
    assert_eq!(
        host.state.root_pointer_target_at(40, 30),
        Some((ResourceId(T), -5, -5))
    );
}

fn overlapping(host: &mut RecordingHost) -> (ToplevelId, ToplevelId) {
    // Created later, A is above B in X.
    host.create_window(1, B, ROOT_WINDOW.0, (50, 50), (100, 100), 0, false);
    host.create_window(1, A, ROOT_WINDOW.0, (0, 0), (100, 100), 0, false);
    let b = host.map_and_reconcile(1, B);
    let a = host.map_and_reconcile(1, A);
    (a, b)
}

/// Spec §4.4: the native system says which window is under the pointer.
/// Mutation: unscoped hit test.
#[test]
fn scope_follows_native_window_not_x_stacking() {
    let mut host = RecordingHost::new();
    let (_, b) = overlapping(&mut host);
    input(
        &mut host,
        RootlessInput::PointerEnter {
            origin: ORIGIN,
            toplevel: b,
            local: (10.0, 10.0),
            time: 1,
        },
    );
    assert_eq!(last_motion(&host), Some((60, 60)));
    assert_eq!(
        host.state.root_pointer_target_at(60, 60),
        Some((ResourceId(B), 10, 10))
    );
}

/// Rooted control: without `enable` the X-topmost window is hit.
#[test]
fn rooted_hit_test_unchanged() {
    let mut host = RecordingHost::rooted();
    host.create_window(1, B, ROOT_WINDOW.0, (50, 50), (100, 100), 0, false);
    host.create_window(1, A, ROOT_WINDOW.0, (0, 0), (100, 100), 0, false);
    host.map(1, B);
    host.map(1, A);
    assert_eq!(
        host.state.root_pointer_target_at(60, 60),
        Some((ResourceId(A), 60, 60))
    );
}

/// Spec §4.4: leave releases held buttons, then moves in place so the hit
/// test returns the root.
#[test]
fn leave_releases_buttons_then_moves_to_root() {
    let mut host = RecordingHost::new();
    let id = bordered(&mut host, false);
    input(&mut host, motion_local(id, 10.0, 10.0));
    host.state.pointer_root = (50, 40);
    host.state.buttons_down = 1;
    let before = pointer_calls(&host).len();
    input(
        &mut host,
        RootlessInput::PointerLeave {
            origin: ORIGIN,
            time: 2,
        },
    );
    assert_eq!(
        pointer_calls(&host)[before..],
        [
            RecordedCall::HostPointer {
                x: 0,
                y: 0,
                button: 0x110,
                pressed: false
            },
            RecordedCall::HostPointer {
                x: 50,
                y: 40,
                button: 0,
                pressed: false
            },
        ]
    );
    assert_eq!(
        host.state.root_pointer_target_at(50, 40),
        Some((ROOT_WINDOW, 50, 40))
    );
}

/// Spec §4.2: input naming an earlier mapped lifetime is dropped.
/// Mutation: resolve the id by XID.
#[test]
fn stale_id_input_is_dropped() {
    let mut host = RecordingHost::new();
    host.create_window(1, T, ROOT_WINDOW.0, (40, 30), (100, 50), 0, false);
    let id1 = host.map_and_reconcile(1, T);
    host.unmap(1, T);
    host.reconcile();
    let id2 = host.map_and_reconcile(1, T);
    let scope = host.state.rootless.as_ref().unwrap().pointer_scope();
    input(&mut host, motion_local(id1, 1.0, 1.0));
    assert!(pointer_calls(&host).is_empty());
    assert_eq!(host.state.rootless.as_ref().unwrap().pointer_scope(), scope);
    host.host(HostRequest::Activated { id: id1 });
    assert_eq!(
        host.state.core_focus.raw, 1,
        "focus unchanged (PointerRoot)"
    );

    input(&mut host, motion_local(id2, 1.0, 1.0));
    assert_eq!(last_motion(&host), Some((41, 31)));
    host.host(HostRequest::Activated { id: id2 });
    assert_eq!(host.state.core_focus.raw, T);
}

fn hints_and_protocols(host: &mut RecordingHost, input_hint: Option<u32>, take_focus: bool) {
    if let Some(input) = input_hint {
        host.change_property(
            1,
            T,
            "WM_HINTS",
            "WM_HINTS",
            32,
            &[1, input, 0, 0, 0, 0, 0, 0, 0],
        );
    }
    if take_focus {
        let atom = host.atom("WM_TAKE_FOCUS");
        host.change_property(1, T, "WM_PROTOCOLS", "ATOM", 32, &[atom]);
    }
}

fn active_window(host: &RecordingHost) -> Option<Vec<u32>> {
    let atom = host.state.atoms.id_for("_NET_ACTIVE_WINDOW")?;
    let v = host.state.resources.window_property(ROOT_WINDOW, atom)?;
    Some(
        v.data
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
    )
}

/// ICCCM §4.1.7 Globally Active (input False + WM_TAKE_FOCUS): no
/// SetInputFocus, one `[WM_TAKE_FOCUS, t]` with t ≠ CurrentTime (§4.2.8).
#[test]
fn activated_input_false_take_focus_sends_only_take_focus() {
    let mut host = RecordingHost::new();
    host.create_window(1, T, ROOT_WINDOW.0, (40, 30), (100, 50), 0, false);
    hints_and_protocols(&mut host, Some(0), true);
    let id = host.map_and_reconcile(1, T);
    host.read(1);
    let focus = host.state.core_focus.raw;
    host.host(HostRequest::Activated { id });
    assert_eq!(host.state.core_focus.raw, focus);
    let out = host.read(1);
    let messages: Vec<&[u8]> = out.chunks(32).filter(|p| p[0] == 33 | 0x80).collect();
    assert_eq!(messages.len(), 1, "{out:?}");
    let word = |at: usize| u32::from_le_bytes(messages[0][at..at + 4].try_into().unwrap());
    assert_eq!(word(8), host.atom("WM_PROTOCOLS"));
    assert_eq!(word(12), host.atom("WM_TAKE_FOCUS"));
    assert_ne!(word(16), 0);
    assert_eq!(active_window(&host), Some(vec![T]));
}

/// ICCCM §4.1.7 Passive: SetInputFocus; EWMH §3.5 `_NET_ACTIVE_WINDOW`.
#[test]
fn activated_passive_sets_focus_and_active_window() {
    let mut host = RecordingHost::new();
    host.create_window(1, T, ROOT_WINDOW.0, (40, 30), (100, 50), 0, false);
    hints_and_protocols(&mut host, Some(1), false);
    let id = host.map_and_reconcile(1, T);
    host.host(HostRequest::Activated { id });
    assert_eq!(host.state.core_focus.raw, T);
    assert_eq!(active_window(&host), Some(vec![T]));
}

/// Spec §6.2: popups are never activated.
#[test]
fn activated_popup_is_dropped() {
    let mut host = RecordingHost::new();
    let id = bordered(&mut host, true);
    let focus = host.state.core_focus.raw;
    host.host(HostRequest::Activated { id });
    assert_eq!(host.state.core_focus.raw, focus);
    assert_eq!(active_window(&host), None);
}

/// Spec §4.4: deactivation releases pressed keys (XWin
/// `winKeybdReleaseKeys`), focuses None, clears `_NET_ACTIVE_WINDOW`.
#[test]
fn deactivated_releases_keys_and_clears_focus() {
    let mut host = RecordingHost::new();
    host.create_window(1, T, ROOT_WINDOW.0, (40, 30), (100, 50), 0, false);
    let id = host.map_and_reconcile(1, T);
    host.host(HostRequest::Activated { id });
    host.state.keys_down[38 / 8] |= 1 << (38 % 8);
    host.host(HostRequest::Deactivated);
    assert!(host.backend.calls().contains(&RecordedCall::HostKey {
        keycode: 38,
        pressed: false,
        repeat: false
    }));
    assert_eq!(host.state.core_focus.raw, 0);
    assert_eq!(active_window(&host), Some(vec![0]));
}

/// Spec §10 popup parent order, rung 2: the pointer-scope top-level beats
/// containment and topmost. Mutation: drop the rung.
#[test]
fn popup_parent_prefers_pointer_scope() {
    let mut host = RecordingHost::new();
    host.create_window(1, A, ROOT_WINDOW.0, (0, 0), (100, 100), 0, false);
    host.create_window(1, B, ROOT_WINDOW.0, (200, 0), (100, 100), 0, false);
    host.map_and_reconcile(1, A);
    let b = host.map_and_reconcile(1, B);
    input(&mut host, motion_local(b, 1.0, 1.0));
    host.create_window(1, T, ROOT_WINDOW.0, (10, 10), (20, 20), 0, true);
    host.map(1, T);
    let events = host.reconcile();
    assert!(
        matches!(events.as_slice(),
            [RootlessEvent::Mapped { kind: ToplevelKind::Popup { parent: Some(p) }, .. }] if *p == b),
        "{events:?}"
    );
}
