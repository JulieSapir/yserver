//! Pure tests of the shared window-system host logic. Expected values come
//! from spec §4.0/§4.2.1/§4.4/§5/§6 and the plan's tables.

use yserver_core::rootless::{
    Decorations, Geometry, GeometryFields as F, HostRequest, HostSerial, Layer, NetWmState,
    RootlessInput, SizeHints, ToplevelId, ToplevelKind, WindowInfo, WindowType,
};

use super::{
    geometry::{GeometrySync, HostSerials, NativeTarget},
    pacing::FramePacer,
    seat::{CloseAction, CloseTracker, FocusTracker, PointerTracker, XStacking, restack_requests},
    space::{GlobalSpace, NativeRect, below_title_bar, content_contains},
    style::{
        ContentLimits, NativeFlags, NativeLevel, NativeStyle, NetStateSync, content_limits,
        level_for, native_style,
    },
};
use crate::{
    rootless::host::FrameEvent,
    surface::{
        input::{
            devices::pointer_origin,
            pointer::{RawPointer, local_position},
        },
        link::ShellLink,
    },
};

fn rect(x: f64, y: f64, width: f64, height: f64) -> NativeRect {
    NativeRect {
        x,
        y,
        width,
        height,
    }
}

/// The primary display in every layout: 1512x982 points at (0,0).
const PRIMARY: NativeRect = NativeRect {
    x: 0.0,
    y: 0.0,
    width: 1512.0,
    height: 982.0,
};

fn above() -> [NativeRect; 2] {
    [PRIMARY, rect(0.0, 982.0, 1920.0, 1080.0)]
}

fn left() -> [NativeRect; 2] {
    [PRIMARY, rect(-1920.0, 0.0, 1920.0, 1080.0)]
}

fn below() -> [NativeRect; 2] {
    [PRIMARY, rect(0.0, -1080.0, 1920.0, 1080.0)]
}

#[test]
fn union_of_two_displays_is_the_root() {
    let displays = [PRIMARY, rect(1512.0, 0.0, 1920.0, 1200.0)];
    let s1 = GlobalSpace::new(&displays, 1.0, 16384);
    assert_eq!(s1.root(), (3432, 1200));
    assert_eq!(s1.to_native((0, 0), (0, 0)).y, 1200.0, "top = 1200");
    let s2 = GlobalSpace::new(&displays, 2.0, 16384);
    assert_eq!(s2.root(), (6864, 2400));
    assert_eq!(s2.scale(), 2.0);
    assert_eq!(
        GlobalSpace::new(&displays, 2.0, 8192).root(),
        (6864, 2400),
        "under the limit"
    );
    assert_eq!(GlobalSpace::new(&displays, 2.0, 4096).root(), (4096, 2400));
}

#[test]
fn x_to_native_and_back_is_identity() {
    let displays = [PRIMARY, rect(1512.0, 0.0, 1920.0, 1200.0)];
    for scale in [1.0, 2.0] {
        let space = GlobalSpace::new(&displays, scale, 16384);
        let (rw, rh) = space.root();
        for bw in [0u16, 5] {
            let extent = (100 + 2 * bw, 60 + 2 * bw);
            let corners = [
                (0, 0),
                (i32::from(rw) - i32::from(extent.0), 0),
                (0, i32::from(rh) - i32::from(extent.1)),
                (
                    i32::from(rw) - i32::from(extent.0),
                    i32::from(rh) - i32::from(extent.1),
                ),
                (3024, 218),
            ];
            for origin in corners {
                assert_eq!(
                    space.to_x(space.to_native(origin, extent)),
                    (origin, extent),
                    "S={scale} bw={bw} origin={origin:?}"
                );
            }
        }
    }
}

#[test]
fn outer_corner_is_the_content_origin() {
    // §4.0: native content (0,0) = the outer corner; §5: X (0,0) = the
    // union's top-left.
    let space = GlobalSpace::new(&above(), 2.0, 16384);
    assert_eq!(
        space.to_native((0, 0), (20, 10)),
        rect(0.0, 2062.0 - 5.0, 10.0, 5.0)
    );
}

#[test]
fn warp_point_offsets_by_the_union_top_left() {
    let alone = GlobalSpace::new(&[PRIMARY], 2.0, 16384);
    assert_eq!(alone.root(), (3024, 1964));
    assert_eq!(alone.to_warp_point((10, 20)), (5.0, 10.0));
    assert_eq!(alone.to_warp_point((0, 0)), (0.0, 0.0));

    let a1 = GlobalSpace::new(&above(), 1.0, 16384);
    assert_eq!(a1.root(), (1920, 2062));
    assert_eq!(a1.to_warp_point((10, 20)), (10.0, -1060.0));
    assert_eq!(a1.to_warp_point((0, 1080)), (0.0, 0.0));
    let a2 = GlobalSpace::new(&above(), 2.0, 16384);
    assert_eq!(a2.to_warp_point((10, 20)), (5.0, -1070.0));

    let l1 = GlobalSpace::new(&left(), 1.0, 16384);
    assert_eq!(l1.root(), (3432, 1080));
    assert_eq!(l1.to_warp_point((0, 0)), (-1920.0, -98.0));
    assert_eq!(l1.to_warp_point((1920, 98)), (0.0, 0.0));

    let b1 = GlobalSpace::new(&below(), 1.0, 16384);
    assert_eq!(b1.root(), (1920, 2062));
    assert_eq!(b1.to_warp_point((10, 1000)), (10.0, 1000.0));
    assert_eq!(b1.to_warp_point((0, 0)), (0.0, 0.0));
}

#[test]
fn native_to_root_on_every_side() {
    let cases = [
        (above(), rect(100.0, 1500.0, 400.0, 300.0), (100, 262)),
        (left(), rect(-1800.0, 200.0, 400.0, 300.0), (120, 580)),
        (below(), rect(100.0, -500.0, 400.0, 300.0), (100, 1182)),
    ];
    for (displays, content, origin) in cases {
        let space = GlobalSpace::new(&displays, 1.0, 16384);
        assert_eq!(space.to_x(content), (origin, (400, 300)), "{content:?}");
        assert_eq!(space.to_native(origin, (400, 300)), content);
    }
}

#[test]
fn local_position_flips_and_scales_without_clamping() {
    let raw = |x_pt, y_pt, scale| RawPointer {
        x_pt,
        y_pt,
        view_h_pt: 100.0,
        scale,
        dx_pt: 0.0,
        dy_pt: 0.0,
    };
    assert_eq!(local_position(&raw(0.0, 100.0, 1.0)), (0.0, 0.0));
    assert_eq!(local_position(&raw(10.0, 0.0, 2.0)), (20.0, 200.0));
    for scale in [1.0, 2.0] {
        assert_eq!(
            local_position(&raw(-3.0, 110.0, scale)),
            (-3.0 * scale, -10.0 * scale)
        );
    }
    // In range it agrees with the root-space conversion (`x_position`).
    let inside = raw(12.25, 40.5, 2.0);
    let (x, y) = local_position(&inside);
    #[allow(clippy::cast_possible_truncation)]
    let floored = (x.floor() as i32, y.floor() as i32);
    assert_eq!(
        floored,
        crate::surface::input::pointer::x_position(&inside, (400, 400))
    );
}

fn geometry(x: i32, y: i32, width: u16, height: u16, border: u16) -> Geometry {
    Geometry {
        x,
        y,
        width,
        height,
        border,
    }
}

const ID: u64 = 4;

/// The serial of a single `Moved`/`Resized`.
fn serial_of(requests: &[HostRequest]) -> HostSerial {
    match requests {
        [HostRequest::Moved { serial, .. } | HostRequest::Resized { serial, .. }] => *serial,
        other => panic!("one geometry request: {other:?}"),
    }
}

/// A window mapped at (10,20), 100x50 content, bw 2 (native 104x54).
fn synced() -> (GeometrySync, HostSerials, ToplevelId) {
    (
        GeometrySync::mapped(&geometry(10, 20, 100, 50, 2)),
        HostSerials::default(),
        ToplevelId::fake(ID),
    )
}

#[test]
fn own_move_acked_needs_nothing() {
    let (mut sync, mut serials, id) = synced();
    let sent = sync.native_changed(id, (30, 40), (104, 54), &mut serials);
    let serial = serial_of(&sent);
    assert_eq!(
        sent,
        [HostRequest::Moved {
            id,
            x: 30,
            y: 40,
            serial
        }]
    );
    assert_eq!(
        sync.configured(&geometry(30, 40, 100, 50, 2), Some(serial), F::empty()),
        None,
        "rule 4"
    );
}

#[test]
fn client_resize_coalesced_with_host_move() {
    let (mut sync, mut serials, id) = synced();
    let serial = serial_of(&sync.native_changed(id, (30, 40), (104, 54), &mut serials));
    assert_eq!(
        sync.configured(
            &geometry(30, 40, 200, 80, 2),
            Some(serial),
            F::WIDTH | F::HEIGHT
        ),
        Some(NativeTarget {
            origin: None,
            extent: Some((204, 84))
        }),
        "rule 3: the extent only"
    );
}

#[test]
fn newer_request_in_flight_ignores_older_ack() {
    let (mut sync, mut serials, id) = synced();
    let older = serial_of(&sync.native_changed(id, (30, 40), (104, 54), &mut serials));
    let newer = serial_of(&sync.native_changed(id, (31, 41), (104, 54), &mut serials));
    assert!(newer > older);
    assert_eq!(
        sync.configured(&geometry(30, 40, 100, 50, 2), Some(older), F::empty()),
        None,
        "rule 4"
    );
    assert!(
        sync.native_changed(id, (31, 41), (104, 54), &mut serials)
            .is_empty(),
        "the newer request still stands"
    );
}

#[test]
fn client_move_applies_and_does_not_echo() {
    let (mut sync, mut serials, id) = synced();
    assert_eq!(
        sync.configured(&geometry(300, 200, 100, 50, 2), None, F::X | F::Y),
        Some(NativeTarget {
            origin: Some((300, 200)),
            extent: None
        })
    );
    assert_eq!(sync.x(), ((300, 200), (104, 54)), "the rect to apply");
    assert!(
        sync.native_changed(id, (300, 200), (104, 54), &mut serials)
            .is_empty(),
        "our own setFrame reports what X has"
    );
}

#[test]
fn size_hint_clamp_snaps_the_native_window() {
    let (mut sync, mut serials, id) = synced();
    let sent = sync.native_changed(id, (10, 20), (300, 200), &mut serials);
    let serial = serial_of(&sent);
    assert_eq!(
        sent,
        [HostRequest::Resized {
            id,
            width: 300,
            height: 200,
            serial
        }]
    );
    // bw 2: content 286x196 = native 290x200.
    assert_eq!(
        sync.configured(
            &geometry(10, 20, 286, 196, 2),
            Some(serial),
            F::WIDTH | F::HEIGHT
        ),
        Some(NativeTarget {
            origin: None,
            extent: Some((290, 200))
        }),
        "rule 5"
    );
    assert!(
        sync.native_changed(id, (10, 20), (290, 200), &mut serials)
            .is_empty()
    );
}

#[test]
fn system_constraint_is_reported() {
    let (mut sync, mut serials, id) = synced();
    assert_eq!(
        sync.configured(&geometry(10, -20, 100, 50, 2), None, F::Y),
        Some(NativeTarget {
            origin: Some((10, -20)),
            extent: None
        })
    );
    let sent = sync.native_changed(id, (10, 25), (104, 54), &mut serials);
    assert_eq!(
        sent,
        [HostRequest::Moved {
            id,
            x: 10,
            y: 25,
            serial: serial_of(&sent)
        }],
        "X follows the constrained native window"
    );
}

#[test]
fn live_drag_posts_every_step() {
    let (mut sync, mut serials, id) = synced();
    let steps = [(11, 20), (12, 21), (14, 23)];
    let serials_sent: Vec<HostSerial> = steps
        .iter()
        .map(|&origin| {
            let sent = sync.native_changed(id, origin, (104, 54), &mut serials);
            assert!(matches!(sent[..], [HostRequest::Moved { x, y, .. }] if (x, y) == origin));
            serial_of(&sent)
        })
        .collect();
    assert!(
        serials_sent.windows(2).all(|p| p[0] < p[1]),
        "{serials_sent:?}"
    );
}

#[test]
fn a_move_and_resize_together_post_both() {
    let (mut sync, mut serials, id) = synced();
    let sent = sync.native_changed(id, (0, 0), (204, 154), &mut serials);
    assert!(
        matches!(
            sent[..],
            [
                HostRequest::Moved { x: 0, y: 0, serial: a, .. },
                HostRequest::Resized { width: 204, height: 154, serial: b, .. }
            ] if a < b
        ),
        "{sent:?}"
    );
}

fn managed(info: &WindowInfo) -> NativeStyle {
    native_style(ToplevelKind::Managed, info)
}

#[test]
fn managed_defaults() {
    assert_eq!(
        managed(&WindowInfo::default()),
        NativeStyle {
            titled: true,
            closable: true,
            miniaturizable: true,
            resizable: true,
            level: NativeLevel::Normal,
            can_activate: true,
            child_of: None,
            fullscreen: true,
        }
    );
}

/// `_MOTIF_WM_HINTS` decorations (`hw/xwin/winwindow.h`): 0 = none,
/// `MwmDecorTitle` = 1 << 3, `MwmDecorBorder` = 1 << 1.
#[test]
fn motif_none_is_borderless() {
    let style = |decorations| {
        managed(&WindowInfo {
            decorations,
            ..WindowInfo::default()
        })
    };
    let none = style(Decorations::None);
    assert!(!none.titled && !none.closable && !none.miniaturizable);
    assert!(none.can_activate, "still a managed window");
    assert!(style(Decorations::Subset((1 << 1) | (1 << 3))).titled);
    assert!(!style(Decorations::Subset(1 << 1)).titled, "border only");
}

#[test]
fn fixed_size_is_not_resizable() {
    let hints = |min, max| WindowInfo {
        size_hints: SizeHints {
            min,
            max,
            ..SizeHints::default()
        },
        ..WindowInfo::default()
    };
    assert!(!managed(&hints(Some((200, 100)), Some((200, 100)))).resizable);
    assert!(managed(&hints(Some((200, 100)), Some((400, 100)))).resizable);
    assert!(managed(&hints(Some((200, 100)), None)).resizable);
}

#[test]
fn transient_dialog_is_a_child() {
    let parent = ToplevelId::fake(9);
    let style = managed(&WindowInfo {
        transient_for: Some(parent),
        window_type: WindowType::Dialog,
        ..WindowInfo::default()
    });
    assert_eq!(style.child_of, Some(parent));
    assert!(style.fullscreen, "dialogs may go fullscreen");
    let utility = managed(&WindowInfo {
        window_type: WindowType::Utility,
        ..WindowInfo::default()
    });
    assert!(!utility.fullscreen, "only Normal and Dialog");
}

fn with_state(state: NetWmState) -> WindowInfo {
    WindowInfo {
        state,
        ..WindowInfo::default()
    }
}

#[test]
fn above_floats() {
    assert_eq!(level_for(Layer::Above), NativeLevel::Floating);
    let state = NetWmState {
        above: true,
        ..NetWmState::default()
    };
    assert_eq!(managed(&with_state(state)).level, NativeLevel::Floating);
}

#[test]
fn below_sinks() {
    assert_eq!(level_for(Layer::Below), NativeLevel::Below);
    assert_eq!(level_for(Layer::Normal), NativeLevel::Normal);
    let state = NetWmState {
        below: true,
        ..NetWmState::default()
    };
    assert_eq!(managed(&with_state(state)).level, NativeLevel::Below);
}

/// §6.2: popups are never activated.
#[test]
fn popup_never_activates() {
    let info = WindowInfo {
        transient_for: Some(ToplevelId::fake(9)),
        ..WindowInfo::default()
    };
    let style = native_style(ToplevelKind::Popup { parent: None }, &info);
    assert_eq!(
        style,
        NativeStyle {
            titled: false,
            closable: false,
            miniaturizable: false,
            resizable: false,
            level: NativeLevel::PopUp,
            can_activate: false,
            child_of: None,
            fullscreen: false,
        }
    );
}

/// ICCCM §4.1.2.3: the base size stands in for a missing minimum.
#[test]
fn limits_add_the_border_and_fall_back_to_base() {
    let hints = SizeHints {
        min: Some((100, 50)),
        max: Some((400, 300)),
        inc: Some((6, 13)),
        base: Some((10, 4)),
        ..SizeHints::default()
    };
    assert_eq!(
        content_limits(&hints, 2),
        ContentLimits {
            min: Some((104, 54)),
            max: Some((404, 304)),
            inc: Some((6, 13)),
        }
    );
    let base_only = SizeHints {
        base: Some((10, 4)),
        ..SizeHints::default()
    };
    assert_eq!(
        content_limits(&base_only, 0),
        ContentLimits {
            min: Some((10, 4)),
            max: None,
            inc: None,
        }
    );
    let huge = SizeHints {
        max: Some((u32::MAX, 70_000)),
        ..SizeHints::default()
    };
    assert_eq!(
        content_limits(&huge, 1).max,
        Some((u16::MAX, u16::MAX)),
        "clamped"
    );
}

fn flags(zoomed: bool, fullscreen: bool, minimized: bool, level: NativeLevel) -> NativeFlags {
    NativeFlags {
        zoomed,
        fullscreen,
        minimized,
        level,
    }
}

fn reported(request: Option<HostRequest>) -> NetWmState {
    match request {
        Some(HostRequest::StateChanged { id, state }) => {
            assert_eq!(id, ToplevelId::fake(ID));
            state
        }
        other => panic!("a StateChanged: {other:?}"),
    }
}

const NORMAL: NativeLevel = NativeLevel::Normal;

#[test]
fn user_zoom_reports_maximized() {
    let id = ToplevelId::fake(ID);
    let mut sync = NetStateSync::mapped(NetWmState::default());
    let state = reported(sync.native(id, flags(true, false, false, NORMAL)));
    assert!(state.maximized_vert && state.maximized_horz);
    assert_eq!(sync.native(id, flags(true, false, false, NORMAL)), None);
    let state = reported(sync.native(id, flags(false, false, false, NORMAL)));
    assert!(!state.maximized_vert && !state.maximized_horz);
}

/// A `SetMaximized(true)` the host applied is read back like a user zoom.
#[test]
fn client_request_is_reported_back() {
    let id = ToplevelId::fake(ID);
    let mut sync = NetStateSync::mapped(NetWmState::default());
    // The client's toggle became WmRequest::SetMaximized(true); the host
    // zoomed and reads the window back.
    let state = reported(sync.native(id, flags(true, false, false, NORMAL)));
    assert_eq!(
        state,
        NetWmState {
            maximized_vert: true,
            maximized_horz: true,
            ..NetWmState::default()
        }
    );
}

#[test]
fn layers_report_above_and_below() {
    let id = ToplevelId::fake(ID);
    let mut sync = NetStateSync::mapped(NetWmState::default());
    let state = reported(sync.native(id, flags(false, false, false, NativeLevel::Floating)));
    assert!(state.above && !state.below);
    let state = reported(sync.native(id, flags(false, false, false, NativeLevel::Below)));
    assert!(state.below && !state.above);
    let state = reported(sync.native(id, flags(false, false, false, NORMAL)));
    assert!(!state.below && !state.above);
}

#[test]
fn fullscreen_and_minimize_report() {
    let id = ToplevelId::fake(ID);
    let mut sync = NetStateSync::mapped(NetWmState {
        above: true,
        ..NetWmState::default()
    });
    let floating = NativeLevel::Floating;
    let state = reported(sync.native(id, flags(false, true, false, floating)));
    assert!(state.fullscreen && state.above);
    let state = reported(sync.native(id, flags(false, true, true, floating)));
    assert!(
        state.hidden && state.fullscreen && state.above,
        "others kept"
    );
}

#[test]
fn client_fields_ride_along() {
    let id = ToplevelId::fake(ID);
    let mut sync = NetStateSync::mapped(NetWmState::default());
    sync.client_state(NetWmState {
        skip_taskbar: true,
        modal: true,
        // Not the client's to set through this path.
        fullscreen: true,
        ..NetWmState::default()
    });
    let state = reported(sync.native(id, flags(true, false, false, NORMAL)));
    assert!(state.skip_taskbar && state.modal);
    assert!(!state.fullscreen, "native fields come from the window");
}

fn ids<const N: usize>() -> [ToplevelId; N] {
    std::array::from_fn(|i| ToplevelId::fake(10 + i as u64))
}

/// §6.2: focus moves between our windows without a None in between.
#[test]
fn switching_between_our_windows_never_deactivates() {
    let [a, b] = ids();
    let mut focus = FocusTracker::default();
    let mut sent = Vec::new();
    sent.extend(focus.became_key(a));
    focus.resigned_key();
    sent.extend(focus.became_key(b));
    sent.extend(focus.settle(Some(b)));
    assert_eq!(
        sent,
        [
            HostRequest::Activated { id: a },
            HostRequest::Activated { id: b }
        ]
    );
    assert_eq!(focus.became_key(b), None, "once");
}

#[test]
fn app_switch_deactivates_once() {
    let [a] = ids();
    let mut focus = FocusTracker::default();
    assert_eq!(focus.became_key(a), Some(HostRequest::Activated { id: a }));
    focus.resigned_key();
    assert_eq!(focus.settle(None), Some(HostRequest::Deactivated));
    assert_eq!(focus.app_deactivated(), None, "already");
    assert_eq!(focus.settle(None), None);
    assert_eq!(
        focus.became_key(a),
        Some(HostRequest::Activated { id: a }),
        "back"
    );
    assert_eq!(focus.app_deactivated(), Some(HostRequest::Deactivated));
}

/// §4.4: `PointerLeave` releases buttons, so never mid-drag.
#[test]
fn leave_waits_for_the_button() {
    let [a] = ids();
    let mut pointer = PointerTracker::default();
    pointer.moved(a, (1.0, 1.0), 1);
    assert_eq!(pointer.button(true, 2), None);
    assert_eq!(pointer.exited(a, 3), None, "a button is down");
    assert_eq!(pointer.inside(), Some(a));
    assert_eq!(
        pointer.button(false, 4),
        Some(RootlessInput::PointerLeave {
            origin: pointer_origin(),
            time: 4
        })
    );
    assert_eq!(pointer.inside(), None);
    assert_eq!(pointer.exited(a, 5), None, "left already");
}

#[test]
fn leave_without_buttons_is_immediate() {
    let [a] = ids();
    let mut pointer = PointerTracker::default();
    pointer.moved(a, (1.0, 1.0), 1);
    assert_eq!(
        pointer.exited(a, 2),
        Some(RootlessInput::PointerLeave {
            origin: pointer_origin(),
            time: 2
        })
    );
    assert_eq!(pointer.button(true, 3), None);
    assert_eq!(pointer.button(false, 4), None, "nothing deferred");
}

#[test]
fn enter_then_motion() {
    let [a, b] = ids();
    let mut pointer = PointerTracker::default();
    let origin = pointer_origin();
    assert_eq!(
        pointer.moved(a, (1.0, 2.0), 1),
        RootlessInput::PointerEnter {
            origin,
            toplevel: a,
            local: (1.0, 2.0),
            time: 1
        }
    );
    assert_eq!(
        pointer.moved(a, (3.0, 4.0), 2),
        RootlessInput::PointerMotionLocal {
            origin,
            toplevel: a,
            local: (3.0, 4.0),
            time: 2
        }
    );
    assert_eq!(
        pointer.moved(b, (5.0, 6.0), 3),
        RootlessInput::PointerEnter {
            origin,
            toplevel: b,
            local: (5.0, 6.0),
            time: 3
        }
    );
    assert_eq!(
        pointer.moved(b, (7.0, 8.0), 4),
        RootlessInput::PointerMotionLocal {
            origin,
            toplevel: b,
            local: (7.0, 8.0),
            time: 4
        }
    );
    assert_eq!(pointer.inside(), Some(b));
}

/// Decision 12: the second close asks to kill; a remap is a new tracker.
#[test]
fn second_close_asks_to_kill() {
    let mut close = CloseTracker::default();
    assert_eq!(close.clicked(), CloseAction::Request);
    assert_eq!(close.clicked(), CloseAction::ConfirmKill);
    assert_eq!(close.clicked(), CloseAction::ConfirmKill);
    assert_eq!(CloseTracker::default().clicked(), CloseAction::Request);
}

/// Apply restacks to an X bottom-to-top order.
fn apply(order: &mut Vec<ToplevelId>, restacks: &[(ToplevelId, Option<ToplevelId>)]) {
    for &(id, above) in restacks {
        order.retain(|&w| w != id);
        let at = above.map_or(0, |a| {
            order.iter().position(|&w| w == a).expect("above") + 1
        });
        order.insert(at, id);
    }
}

#[test]
fn restack_moves_only_what_differs() {
    let [a, b, c, d] = ids();
    let native = [c, a, b];
    let x = [a, b, c];
    let restacks = restack_requests(&native, &x);
    let mut model = x.to_vec();
    apply(&mut model, &restacks);
    assert_eq!(model, [b, a, c], "native order, bottom to top");
    assert_eq!(restacks.len(), 1, "{restacks:?}");
    assert_eq!(restack_requests(&[c, b, a], &[a, b, c]), [], "equal");
    // Windows only one side knows are left alone.
    let restacks = restack_requests(&[d, b, a], &[b, a, c]);
    let mut model = vec![b, a, c];
    apply(&mut model, &restacks);
    assert_eq!(model, [a, b, c]);
    for native in [[a, b, c, d], [d, c, b, a], [b, d, a, c], [c, a, d, b]] {
        let x = [a, b, c, d];
        let mut model = x.to_vec();
        apply(&mut model, &restack_requests(&native, &x));
        let mut want = native.to_vec();
        want.reverse();
        assert_eq!(model, want, "{native:?}");
    }
}

#[test]
fn pacer_answers_each_wanted_id_once() {
    let [a, b] = ids();
    let link = std::sync::Arc::new(ShellLink::default());
    let pacer = FramePacer::new(std::sync::Arc::clone(&link));
    pacer.want(a);
    pacer.want(a);
    pacer.want(b);
    pacer.tick();
    let ready: Vec<ToplevelId> = link
        .drain_frames()
        .into_iter()
        .map(|e| match e {
            FrameEvent::Ready { id } => id,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(ready, [a, b]);
    pacer.tick();
    assert!(link.drain_frames().is_empty(), "drained");
}

/// The primary below its 24-point menu bar.
const VISIBLE: NativeRect = NativeRect {
    x: 0.0,
    y: 0.0,
    width: 1512.0,
    height: 958.0,
};
const TITLE_BAR: f64 = 28.0;

/// An X window at the root's top would put its title bar under the menu
/// bar: the content moves down by menu bar + title bar, and X hears of it.
#[test]
fn title_bar_stays_reachable_and_x_follows() {
    for scale in [1.0, 2.0] {
        let space = GlobalSpace::new(&[PRIMARY], scale, 16384);
        #[allow(clippy::cast_possible_truncation)]
        let shift = (52.0 * scale) as i32;
        let extent = (400, 300);
        let content = space.to_native((0, 0), extent);
        let placed = below_title_bar(content, TITLE_BAR, VISIBLE);
        assert_eq!(
            placed.y + placed.height + TITLE_BAR,
            958.0,
            "title bar just under the menu bar"
        );
        assert_eq!(
            (placed.x, placed.width, placed.height),
            (0.0, content.width, content.height)
        );
        let (origin, got) = space.to_x(placed);
        assert_eq!((origin, got), ((0, shift), extent), "S={scale}");

        let id = ToplevelId::fake(ID);
        let mut sync = GeometrySync::mapped(&geometry(0, 0, 400, 300, 0));
        let mut serials = HostSerials::default();
        let sent = sync.native_changed(id, origin, got, &mut serials);
        assert_eq!(
            sent,
            [HostRequest::Moved {
                id,
                x: 0,
                y: shift,
                serial: serial_of(&sent)
            }]
        );
    }
}

#[test]
fn a_window_lower_down_is_left_where_x_put_it() {
    let space = GlobalSpace::new(&[PRIMARY], 1.0, 16384);
    for y in [52, 100, 600] {
        let content = space.to_native((10, y), (400, 300));
        assert_eq!(
            below_title_bar(content, TITLE_BAR, VISIBLE),
            content,
            "y={y}"
        );
    }
}

#[test]
fn x_stacking_follows_mapped_and_restacked() {
    let [a, b, c] = ids();
    let mut x = XStacking::default();
    x.place(a, None);
    x.place(b, Some(a));
    x.place(c, Some(b));
    assert_eq!(x.bottom_to_top(), [a, b, c]);
    x.place(c, None);
    assert_eq!(x.bottom_to_top(), [c, a, b], "to the bottom");
    x.place(c, Some(b));
    assert_eq!(x.bottom_to_top(), [a, b, c], "back on top");
    x.place(a, Some(b));
    assert_eq!(x.bottom_to_top(), [b, a, c], "directly above b");
    x.remove(a);
    assert_eq!(x.bottom_to_top(), [b, c]);
    x.place(a, Some(ToplevelId::fake(99)));
    assert_eq!(x.bottom_to_top(), [b, c, a], "an unknown sibling: on top");
    // What the host reports moves X to the native order.
    let restacks = restack_requests(&[b, a, c], x.bottom_to_top());
    for (id, above) in restacks {
        x.place(id, above);
    }
    assert_eq!(x.bottom_to_top(), [c, a, b]);
}

/// macOS during a drag: the exit is deferred, drag motion keeps coming to
/// the window that got the mouse-down (outside it), and the release sends
/// the position first. The leave still arrives with the release.
#[test]
fn drag_motion_after_the_exit_keeps_the_leave() {
    let [a] = ids();
    let mut pointer = PointerTracker::default();
    let origin = pointer_origin();
    pointer.moved(a, (5.0, 5.0), 1);
    assert_eq!(pointer.button(true, 2), None);
    assert_eq!(pointer.exited(a, 3), None, "deferred");
    for (time, x) in [(4, -10.0), (5, -20.0)] {
        assert_eq!(
            pointer.moved(a, (x, 5.0), time),
            RootlessInput::PointerMotionLocal {
                origin,
                toplevel: a,
                local: (x, 5.0),
                time
            },
            "still the grab window's motion"
        );
    }
    // `button_local`: motion, then the release.
    pointer.moved(a, (-25.0, 5.0), 6);
    assert_eq!(
        pointer.button(false, 6),
        Some(RootlessInput::PointerLeave { origin, time: 6 })
    );
    assert_eq!(pointer.inside(), None, "no longer inside");
}

#[test]
fn re_entry_before_the_release_cancels_the_leave() {
    let [a] = ids();
    let mut pointer = PointerTracker::default();
    pointer.moved(a, (5.0, 5.0), 1);
    pointer.button(true, 2);
    pointer.exited(a, 3);
    pointer.moved(a, (-10.0, 5.0), 4);
    assert_eq!(
        pointer.entered(a, (2.0, 5.0), 5),
        RootlessInput::PointerMotionLocal {
            origin: pointer_origin(),
            toplevel: a,
            local: (2.0, 5.0),
            time: 5
        }
    );
    pointer.moved(a, (3.0, 5.0), 6);
    assert_eq!(pointer.button(false, 6), None, "back inside");
    assert_eq!(pointer.inside(), Some(a));
}

/// A press in A, a drag into popup P: the window under the pointer gets the
/// motion (Enter P, never a Leave that would release the button), and the
/// release happens in P. Back in A before the release: Enter A again.
#[test]
fn a_drag_crosses_into_the_window_under_the_pointer() {
    let [a, p] = ids();
    let origin = pointer_origin();
    let mut pointer = PointerTracker::default();
    pointer.moved(a, (5.0, 5.0), 1);
    pointer.button(true, 2);
    assert!(pointer.dragging());
    assert_eq!(pointer.exited(a, 3), None, "deferred while held");
    assert_eq!(
        pointer.dragged(Some(p), (4.0, 6.0), 4),
        Some(RootlessInput::PointerEnter {
            origin,
            toplevel: p,
            local: (4.0, 6.0),
            time: 4
        })
    );
    assert_eq!(
        pointer.dragged(Some(p), (7.0, 8.0), 5),
        Some(RootlessInput::PointerMotionLocal {
            origin,
            toplevel: p,
            local: (7.0, 8.0),
            time: 5
        })
    );
    assert_eq!(pointer.button(false, 6), None, "released inside P");
    assert_eq!(pointer.inside(), Some(p));

    // Out and back before the release.
    pointer.button(true, 7);
    assert!(matches!(
        pointer.dragged(Some(a), (1.0, 1.0), 8),
        Some(RootlessInput::PointerEnter { toplevel, .. }) if toplevel == a
    ));
    assert_eq!(pointer.button(false, 9), None);
    assert_eq!(pointer.inside(), Some(a));

    // Over none of ours at the release: the leave comes with it.
    pointer.button(true, 10);
    assert!(matches!(
        pointer.dragged(None, (-30.0, 1.0), 11),
        Some(RootlessInput::PointerMotionLocal { toplevel, .. }) if toplevel == a
    ));
    assert_eq!(
        pointer.button(false, 12),
        Some(RootlessInput::PointerLeave { origin, time: 12 })
    );
    assert_eq!(pointer.inside(), None);
}

/// A drag from X content up into the window's own title bar: the title bar
/// is none of X's, so no Enter, the leave stays pending and comes with the
/// release; back into the content before the release cancels it.
#[test]
fn a_drag_into_the_title_bar_is_outside_x() {
    let [a] = ids();
    // Content 400x300 at (100,100); the title bar is the 28 points above.
    let content = rect(100.0, 100.0, 400.0, 300.0);
    let under = |point: (f64, f64)| content_contains(content, point).then_some(a);
    assert_eq!(under((100.0, 100.0)), Some(a), "bottom-left corner");
    assert_eq!(under((300.0, 399.5)), Some(a));
    assert_eq!(under((300.0, 410.0)), None, "the title bar");
    assert_eq!(under((500.0, 200.0)), None, "right edge excluded");

    let origin = pointer_origin();
    for back in [false, true] {
        let mut pointer = PointerTracker::default();
        pointer.moved(a, (10.0, 10.0), 1);
        pointer.button(true, 2);
        pointer.exited(a, 3);
        let in_title_bar = pointer.dragged(under((300.0, 410.0)), (200.0, -10.0), 4);
        assert!(
            matches!(in_title_bar, Some(RootlessInput::PointerMotionLocal { toplevel, .. }) if toplevel == a),
            "no Enter: {in_title_bar:?}"
        );
        if back {
            pointer.dragged(under((300.0, 350.0)), (200.0, 50.0), 5);
            assert_eq!(pointer.button(false, 6), None, "back inside: no leave");
            assert_eq!(pointer.inside(), Some(a));
        } else {
            assert_eq!(
                pointer.button(false, 6),
                Some(RootlessInput::PointerLeave { origin, time: 6 })
            );
            assert_eq!(pointer.inside(), None);
        }
    }
}
