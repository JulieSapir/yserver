//! Plan RL0 Task 4: `WindowInfo` decoding and `InfoChanged`.

use yserver_protocol::x11::{ClientByteOrder, ResourceId};

use super::RecordingHost;
use crate::{
    resources::ROOT_WINDOW,
    rootless::{
        Decorations, InfoFields, RootlessEvent, SizeHints, ToplevelKind, WindowInfo, WindowType,
        parse_window_info,
    },
};

const W: u32 = 0x0020_0001;
const BE: u32 = 2;

fn host_with_window() -> RecordingHost {
    let mut host = RecordingHost::new();
    host.connect(BE, ClientByteOrder::BigEndian);
    host.create_window(1, W, ROOT_WINDOW.0, (0, 0), (50, 40), 0, false);
    host
}

fn info(host: &RecordingHost) -> WindowInfo {
    parse_window_info(&host.state, ResourceId(W), &|_| None)
}

/// `WM_NORMAL_HINTS` (ICCCM §4.1.2.3; flags `Xutil.h:102-112`): xterm's hint
/// set, `PMinSize|PResizeInc|PBaseSize|PWinGravity` = 0x350, min 10×17, inc
/// 6×13, base 4×4, gravity NorthWest. Identical from an LE and a BE writer
/// (P1 stores property data in server order). Mutation: decode the stored
/// words big-endian.
#[test]
fn wm_normal_hints_from_le_and_be_writers() {
    let hints: [u32; 18] = [0x350, 0, 0, 0, 0, 10, 17, 0, 0, 6, 13, 0, 0, 0, 0, 4, 4, 1];
    let expected = SizeHints {
        min: Some((10, 17)),
        inc: Some((6, 13)),
        base: Some((4, 4)),
        gravity: Some(1),
        ..SizeHints::default()
    };
    for client in [1, BE] {
        let mut host = host_with_window();
        host.change_property(client, W, "WM_NORMAL_HINTS", "WM_SIZE_HINTS", 32, &hints);
        assert_eq!(info(&host).size_hints, expected, "client {client}");
        assert_eq!(info(&host).size_hints.max, None);
    }
    // Pre-ICCCM-1, 15 words: no base, no gravity.
    let mut host = host_with_window();
    host.change_property(1, W, "WM_NORMAL_HINTS", "WM_SIZE_HINTS", 32, &hints[..15]);
    assert_eq!(info(&host).size_hints.base, None);
    assert_eq!(info(&host).size_hints.min, Some((10, 17)));
}

/// `WM_HINTS` (ICCCM §4.1.2.4; `InputHint` bit 0, `XUrgencyHint` bit 8).
#[test]
fn wm_hints_input_and_urgency() {
    let mut host = host_with_window();
    assert_eq!(info(&host).input_hint, None, "no property");
    host.change_property(
        1,
        W,
        "WM_HINTS",
        "WM_HINTS",
        32,
        &[1, 0, 0, 0, 0, 0, 0, 0, 0],
    );
    assert_eq!(info(&host).input_hint, Some(false));
    assert!(!info(&host).urgency);
    host.change_property(
        1,
        W,
        "WM_HINTS",
        "WM_HINTS",
        32,
        &[0x100, 0, 0, 0, 0, 0, 0, 0, 0],
    );
    assert_eq!(info(&host).input_hint, None);
    assert!(info(&host).urgency);
}

/// EWMH §5.1: `_NET_WM_NAME` "MUST be used in preference to" `WM_NAME`;
/// `WM_NAME` of type `STRING` is Latin-1 (ICCCM §2.7.1). Mutation: prefer
/// `WM_NAME`.
#[test]
fn net_wm_name_over_wm_name_and_latin1_wm_name() {
    let mut host = host_with_window();
    host.change_property_bytes(1, W, "WM_NAME", "STRING", &[0xE9]);
    assert_eq!(info(&host).title, "é");
    host.change_property_bytes(1, W, "_NET_WM_NAME", "UTF8_STRING", "ünï".as_bytes());
    assert_eq!(info(&host).title, "ünï");
}

/// `_MOTIF_WM_HINTS` flags/functions/decorations (`hw/xwin/winwindow.h:114-136`):
/// `MwmHintsDecorations` (bit 1) with decorations 0 is no decoration.
#[test]
fn motif_hints_no_decorations() {
    let mut host = host_with_window();
    host.change_property(
        1,
        W,
        "_MOTIF_WM_HINTS",
        "_MOTIF_WM_HINTS",
        32,
        &[2, 0, 0, 0, 0],
    );
    assert_eq!(info(&host).decorations, Decorations::None);
    host.change_property(
        1,
        W,
        "_MOTIF_WM_HINTS",
        "_MOTIF_WM_HINTS",
        32,
        &[0, 0, 0, 0, 0],
    );
    assert_eq!(info(&host).decorations, Decorations::Full);
}

/// EWMH §5.6: the first known type wins; with none, a transient is a Dialog.
#[test]
fn net_wm_window_type_first_known_and_transient_default() {
    let mut host = host_with_window();
    let dialog = host.atom("_NET_WM_WINDOW_TYPE_DIALOG");
    host.change_property(1, W, "_NET_WM_WINDOW_TYPE", "ATOM", 32, &[dialog]);
    assert_eq!(info(&host).window_type, WindowType::Dialog);

    let unknown = host.atom("_KDE_NET_WM_WINDOW_TYPE_OVERRIDE");
    let normal = host.atom("_NET_WM_WINDOW_TYPE_NORMAL");
    host.change_property(1, W, "_NET_WM_WINDOW_TYPE", "ATOM", 32, &[unknown, normal]);
    assert_eq!(info(&host).window_type, WindowType::Normal);

    host.delete_property(1, W, "_NET_WM_WINDOW_TYPE");
    host.change_property(1, W, "WM_TRANSIENT_FOR", "WINDOW", 32, &[ROOT_WINDOW.0]);
    assert_eq!(info(&host).window_type, WindowType::Dialog);
}

/// `WM_CLASS` (ICCCM §4.1.2.5): instance then class, NUL-terminated.
#[test]
fn wm_class_instance_and_class() {
    let mut host = host_with_window();
    host.change_property_bytes(1, W, "WM_CLASS", "STRING", b"xterm\0XTerm\0");
    assert_eq!(
        info(&host).class,
        Some(("xterm".to_owned(), "XTerm".to_owned()))
    );
}

/// Spec §4.3: a property change after map reaches the host as `InfoChanged`
/// with only the changed field. Mutation: no dirty hook in ChangeProperty.
#[test]
fn title_change_emits_info_changed_with_only_title() {
    let mut host = host_with_window();
    let id = host.map_and_reconcile(1, W);
    host.change_property_bytes(1, W, "_NET_WM_NAME", "UTF8_STRING", b"hello");
    let events = host.reconcile();
    let [
        RootlessEvent::InfoChanged {
            id: got,
            info,
            changed,
        },
    ] = events.as_slice()
    else {
        panic!("expected one InfoChanged: {events:?}");
    };
    assert_eq!(*got, id);
    assert_eq!(*changed, InfoFields::TITLE);
    assert_eq!(info.title, "hello");
    assert_eq!(host.reconcile(), vec![], "reported once");
}

/// The same through DeleteProperty. Mutation: no dirty hook in DeleteProperty.
#[test]
fn delete_property_marks_dirty() {
    let mut host = host_with_window();
    host.change_property_bytes(1, W, "_NET_WM_NAME", "UTF8_STRING", b"hello");
    host.map_and_reconcile(1, W);
    host.delete_property(1, W, "_NET_WM_NAME");
    let events = host.reconcile();
    assert!(
        matches!(events.as_slice(),
            [RootlessEvent::InfoChanged { changed, info, .. }]
                if *changed == InfoFields::TITLE && info.title.is_empty()),
        "{events:?}"
    );
}

/// `Mapped` carries the info parsed at map time.
#[test]
fn mapped_carries_parsed_info() {
    let mut host = host_with_window();
    host.change_property_bytes(1, W, "WM_CLASS", "STRING", b"xterm\0XTerm\0");
    host.map(1, W);
    let events = host.reconcile();
    assert!(
        matches!(events.as_slice(),
            [RootlessEvent::Mapped { info, .. }]
                if info.class == Some(("xterm".to_owned(), "XTerm".to_owned()))),
        "{events:?}"
    );
}

/// Spec §10 popup parent order, rung 1: `WM_TRANSIENT_FOR` naming a
/// Managed top-level wins over the topmost. Mutation: drop the rung.
#[test]
fn popup_parent_prefers_transient_for() {
    let mut host = host_with_window();
    host.create_window(1, 0x0020_0002, ROOT_WINDOW.0, (100, 0), (50, 40), 0, false);
    let lower = host.map_and_reconcile(1, W);
    host.map_and_reconcile(1, 0x0020_0002);
    host.create_window(1, 0x0020_0010, ROOT_WINDOW.0, (300, 300), (10, 10), 0, true);
    host.change_property(1, 0x0020_0010, "WM_TRANSIENT_FOR", "WINDOW", 32, &[W]);
    host.map(1, 0x0020_0010);
    let events = host.reconcile();
    let [RootlessEvent::Mapped { kind, info, .. }] = events.as_slice() else {
        panic!("expected one Mapped: {events:?}");
    };
    assert_eq!(
        *kind,
        ToplevelKind::Popup {
            parent: Some(lower)
        }
    );
    assert_eq!(info.transient_for, Some(lower));
}

/// RotateProperties is a property writer too (`props.rs`, Xorg
/// `ProcRotateProperties`). Mutation: no dirty hook in RotateProperties.
#[test]
fn rotate_properties_marks_dirty() {
    let mut host = host_with_window();
    host.change_property_bytes(1, W, "WM_CLASS", "STRING", b"xterm\0XTerm\0");
    host.change_property_bytes(1, W, "_PROBE_CLASS", "STRING", b"uxterm\0UXTerm\0");
    host.map_and_reconcile(1, W);
    let (class, probe) = (host.atom("WM_CLASS"), host.atom("_PROBE_CLASS"));
    let mut body = W.to_le_bytes().to_vec();
    body.extend_from_slice(&2u16.to_le_bytes());
    body.extend_from_slice(&1i16.to_le_bytes());
    body.extend_from_slice(&class.to_le_bytes());
    body.extend_from_slice(&probe.to_le_bytes());
    let out = host.request(1, 114, 0, &body);
    super::assert_no_error(&out, "RotateProperties");
    let events = host.reconcile();
    assert!(
        matches!(events.as_slice(),
            [RootlessEvent::InfoChanged { changed, info, .. }]
                if *changed == InfoFields::CLASS
                    && info.class == Some(("uxterm".to_owned(), "UXTerm".to_owned()))),
        "{events:?}"
    );
}

// ---- Transient parents that resolve or change after map ----

const P: u32 = 0x0020_0005;
const Q: u32 = 0x0020_0010;

fn transient_events(
    events: &[RootlessEvent],
    window_id: crate::rootless::ToplevelId,
) -> Vec<RootlessEvent> {
    events
        .iter()
        .filter(|e| match e {
            RootlessEvent::InfoChanged { id, .. } | RootlessEvent::ParentChanged { id, .. } => {
                *id == window_id
            }
            _ => false,
        })
        .cloned()
        .collect()
}

/// A transient mapped before its parent: once the parent maps, the host hears
/// the resolved `transient_for` (`InfoChanged`), and a popup its new parent
/// (`ParentChanged`). Mutation: resolve transients only at map/property change.
#[test]
fn transient_parent_mapped_later_resolves() {
    let mut host = host_with_window();
    host.create_window(1, P, ROOT_WINDOW.0, (200, 200), (50, 40), 0, false);
    host.change_property(1, W, "WM_TRANSIENT_FOR", "WINDOW", 32, &[P]);
    let dialog = host.map_and_reconcile(1, W);
    host.create_window(1, Q, ROOT_WINDOW.0, (300, 300), (10, 10), 0, true);
    host.change_property(1, Q, "WM_TRANSIENT_FOR", "WINDOW", 32, &[P]);
    let popup = host.map_and_reconcile(1, Q);

    host.map(1, P);
    let events = host.reconcile();
    let Some(RootlessEvent::Mapped { id: parent, .. }) = events.first() else {
        panic!("parent Mapped first: {events:?}");
    };
    let parent = *parent;
    assert!(
        matches!(transient_events(&events, dialog).as_slice(),
            [RootlessEvent::InfoChanged { changed, info, .. }]
                if *changed == InfoFields::TRANSIENT_FOR && info.transient_for == Some(parent)),
        "{events:?}"
    );
    let popup_events = transient_events(&events, popup);
    assert!(
        matches!(popup_events.as_slice(),
            [RootlessEvent::InfoChanged { changed, .. },
             RootlessEvent::ParentChanged { parent: Some(p), .. }]
                if *changed == InfoFields::TRANSIENT_FOR && *p == parent),
        "{events:?}"
    );
}

/// `WM_TRANSIENT_FOR` changed on a mapped popup moves its parent.
/// Mutation: update `WindowInfo` only, leave `ToplevelKind::Popup` alone.
#[test]
fn transient_for_change_on_mapped_popup_changes_its_parent() {
    let mut host = host_with_window();
    host.create_window(1, P, ROOT_WINDOW.0, (200, 200), (50, 40), 0, false);
    let a = host.map_and_reconcile(1, W);
    let b = host.map_and_reconcile(1, P);
    host.create_window(1, Q, ROOT_WINDOW.0, (300, 300), (10, 10), 0, true);
    host.change_property(1, Q, "WM_TRANSIENT_FOR", "WINDOW", 32, &[W]);
    host.map(1, Q);
    let events = host.reconcile();
    let [
        RootlessEvent::Mapped {
            id: popup, kind, ..
        },
    ] = events.as_slice()
    else {
        panic!("{events:?}");
    };
    assert_eq!(*kind, ToplevelKind::Popup { parent: Some(a) });
    let popup = *popup;

    host.change_property(1, Q, "WM_TRANSIENT_FOR", "WINDOW", 32, &[P]);
    let events = host.reconcile();
    assert!(
        matches!(events.as_slice(),
            [RootlessEvent::InfoChanged { id, changed, .. },
             RootlessEvent::ParentChanged { id: id2, parent: Some(p) }]
                if *id == popup && *id2 == popup && *changed == InfoFields::TRANSIENT_FOR && *p == b),
        "{events:?}"
    );
}

/// A popup whose parent unmaps is re-parented by the same order (here: the
/// remaining topmost Managed), never left naming a retired id.
#[test]
fn popup_parent_unmapped_moves_the_popup() {
    let mut host = host_with_window();
    host.create_window(1, P, ROOT_WINDOW.0, (200, 200), (50, 40), 0, false);
    let a = host.map_and_reconcile(1, W);
    let b = host.map_and_reconcile(1, P);
    host.create_window(1, Q, ROOT_WINDOW.0, (210, 210), (10, 10), 0, true);
    let popup = host.map_and_reconcile(1, Q);
    host.unmap(1, P);
    assert_eq!(
        host.reconcile(),
        vec![
            RootlessEvent::Unmapped { id: b },
            RootlessEvent::ParentChanged {
                id: popup,
                parent: Some(a)
            }
        ]
    );
}

/// Ids never wrap into a panic.
#[test]
fn toplevel_id_counter_wraps_without_panic() {
    let mut host = RecordingHost::new();
    let r = host.state.rootless.as_mut().unwrap();
    r.next_id = std::num::NonZeroU64::MAX;
    assert_eq!(r.allocate_id().get(), u64::MAX);
    assert_eq!(r.allocate_id().get(), 1);
}
