//! Where a passive button grab's activating press is reported (GH #277).

use super::*;

/// GH #277 (probe V9, measured on Xorg in
/// `tools/vng-scenarios/key-grab-activation`): Xorg `ActivatePassiveGrab`
/// (dix/events.c) delivers the activating press with
/// `FixUpEventFromWindow(grab->window)` + `TryClientEvents` — no
/// `owner_events` walk. The grab client selects ButtonPress on the root
/// and on `bar` but not on the grab window `top`; the press is still
/// reported on `top`, child `bar`. The release, under the active grab,
/// takes the owner_events walk to `bar`.
#[test]
fn passive_grab_activating_press_skips_owner_events_walk() {
    use yserver_protocol::x11::ResourceId;

    let mut state = ServerState::new();
    let top = ResourceId(0x0020_0001);
    let bar = ResourceId(0x0020_0002);
    let mut peer = install_client(&mut state, 1);
    for (win, parent, x, y, w, h) in [
        (top, crate::resources::ROOT_WINDOW, 100, 100, 400, 300),
        (bar, top, 0, 250, 400, 50),
    ] {
        state.resources.create_window(
            ClientId(1),
            yserver_protocol::x11::CreateWindowRequest {
                depth: 24,
                window: win,
                parent,
                x,
                y,
                width: w,
                height: h,
                class: 1,
                visual: crate::resources::ROOT_VISUAL,
                ..Default::default()
            },
        );
        let _ = state.resources.map_window(win);
    }
    let masks = &mut state.clients.get_mut(&1).unwrap().event_masks;
    masks.insert(crate::resources::ROOT_WINDOW, 0x0000_000C);
    masks.insert(bar, 0x0000_000C);
    state.button_grabs.push(crate::server::PassiveButtonGrab {
        device_id: 0,
        owner: ClientId(1),
        grab_window: top,
        button: 1,
        modifiers: 0x8000,
        owner_events: true,
        event_mask: 0x0000_000C,
        pointer_mode: 1,
        keyboard_mode: 1,
        confine_to: ResourceId(0),
        via_xi2: false,
    });
    let mut xid_map = HostXidMap::new();
    xid_map.insert(0xCAFE_u32, crate::resources::ROOT_WINDOW);
    let mut backend = crate::backend::recording::RecordingBackend::default();
    let mut press = HostPointerEvent {
        origin: crate::core_loop::message::InputOrigin::XTest(4),
        kind: PointerEventKind::ButtonPress,
        host_xid: 0xCAFE,
        detail: 1,
        time: 0,
        root_x: 300,
        root_y: 375,
        event_x: 300,
        event_y: 375,
        state: 0,
        crossing_mode: 0,
        child: 0,
        raw_dx: 0,
        raw_dy: 0,
        tree_change: false,
    };
    let fields = |bytes: &[u8], code: u8| -> Vec<(u32, u32, i16, i16)> {
        bytes
            .chunks(32)
            .filter(|e| e[0] & 0x7f == code)
            .map(|e| {
                let u32_at = |at: usize| u32::from_le_bytes(e[at..at + 4].try_into().unwrap());
                let i16_at = |at: usize| i16::from_le_bytes([e[at], e[at + 1]]);
                (u32_at(12), u32_at(16), i16_at(24), i16_at(26))
            })
            .collect()
    };
    let _ = pointer_event_fanout_to_state(&mut state, &mut backend, &xid_map, press, true, false);
    assert_eq!(
        fields(&read_all_available(&mut peer), 4),
        [(top.0, bar.0, 200, 275)],
        "activating press: event=top child=bar ev=200,275 (Xorg probe V9)",
    );
    press.kind = PointerEventKind::ButtonRelease;
    press.state = 0x100;
    let _ = pointer_event_fanout_to_state(&mut state, &mut backend, &xid_map, press, true, false);
    assert_eq!(
        fields(&read_all_available(&mut peer), 5),
        [(bar.0, 0, 200, 25)],
        "release under the grab walks to bar (Xorg probe V9)",
    );
}
