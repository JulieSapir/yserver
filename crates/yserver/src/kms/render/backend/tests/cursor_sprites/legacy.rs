use super::support::{
    accepted_live_records, assert_no_cursor_resources, create_argb_cursor, finish_fixture,
    has_unretired_accepted_event, retire_pending_output,
};
use crate::{
    drm::{CursorDeviceCaps, atomic::AtomicCommitKind},
    kms::cursor_plane::{
        ops::{
            CursorBufferIo,
            recording::{CursorBufferLedger, RecordingCursorBufferIo},
        },
        topology::{
            CursorPathReason, CursorPlaneTestPlane, CursorPlaneTestSnapshot, CursorStatePath,
        },
    },
};
use std::{collections::HashSet, rc::Rc};
use yserver_core::backend::Backend;

const CURSOR_PROPERTIES: [&str; 12] = [
    "FB_ID",
    "CRTC_ID",
    "CRTC_X",
    "CRTC_Y",
    "CRTC_W",
    "CRTC_H",
    "SRC_X",
    "SRC_Y",
    "SRC_W",
    "SRC_H",
    "HOTSPOT_X",
    "HOTSPOT_Y",
];

fn snapshot(
    crtc: u32,
    missing_property: Option<&str>,
    cursor_caps: Option<CursorDeviceCaps>,
) -> CursorPlaneTestSnapshot {
    let property_names = CURSOR_PROPERTIES
        .into_iter()
        .filter(|name| Some(*name) != missing_property)
        .map(str::to_owned)
        .collect();
    CursorPlaneTestSnapshot {
        crtcs: vec![crtc],
        planes: vec![CursorPlaneTestPlane {
            handle: 21,
            possible_crtcs: vec![crtc],
            property_names,
        }],
        cursor_caps,
        has_buffer: true,
    }
}

fn install_snapshot(
    events: &mut super::super::commit_identity::support::AtomicEventFixture,
    snapshot: CursorPlaneTestSnapshot,
) -> Rc<CursorBufferLedger> {
    let (io, ledger) = RecordingCursorBufferIo::new(1);
    let buffer_io: Rc<dyn CursorBufferIo> = Rc::new(io);
    events
        .backend
        .platform
        .for_tests_install_cursor_plane_from_snapshot(0, snapshot, buffer_io, 64, 64);
    ledger
}

fn exercise_legacy_backend_scenario(
    scenario_name: &str,
    missing_property: Option<&str>,
    caps: Option<CursorDeviceCaps>,
    reason: CursorPathReason,
) -> bool {
    use super::super::commit_identity::recording_scene::RecordingSceneFixture;

    let Some(mut scene) = RecordingSceneFixture::new(1).expect("Legacy scene fixture") else {
        return false;
    };
    let output = &scene.events.backend.platform.outputs[0];
    let crtc = output.output.crtc;
    let device_key = output.key.device_key;
    let ledger = install_snapshot(
        &mut scene.events,
        snapshot(u32::from(crtc), missing_property, caps),
    );
    let legacy = Rc::new(super::support::RecordingLegacyCursor::default());
    let device = &mut scene.events.backend.platform.devices[0];
    assert_eq!(device.cursor.state_path(), CursorStatePath::Legacy);
    assert_eq!(
        device
            .cursor
            .plane
            .as_ref()
            .expect("installed Legacy plane")
            .state_path()
            .reason,
        reason,
        "{scenario_name} snapshot keeps its expected Legacy reason",
    );
    let plane = device
        .cursor
        .plane
        .as_mut()
        .expect("installed Legacy plane");
    let (_, crtc_x, crtc_y) = plane
        .atomic_position_properties(crtc)
        .expect("Legacy path retains master's atomic position properties");
    plane.for_tests_set_show_executor(legacy.clone());
    plane.for_tests_set_hide_executor(legacy.clone());
    assert_eq!(plane.sprites().for_tests_buffer_count(), 0);

    let cursor = create_argb_cursor(
        &mut scene.events.backend,
        (4, 4),
        [0x12, 0x34, 0x56, 0xff],
        (1, 2),
    );
    let cursor_id = cursor.as_raw();
    let root = scene.events.backend.core.window_id;
    Backend::define_cursor(&mut scene.events.backend, None, root, cursor_id)
        .expect("define the Legacy path cursor through Backend");
    Backend::warp_pointer_root(&mut scene.events.backend, &mut scene.events.state, 100, 90);
    let width = scene.events.backend.platform.fb_w;
    let height = scene.events.backend.platform.fb_h;
    Backend::clear_area(
        &mut scene.events.backend,
        None,
        root,
        0,
        None,
        0,
        0,
        width,
        height,
        (0, 0),
    )
    .expect("damage the Legacy cursor scene through Backend");

    let device_fd = scene.kms_fd(0);
    let mut delivered = HashSet::new();
    Backend::maybe_composite(&mut scene.events.backend).expect("compose the Legacy cursor scene");
    retire_pending_output(&mut scene.events, &mut delivered, 0, 1);
    assert_eq!(
        legacy.show_calls.get(),
        1,
        "{scenario_name} legacy show count"
    );
    assert_eq!(
        legacy.move_calls.get(),
        1,
        "{scenario_name} legacy move count"
    );
    assert_eq!(
        legacy.hide_calls.get(),
        0,
        "{scenario_name} legacy hide count"
    );

    for frame in 2..=4 {
        Backend::maybe_composite(&mut scene.events.backend)
            .expect("compose the Legacy position-only update");
        if has_unretired_accepted_event(&scene.events, &delivered, 0) {
            retire_pending_output(&mut scene.events, &mut delivered, 0, frame);
        }
        if scene
            .events
            .backend
            .platform
            .for_tests_atomic_cursor_pending_position(device_key, crtc)
            .is_none()
            && !has_unretired_accepted_event(&scene.events, &delivered, 0)
        {
            break;
        }
    }
    assert_eq!(
        scene
            .events
            .backend
            .platform
            .for_tests_atomic_cursor_pending_position(device_key, crtc),
        None,
        "{scenario_name} Legacy position update settles after its accepted event",
    );
    let output_records: Vec<_> = accepted_live_records(&scene.events.recorders[&device_fd])
        .into_iter()
        .filter(|record| record.events.iter().any(|event| event.crtc == crtc))
        .collect();
    assert_eq!(
        output_records
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        [AtomicCommitKind::Scene, AtomicCommitKind::CursorOnly],
        "{scenario_name} Legacy show stays on the ioctl path and only pointer movement uses an atomic commit",
    );
    let position_records: Vec<_> = output_records
        .iter()
        .filter(|record| record.kind == AtomicCommitKind::CursorOnly)
        .collect();
    assert!(
        !position_records.is_empty(),
        "{scenario_name} position commit exists"
    );
    for record in position_records {
        let properties: HashSet<_> = record
            .properties
            .iter()
            .map(|property| property.property)
            .collect();
        assert_eq!(
            properties,
            HashSet::from([crtc_x, crtc_y]),
            "{scenario_name} cursor-only commit contains only CRTC_X and CRTC_Y",
        );
    }
    assert_eq!(
        legacy.show_calls.get(),
        1,
        "{scenario_name} legacy show count"
    );
    assert_eq!(
        legacy.move_calls.get(),
        1,
        "{scenario_name} legacy move count"
    );
    assert_eq!(
        legacy.hide_calls.get(),
        0,
        "{scenario_name} legacy hide count"
    );
    assert_eq!(
        scene.events.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprites()
            .for_tests_buffer_count(),
        0,
        "{scenario_name} Legacy path never allocates sprites",
    );
    assert!(
        ledger.records().is_empty(),
        "{scenario_name} Legacy upload never AddFB2s a sprite",
    );

    Backend::define_cursor(&mut scene.events.backend, None, root, 0x7f10)
        .expect("restore the sticky root cursor through Backend");
    Backend::free_cursor(&mut scene.events.backend, None, cursor_id)
        .expect("free the Legacy test cursor through Backend");
    finish_fixture(&mut scene.events, &mut delivered);
    assert_eq!(
        scene.events.backend.platform.devices[0]
            .atomic_commit_sink
            .for_tests_pending_page_flip_entry_count(),
        0,
        "{scenario_name} leaves no pending page flip",
    );
    assert_eq!(ledger.records(), []);
    scene.finish_after_normal_drain();
    assert_eq!(
        ledger.end_state(),
        (0, 0, 0),
        "{scenario_name} ledger settles"
    );
    true
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: skip Legacy show for rejected-cap/no-caps snapshots, allocate a
// Legacy sprite pool, or issue AddFB2.
fn p2_legacy_path_device_uses_masters_ioctls_and_allocates_no_sprites() {
    use super::super::commit_identity::support::AtomicEventFixture;

    let mut headless = AtomicEventFixture::new(1);
    let crtc = u32::from(headless.backend.platform.outputs[0].output.crtc);
    let scenarios = [
        (
            "missing FB_ID",
            Some("FB_ID"),
            Some(CursorDeviceCaps::new(false, Some("amdgpu".to_owned()))),
            CursorPathReason::MissingProperty("FB_ID"),
        ),
        (
            "rejected virtio hotspot capability",
            None,
            Some(CursorDeviceCaps::new(false, Some("virtio_gpu".to_owned()))),
            CursorPathReason::HotspotSupportMissing,
        ),
        (
            "missing device caps",
            None,
            None,
            CursorPathReason::NoDeviceCaps,
        ),
    ];
    let mut headless_ledgers = Vec::new();
    for (_, missing_property, caps, reason) in scenarios.iter().cloned() {
        let ledger = install_snapshot(&mut headless, snapshot(crtc, missing_property, caps));
        let plane = headless.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .expect("headless Legacy cursor plane");
        assert_eq!(plane.state_path().path, CursorStatePath::Legacy);
        assert_eq!(plane.state_path().reason, reason);
        assert_eq!(plane.sprites().for_tests_buffer_count(), 0);
        assert!(
            ledger.records().is_empty(),
            "Legacy install did not allocate a sprite"
        );
        assert_eq!(ledger.end_state(), (0, 0, 0));
        headless_ledgers.push(ledger);
    }
    let mut headless_delivered = HashSet::new();
    finish_fixture(&mut headless, &mut headless_delivered);
    assert!(
        headless_ledgers
            .iter()
            .all(|ledger| ledger.records().is_empty())
    );

    for (scenario_index, (scenario_name, missing_property, caps, reason)) in
        scenarios.iter().cloned().enumerate()
    {
        if !exercise_legacy_backend_scenario(scenario_name, missing_property, caps, reason) {
            assert_eq!(
                scenario_index, 0,
                "Legacy scene fixture became unavailable after a scenario already ran",
            );
            return;
        }
    }
}

#[test]
// Mutation: give a no-plane device the Atomic path so its ENXIO fallback no longer latches.
fn p2_apple_dcp_topology_keeps_the_software_latch() {
    use super::super::commit_identity::support::AtomicEventFixture;

    let mut events = AtomicEventFixture::new(1);
    let crtc = events.backend.platform.outputs[0].output.crtc;
    let (io, ledger) = RecordingCursorBufferIo::new(1);
    let buffer_io: Rc<dyn CursorBufferIo> = Rc::new(io);
    events
        .backend
        .platform
        .for_tests_install_cursor_plane_from_snapshot(
            0,
            CursorPlaneTestSnapshot {
                crtcs: vec![u32::from(crtc)],
                planes: Vec::new(),
                cursor_caps: Some(CursorDeviceCaps::new(false, Some("apple".to_owned()))),
                has_buffer: true,
            },
            buffer_io,
            64,
            64,
        );

    let device_key = events.backend.platform.devices[0].key;
    let state = &events.backend.platform.devices[0].cursor;
    assert_eq!(state.state_path(), CursorStatePath::Legacy);
    assert_eq!(
        state.plane.as_ref().unwrap().state_path().reason,
        CursorPathReason::NoCursorPlane,
    );
    assert!(!state.atomic_cursor);
    assert!(events.backend.platform.cursor_plane_available());
    assert!(
        events
            .recorders
            .values()
            .all(|recorder| recorder.records().is_empty())
    );
    assert!(
        !events
            .backend
            .platform
            .for_tests_hw_cursor_disabled_for_device(device_key)
    );
    assert!(
        events
            .backend
            .platform
            .for_tests_note_unbound_cursor_failure(
                0,
                crtc,
                &std::io::Error::from_raw_os_error(libc::ENXIO),
            )
    );
    assert!(
        events
            .backend
            .platform
            .for_tests_hw_cursor_disabled_for_device(device_key)
    );
    assert!(!events.backend.platform.cursor_plane_available());
    assert!(
        events
            .recorders
            .values()
            .all(|recorder| recorder.records().is_empty())
    );
    assert_eq!(ledger.records(), []);
    assert_eq!(ledger.end_state(), (0, 0, 0));

    let mut delivered = HashSet::new();
    finish_fixture(&mut events, &mut delivered);
    assert_no_cursor_resources(&events.backend, Some(&ledger));
}

#[test]
// Mutation: accept the incomplete virtio_gpu plane despite its missing HOTSPOT_Y property.
fn p2_paravirtualized_with_accepted_cap_but_incomplete_plane_is_legacy() {
    use super::super::commit_identity::support::AtomicEventFixture;

    let mut events = AtomicEventFixture::new(1);
    let crtc = events.backend.platform.outputs[0].output.crtc;
    let ledger = install_snapshot(
        &mut events,
        snapshot(
            u32::from(crtc),
            Some("HOTSPOT_Y"),
            Some(CursorDeviceCaps::new(true, Some("virtio_gpu".to_owned()))),
        ),
    );
    let plane = events.backend.platform.devices[0]
        .cursor
        .plane
        .as_ref()
        .expect("assigned virtio cursor plane");
    assert_eq!(plane.state_path().path, CursorStatePath::Legacy);
    assert_eq!(
        plane.state_path().reason,
        CursorPathReason::HotspotSupportMissing
    );
    assert!(
        plane
            .for_tests_explicit_plane_assignment()
            .is_some_and(|assignment| assignment.contains_key(&crtc))
    );
    assert_eq!(plane.sprites().for_tests_buffer_count(), 0);
    assert!(ledger.records().is_empty());
    assert_eq!(ledger.end_state(), (0, 0, 0));

    let mut delivered = HashSet::new();
    finish_fixture(&mut events, &mut delivered);
    assert_no_cursor_resources(&events.backend, Some(&ledger));
}
