use super::support::{
    AtomicPathFixture, PathKind, accept_cursor_detach, accepted_live_records,
    applied_cursor_binding, assert_no_cursor_resources, compose_until_cursor_commit_where,
    create_argb_cursor, create_bitmap_cursor, cursor_properties_for_output, define_root_cursor,
    enter_held_direct, establish_hardware_cursor, finish_atomic_fixture, record_count,
    record_property, retire_cursor_record, sprite_users,
};
use crate::{
    drm::atomic::{AtomicCommitKind, tests_support::AtomicCommitRecord},
    kms::render::backend::tests::commit_identity::{
        recording_scene::{P1SceneRoute, RecordingSceneFixture},
        support::accepted_event_id,
    },
};
use std::{collections::HashSet, rc::Rc, time::Instant};
use yserver_core::backend::Backend;

fn atomic_fixture(test_name: &str) -> Option<AtomicPathFixture> {
    let fixture = AtomicPathFixture::new(PathKind::Physical)
        .expect("Atomic cursor scene fixture construction");
    if fixture.is_none() {
        println!("SKIP {test_name}: no Vulkan test device");
    }
    fixture
}

fn establish(
    fixture: &mut AtomicPathFixture,
    delivered: &mut HashSet<super::support::CompletionKey>,
    hotspot: (u16, u16),
) -> super::support::HardwareCursor {
    let cursor = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (8, 8),
        [0xff, 0xff, 0xff, 0xff],
        hotspot,
    );
    let hardware = establish_hardware_cursor(fixture, delivered, cursor, (120, 100));
    super::support::drain_unretired_output_events(fixture, delivered, hardware.output_idx);
    hardware
}

fn records(
    fixture: &AtomicPathFixture,
    hardware: super::support::HardwareCursor,
) -> Vec<AtomicCommitRecord> {
    accepted_live_records(&fixture.scene.events.recorders[&hardware.device_fd])
}

fn finish(
    mut fixture: AtomicPathFixture,
    delivered: &mut HashSet<super::support::CompletionKey>,
    hardware: super::support::HardwareCursor,
) {
    let detach = accept_cursor_detach(&mut fixture, delivered, hardware.output_idx);
    retire_cursor_record(&mut fixture, delivered, hardware.output_idx, &detach, 99);
    finish_atomic_fixture(fixture, delivered);
}

fn cursor_position_values(
    record: &AtomicCommitRecord,
    properties: crate::kms::cursor_plane::topology::CursorPlaneProperties,
) -> (i32, i32) {
    (
        i32::try_from(record_property(record, properties.crtc_x).expect("CRTC_X present"))
            .expect("CRTC_X fits i32"),
        i32::try_from(record_property(record, properties.crtc_y).expect("CRTC_Y present"))
            .expect("CRTC_Y fits i32"),
    )
}

fn assert_position_only(
    record: &AtomicCommitRecord,
    properties: crate::kms::cursor_plane::topology::CursorPlaneProperties,
    expected: (i32, i32),
) {
    let cursor_properties: Vec<_> = record
        .properties
        .iter()
        .filter(|update| update.object == properties.plane.into())
        .map(|update| update.property)
        .collect();
    assert!(
        !cursor_properties.is_empty(),
        "record carries cursor properties"
    );
    assert!(
        cursor_properties
            .iter()
            .all(|property| *property == properties.crtc_x || *property == properties.crtc_y),
        "cursor-plane commit carries properties other than CRTC_X/CRTC_Y: {cursor_properties:?}",
    );
    assert_eq!(cursor_position_values(record, properties), expected);
}

fn damage_root(fixture: &mut RecordingSceneFixture) {
    let root = fixture.events.backend.core.window_id;
    let width = fixture.events.backend.platform.fb_w;
    let height = fixture.events.backend.platform.fb_h;
    Backend::clear_area(
        &mut fixture.events.backend,
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
    .expect("damage root through Backend");
}

fn copied_atomic_fixture(test_name: &str) -> Option<AtomicPathFixture> {
    use crate::kms::cursor_plane::{
        ops::{CursorBufferIo, recording::RecordingCursorBufferIo},
        topology::{CursorPlaneTestPlane, CursorPlaneTestSnapshot},
    };

    let scene = RecordingSceneFixture::with_route(1, P1SceneRoute::CopiedSameDevice)
        .expect("copied recording scene fixture construction");
    let Some(mut scene) = scene else {
        println!("SKIP {test_name}: no Vulkan test device");
        return None;
    };
    let output = &scene.events.backend.platform.outputs[0];
    let device_idx = scene
        .events
        .backend
        .platform
        .devices
        .iter()
        .position(|device| device.key == output.key.device_key)
        .expect("copied scene output has an owning KMS device");
    let crtc = output.output.crtc;
    let (io, ledger) = RecordingCursorBufferIo::new(256);
    let buffer_io: Rc<dyn CursorBufferIo> = Rc::new(io);
    scene
        .events
        .backend
        .platform
        .for_tests_install_cursor_plane_from_snapshot(
            device_idx,
            CursorPlaneTestSnapshot {
                crtcs: vec![u32::from(crtc)],
                planes: vec![CursorPlaneTestPlane {
                    handle: 21,
                    possible_crtcs: vec![u32::from(crtc)],
                    property_names: [
                        "FB_ID", "CRTC_ID", "CRTC_X", "CRTC_Y", "CRTC_W", "CRTC_H", "SRC_X",
                        "SRC_Y", "SRC_W", "SRC_H",
                    ]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                }],
                cursor_caps: Some(crate::drm::CursorDeviceCaps::new(
                    false,
                    Some("amdgpu".to_owned()),
                )),
                has_buffer: true,
            },
            buffer_io,
            64,
            64,
        );
    let legacy = Rc::new(super::support::RecordingLegacyCursor::default());
    let plane = scene.events.backend.platform.devices[device_idx]
        .cursor
        .plane
        .as_mut()
        .expect("installed copied-route cursor plane");
    plane.for_tests_set_show_executor(legacy.clone());
    plane.for_tests_set_hide_executor(legacy.clone());
    assert_eq!(
        scene.events.backend.platform.devices[device_idx]
            .cursor
            .state_path(),
        crate::kms::cursor_plane::topology::CursorStatePath::Atomic
    );
    Some(AtomicPathFixture {
        scene,
        ledger,
        legacy,
    })
}

fn complete_copied_render(fixture: &mut AtomicPathFixture) {
    let job_ids = fixture.scene.pending_render_job_ids();
    if !job_ids.is_empty() {
        // The readiness fixture controls when Backend sees a render completion;
        // it does not stand in for GPU completion. Settle the submitted work
        // before releasing those completion notifications.
        fixture.scene.events.wait_for_submitted_gpu_work();
    }
    for job_id in job_ids {
        fixture
            .scene
            .readiness
            .as_ref()
            .expect("copied route has render-completion readiness")
            .release_ready_job(job_id);
    }
    for _ in 0..16 {
        Backend::on_scanout_render_completion(
            &mut fixture.scene.events.backend,
            &mut fixture.scene.events.state,
        );
        // The copied sink handoff may submit queue work while the source
        // completion is consumed. The test injects flip events after this
        // helper returns, so settle that handoff before permitting retirement.
        fixture.scene.events.wait_for_submitted_gpu_work();
        if fixture.scene.pending_render_job_ids().is_empty() {
            return;
        }
        Backend::before_block(&mut fixture.scene.events.backend);
    }
    assert!(fixture.scene.pending_render_job_ids().is_empty());
}

fn establish_copied_cursor(
    fixture: &mut AtomicPathFixture,
    delivered: &mut HashSet<super::support::CompletionKey>,
) -> super::support::HardwareCursor {
    let cursor = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (8, 8),
        [0xff, 0xff, 0xff, 0xff],
        (0, 0),
    );
    define_root_cursor(&mut fixture.scene.events.backend, cursor);
    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        120,
        100,
    );
    damage_root(&mut fixture.scene);
    let device_fd = fixture.scene.kms_fd(0);
    let crtc = fixture.scene.events.backend.platform.outputs[0].output.crtc;
    for frame in 1..=8 {
        Backend::before_block(&mut fixture.scene.events.backend);
        Backend::maybe_composite(&mut fixture.scene.events.backend)
            .expect("compose copied cursor scene through Backend");
        complete_copied_render(fixture);
        if super::support::has_unretired_accepted_event(&fixture.scene.events, delivered, 0) {
            super::support::retire_pending_output(
                &mut fixture.scene.events,
                delivered,
                0,
                u32::try_from(frame).expect("test frame fits"),
            );
        }
        let Some(binding) = applied_cursor_binding(&fixture.scene.events.backend, 0) else {
            continue;
        };
        if !fixture
            .scene
            .events
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
        {
            let properties = cursor_properties_for_output(&fixture.scene.events.backend, 0);
            let attach_commit_id =
                accepted_live_records(&fixture.scene.events.recorders[&device_fd])
                    .into_iter()
                    .find(|record| {
                        record.kind == AtomicCommitKind::CursorOnly
                            && record_property(record, properties.fb_id)
                                == Some(u64::from(binding.framebuffer))
                    })
                    .and_then(|record| accepted_event_id(&record))
                    .expect("accepted copied-route cursor attach has an event ID");
            let hardware = super::support::HardwareCursor {
                output_idx: 0,
                device_fd,
                crtc,
                attach_commit_id,
            };
            super::support::drain_unretired_output_events(fixture, delivered, 0);
            return hardware;
        }
    }
    panic!("copied-route hardware cursor was not established");
}

fn finish_copied_fixture(
    mut fixture: AtomicPathFixture,
    delivered: &mut HashSet<super::support::CompletionKey>,
) {
    Backend::set_cursor_hidden(&mut fixture.scene.events.backend, true);
    damage_root(&mut fixture.scene);
    for frame in 60..76 {
        Backend::before_block(&mut fixture.scene.events.backend);
        Backend::maybe_composite(&mut fixture.scene.events.backend)
            .expect("compose copied cursor detach through Backend");
        complete_copied_render(&mut fixture);
        if super::support::has_unretired_accepted_event(&fixture.scene.events, delivered, 0) {
            super::support::retire_pending_output(
                &mut fixture.scene.events,
                delivered,
                0,
                u32::try_from(frame).expect("test frame fits"),
            );
        }
        if applied_cursor_binding(&fixture.scene.events.backend, 0).is_none()
            && !fixture
                .scene
                .events
                .backend
                .platform
                .cursor_commit_in_flight_for_output(0)
            && fixture.scene.pending_render_job_ids().is_empty()
            && !super::support::has_unretired_accepted_event(&fixture.scene.events, delivered, 0)
        {
            break;
        }
    }
    assert!(applied_cursor_binding(&fixture.scene.events.backend, 0).is_none());
    assert!(fixture.scene.pending_render_job_ids().is_empty());
    finish_atomic_fixture(fixture, delivered);
}

fn reject_position_carrying_scene(
    fixture: &mut AtomicPathFixture,
    hardware: super::support::HardwareCursor,
    blocking_error: Option<i32>,
) -> (AtomicCommitRecord, AtomicCommitRecord) {
    let recorder = Rc::clone(&fixture.scene.events.recorders[&hardware.device_fd]);
    let start = record_count(fixture, hardware.device_fd);
    damage_root(&mut fixture.scene);
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
    recorder.script_result(Ok(()));
    if let Some(error) = blocking_error {
        recorder.script_result(Err(std::io::Error::from_raw_os_error(error)));
    }
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("scene rejection is recovered through the Backend");
    let recent = recorder.records();
    let rejected = recent
        .iter()
        .skip(start)
        .find(|record| {
            record.kind == AtomicCommitKind::SceneWithCursor
                && !record
                    .flags
                    .contains(::drm::control::AtomicCommitFlags::TEST_ONLY)
                && record.result != crate::drm::atomic::tests_support::ScriptedResult::Ok
        })
        .cloned()
        .expect("scene-with-cursor commit rejected");
    let detach = recent
        .into_iter()
        .skip(start)
        .find(|record| {
            record.kind == AtomicCommitKind::CursorOnly
                && record.flags.is_empty()
                && record.events.is_empty()
        })
        .expect("rejected scene rolls back with a blocking cursor-plane detach");
    (rejected, detach)
}

#[test]
#[ignore = "requires the lavapipe GPU scene fixture"]
// Mutation: restore plane.is_visible_on in the scheduler or submit gate.
fn p2_idle_pointer_motion_wakes_the_scheduler_and_sends_one_cursor_only_move() {
    let Some(mut fixture) =
        atomic_fixture("p2_idle_pointer_motion_wakes_the_scheduler_and_sends_one_cursor_only_move")
    else {
        return;
    };
    let mut delivered = HashSet::new();
    let hardware = establish(&mut fixture, &mut delivered, (0, 0));
    let properties =
        cursor_properties_for_output(&fixture.scene.events.backend, hardware.output_idx);
    let start = records(&fixture, hardware).len();

    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        137,
        116,
    );
    assert!(
        Backend::next_wakeup(&fixture.scene.events.backend)
            .is_some_and(|wakeup| wakeup <= Instant::now()),
        "an unsent Atomic cursor position schedules an immediate wakeup",
    );
    Backend::before_block(&mut fixture.scene.events.backend);
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("submit idle cursor position through Backend");

    let submitted: Vec<_> = records(&fixture, hardware)
        .into_iter()
        .skip(start)
        .collect();
    assert_eq!(submitted.len(), 1);
    assert_eq!(submitted[0].kind, AtomicCommitKind::CursorOnly);
    assert_position_only(&submitted[0], properties, (137, 116));
    retire_cursor_record(
        &mut fixture,
        &mut delivered,
        hardware.output_idx,
        &submitted[0],
        40,
    );
    finish(fixture, &mut delivered, hardware);
}

#[test]
#[ignore = "requires the lavapipe GPU scene fixture"]
// Mutation: restore is_visible_on in cursor_crtc_membership_dirty.
fn p2_motion_inside_one_output_does_not_set_membership_dirty() {
    let Some(mut fixture) =
        atomic_fixture("p2_motion_inside_one_output_does_not_set_membership_dirty")
    else {
        return;
    };
    let mut delivered = HashSet::new();
    let hardware = establish(&mut fixture, &mut delivered, (0, 0));
    let start = records(&fixture, hardware).len();
    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        137,
        116,
    );
    assert!(
        !fixture
            .scene
            .events
            .backend
            .platform
            .cursor_crtc_membership_dirty(137, 116, 0, 0, 8, 8),
        "an already-bound CRTC remains a member while the cursor stays inside it",
    );
    assert!(
        !fixture.scene.events.backend.for_tests_scene_wants_scene(),
        "in-output cursor motion does not request a scene recomposition",
    );
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("submit in-output cursor position through Backend");
    let submitted = records(&fixture, hardware)
        .into_iter()
        .skip(start)
        .find(|record| record.kind == AtomicCommitKind::CursorOnly)
        .expect("in-output motion is sent as one cursor-only position update");
    retire_cursor_record(
        &mut fixture,
        &mut delivered,
        hardware.output_idx,
        &submitted,
        41,
    );
    finish(fixture, &mut delivered, hardware);
}

#[test]
#[ignore = "requires the lavapipe GPU scene fixture"]
// Mutation: retain the pending P2 position and leave the latest cache at P2.
fn p2_returning_to_the_applied_position_clears_pending_and_latest() {
    let Some(mut fixture) =
        atomic_fixture("p2_returning_to_the_applied_position_clears_pending_and_latest")
    else {
        return;
    };
    let mut delivered = HashSet::new();
    let hardware = establish(&mut fixture, &mut delivered, (0, 0));
    let properties =
        cursor_properties_for_output(&fixture.scene.events.backend, hardware.output_idx);
    let offset = records(&fixture, hardware).len();

    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        130,
        110,
    );
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("submit P1 cursor-only move");
    let p1 = records(&fixture, hardware)
        .into_iter()
        .skip(offset)
        .find(|record| record.kind == AtomicCommitKind::CursorOnly)
        .expect("P1 cursor move accepted");
    assert_position_only(&p1, properties, (130, 110));
    let p1_binding = applied_cursor_binding(&fixture.scene.events.backend, hardware.output_idx)
        .expect("accepted cursor move retains applied binding");
    assert_eq!((p1_binding.x, p1_binding.y), (130, 110));

    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        145,
        125,
    );
    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        130,
        110,
    );
    let pending = fixture
        .scene
        .events
        .backend
        .platform
        .for_tests_atomic_cursor_pending_position(
            fixture.scene.events.backend.platform.outputs[hardware.output_idx]
                .key
                .device_key,
            hardware.crtc,
        );
    assert!(
        pending.is_none(),
        "returning to applied P1 removes pending P2"
    );
    retire_cursor_record(&mut fixture, &mut delivered, hardware.output_idx, &p1, 42);

    let after_event = records(&fixture, hardware);
    assert!(
        !after_event.iter().skip(offset + 1).any(|record| {
            record.kind == AtomicCommitKind::CursorOnly
                && cursor_position_values(record, properties) == (145, 125)
        }),
        "P2 is never sent after the pointer returned to P1",
    );
    let root = fixture.scene.events.backend.core.window_id;
    let width = fixture.scene.events.backend.platform.fb_w;
    let height = fixture.scene.events.backend.platform.fb_h;
    Backend::clear_area(
        &mut fixture.scene.events.backend,
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
    .expect("damage scene through Backend");
    let scene_start = records(&fixture, hardware).len();
    Backend::maybe_composite(&mut fixture.scene.events.backend).expect("submit scene at P1");
    let scene = records(&fixture, hardware)
        .into_iter()
        .skip(scene_start)
        .find(|record| record.kind == AtomicCommitKind::SceneWithCursor)
        .expect("next scene commit carries the applied P1 position");
    assert_position_only(&scene, properties, (130, 110));
    retire_cursor_record(
        &mut fixture,
        &mut delivered,
        hardware.output_idx,
        &scene,
        43,
    );
    let idle_start = records(&fixture, hardware).len();
    Backend::before_block(&mut fixture.scene.events.backend);
    Backend::maybe_composite(&mut fixture.scene.events.backend).expect("idle scheduling pass");
    assert_eq!(records(&fixture, hardware).len(), idle_start);
    assert!(
        Backend::next_wakeup(&fixture.scene.events.backend)
            .is_none_or(|wakeup| wakeup > Instant::now()),
        "stable applied position has no due cursor wakeup",
    );
    finish(fixture, &mut delivered, hardware);
}

#[test]
#[ignore = "requires the lavapipe GPU scene fixture"]
// Mutation: use the staged effective cursor's hotspot before its replacement lands.
fn p2_staged_replacement_hotspot_does_not_shift_the_applied_plane() {
    let Some(mut fixture) =
        atomic_fixture("p2_staged_replacement_hotspot_does_not_shift_the_applied_plane")
    else {
        return;
    };
    let mut delivered = HashSet::new();
    let hardware = establish(&mut fixture, &mut delivered, (0, 0));
    let properties = cursor_properties_for_output(&fixture.scene.events.backend, 0);
    let binding_a = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();
    let cursor_b = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (8, 8),
        [0x11, 0x22, 0x33, 0xff],
        (8, 8),
    );
    define_root_cursor(&mut fixture.scene.events.backend, cursor_b);

    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        140,
        120,
    );
    let latest = fixture
        .scene
        .events
        .backend
        .platform
        .cursor_plane_atomic_positions_for_outputs(&[0]);
    assert_eq!(latest.len(), 1);
    assert_eq!((latest[0].x, latest[0].y), (140, 120));
    assert_eq!(
        (binding_a.hot_x, binding_a.hot_y),
        (0, 0),
        "the old binding remains the position basis until replacement acceptance",
    );

    let start = record_count(&fixture, hardware.device_fd);
    let root = fixture.scene.events.backend.core.window_id;
    let width = fixture.scene.events.backend.platform.fb_w;
    let height = fixture.scene.events.backend.platform.fb_h;
    Backend::clear_area(
        &mut fixture.scene.events.backend,
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
    .expect("damage root before staged replacement");
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("submit scene position under applied hotspot");
    let scene = records(&fixture, hardware)
        .into_iter()
        .skip(start)
        .find(|record| {
            matches!(
                record.kind,
                AtomicCommitKind::Scene | AtomicCommitKind::SceneWithCursor
            ) && record_property(record, properties.crtc_x).is_some()
        })
        .expect("scene frame carries position before the replacement lands");
    assert_position_only(&scene, properties, (140, 120));
    retire_cursor_record(&mut fixture, &mut delivered, 0, &scene, 46);

    let replacement = compose_until_cursor_commit_where(
        &mut fixture,
        &mut delivered,
        0,
        start,
        properties.fb_id,
        |record| record_property(record, properties.fb_id) != Some(binding_a.framebuffer.into()),
    );
    assert_eq!(
        record_property(&replacement, properties.crtc_x),
        Some(132),
        "replacement carries x after its hotspot changes the position from the scene value",
    );
    assert_eq!(record_property(&replacement, properties.crtc_y), Some(112));
    let binding_b = applied_cursor_binding(&fixture.scene.events.backend, 0)
        .expect("replacement binding accepted");
    assert_eq!((binding_b.hot_x, binding_b.hot_y), (8, 8));
    retire_cursor_record(&mut fixture, &mut delivered, 0, &replacement, 47);

    let move_start = records(&fixture, hardware).len();
    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        150,
        130,
    );
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("submit motion after replacement acceptance");
    let moved = records(&fixture, hardware)
        .into_iter()
        .skip(move_start)
        .find(|record| record.kind == AtomicCommitKind::CursorOnly)
        .expect("post-replacement motion uses cursor-only commit");
    assert_position_only(&moved, properties, (142, 122));
    retire_cursor_record(&mut fixture, &mut delivered, 0, &moved, 48);
    finish(fixture, &mut delivered, hardware);
}

#[test]
#[ignore = "requires the lavapipe GPU scene fixture"]
// Mutation: skip applied x/y upkeep after a scene position; the equal replacement must omit x/y.
fn p2_accepted_scene_position_updates_the_applied_state() {
    let Some(mut fixture) = atomic_fixture("p2_accepted_scene_position_updates_the_applied_state")
    else {
        return;
    };
    let mut delivered = HashSet::new();
    let hardware = establish(&mut fixture, &mut delivered, (0, 0));
    let properties = cursor_properties_for_output(&fixture.scene.events.backend, 0);
    let binding_a = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();
    let cursor_b = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (8, 8),
        [0xff, 0x80, 0x80, 0xff],
        (0, 0),
    );
    define_root_cursor(&mut fixture.scene.events.backend, cursor_b);
    let start = records(&fixture, hardware).len();
    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        130,
        110,
    );
    let root = fixture.scene.events.backend.core.window_id;
    let width = fixture.scene.events.backend.platform.fb_w;
    let height = fixture.scene.events.backend.platform.fb_h;
    Backend::clear_area(
        &mut fixture.scene.events.backend,
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
    .expect("damage root before the accepted scene position");
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("submit scene frame with the latest cursor position");
    let scene = records(&fixture, hardware)
        .into_iter()
        .skip(start)
        .find(|record| {
            matches!(
                record.kind,
                AtomicCommitKind::Scene | AtomicCommitKind::SceneWithCursor
            ) && record_property(record, properties.crtc_x).is_some()
        })
        .expect("accepted scene commit carries CRTC_X/Y");
    assert_position_only(&scene, properties, (130, 110));
    let applied = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();
    retire_cursor_record(&mut fixture, &mut delivered, 0, &scene, 49);

    let equal_replacement = compose_until_cursor_commit_where(
        &mut fixture,
        &mut delivered,
        0,
        start,
        properties.fb_id,
        |record| record_property(record, properties.fb_id) != Some(binding_a.framebuffer.into()),
    );
    assert_eq!(record_property(&equal_replacement, properties.crtc_x), None);
    assert_eq!(record_property(&equal_replacement, properties.crtc_y), None);
    let binding_b = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();
    assert_eq!((binding_b.x, binding_b.y), (130, 110));
    assert_eq!((applied.x, applied.y), (130, 110));
    retire_cursor_record(&mut fixture, &mut delivered, 0, &equal_replacement, 50);

    let cursor_c = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (8, 8),
        [0x80, 0xff, 0x80, 0xff],
        (8, 8),
    );
    define_root_cursor(&mut fixture.scene.events.backend, cursor_c);
    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        137,
        115,
    );
    let root = fixture.scene.events.backend.core.window_id;
    let width = fixture.scene.events.backend.platform.fb_w;
    let height = fixture.scene.events.backend.platform.fb_h;
    Backend::clear_area(
        &mut fixture.scene.events.backend,
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
    .expect("damage root before changed-position replacement");
    let changed_scene_start = records(&fixture, hardware).len();
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("submit a scene position before changed-position replacement");
    let changed_scene = records(&fixture, hardware)
        .into_iter()
        .skip(changed_scene_start)
        .find(|record| {
            matches!(
                record.kind,
                AtomicCommitKind::Scene | AtomicCommitKind::SceneWithCursor
            ) && record_property(record, properties.crtc_x).is_some()
        })
        .expect("scene carries the position under the applied hotspot");
    assert_position_only(&changed_scene, properties, (137, 115));
    retire_cursor_record(&mut fixture, &mut delivered, 0, &changed_scene, 51);
    let changed_replacement = compose_until_cursor_commit_where(
        &mut fixture,
        &mut delivered,
        0,
        changed_scene_start,
        properties.fb_id,
        |record| record_property(record, properties.fb_id) != Some(binding_b.framebuffer.into()),
    );
    assert_eq!(
        record_property(&changed_replacement, properties.crtc_x),
        Some(129)
    );
    assert_eq!(
        record_property(&changed_replacement, properties.crtc_y),
        Some(107)
    );
    retire_cursor_record(&mut fixture, &mut delivered, 0, &changed_replacement, 52);
    finish(fixture, &mut delivered, hardware);
}

#[test]
#[ignore = "requires the lavapipe GPU scene fixture"]
// Mutation: append FB_ID to a position carrier or use the staged image hotspot.
fn p2_scene_direct_copied_and_unflip_commits_carry_only_the_applied_position() {
    {
        let Some(mut fixture) = atomic_fixture(
            "p2_scene_direct_copied_and_unflip_commits_carry_only_the_applied_position",
        ) else {
            return;
        };
        let mut delivered = HashSet::new();
        let hardware = establish(&mut fixture, &mut delivered, (0, 0));
        let properties = cursor_properties_for_output(&fixture.scene.events.backend, 0);
        let binding = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();
        let users = sprite_users(&fixture.scene.events.backend, 0, binding.lease);
        let start = records(&fixture, hardware).len();
        damage_root(&mut fixture.scene);
        Backend::maybe_composite(&mut fixture.scene.events.backend)
            .expect("submit shared scene position carrier");
        let scene = records(&fixture, hardware)
            .into_iter()
            .skip(start)
            .find(|record| {
                matches!(
                    record.kind,
                    AtomicCommitKind::Scene | AtomicCommitKind::SceneWithCursor
                ) && record_property(record, properties.crtc_x).is_some()
            })
            .expect("shared scene carries cursor position");
        assert_position_only(&scene, properties, (binding.x, binding.y));
        assert_eq!(
            sprite_users(&fixture.scene.events.backend, 0, binding.lease),
            users
        );
        retire_cursor_record(&mut fixture, &mut delivered, 0, &scene, 51);
        finish(fixture, &mut delivered, hardware);
    }

    {
        let Some(mut fixture) = atomic_fixture(
            "p2_scene_direct_copied_and_unflip_commits_carry_only_the_applied_position",
        ) else {
            return;
        };
        let mut delivered = HashSet::new();
        let hardware = establish(&mut fixture, &mut delivered, (0, 0));
        let properties = cursor_properties_for_output(&fixture.scene.events.backend, 0);
        let binding = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();
        let users = sprite_users(&fixture.scene.events.backend, 0, binding.lease);
        let start = records(&fixture, hardware).len();
        let direct = enter_held_direct(&mut fixture, &mut delivered, 7)
            .expect("admit direct scanout through Backend");
        let direct_record = records(&fixture, hardware)
            .into_iter()
            .skip(start)
            .find(|record| {
                record.kind != AtomicCommitKind::CursorOnly
                    && accepted_event_id(record).is_some()
                    && record_property(record, properties.crtc_x).is_some()
            })
            .expect("direct record carries the applied cursor position");
        assert_position_only(&direct_record, properties, (binding.x, binding.y));
        assert_eq!(
            sprite_users(&fixture.scene.events.backend, 0, binding.lease),
            users
        );

        let mut successor = direct.candidate;
        successor.present_id += 1;
        successor.valid_region_xid = 0x55;
        let mut successor_event = direct.event.clone();
        successor_event.present_id = successor.present_id;
        successor_event.serial = successor_event.serial.wrapping_add(1);
        assert!(
            !Backend::try_present_direct(
                &mut fixture.scene.events.backend,
                successor,
                successor_event,
            )
            .expect("ineligible Present asks for composed unflip"),
        );
        let unflip_start = records(&fixture, hardware).len();
        Backend::maybe_composite(&mut fixture.scene.events.backend)
            .expect("submit composed unflip through Backend");
        let unflip = records(&fixture, hardware)
            .into_iter()
            .skip(unflip_start)
            .find(|record| {
                record.kind != AtomicCommitKind::CursorOnly
                    && accepted_event_id(record).is_some()
                    && record_property(record, properties.crtc_x).is_some()
            })
            .expect("composed unflip carries the applied cursor position");
        assert_position_only(&unflip, properties, (binding.x, binding.y));
        assert_eq!(
            sprite_users(&fixture.scene.events.backend, 0, binding.lease),
            users
        );
        retire_cursor_record(&mut fixture, &mut delivered, 0, &unflip, 52);
        finish(fixture, &mut delivered, hardware);
    }

    {
        let Some(mut fixture) = copied_atomic_fixture(
            "p2_scene_direct_copied_and_unflip_commits_carry_only_the_applied_position",
        ) else {
            return;
        };
        let mut delivered = HashSet::new();
        let hardware = establish_copied_cursor(&mut fixture, &mut delivered);
        let properties = cursor_properties_for_output(&fixture.scene.events.backend, 0);
        let binding = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();
        let users = sprite_users(&fixture.scene.events.backend, 0, binding.lease);
        let start = records(&fixture, hardware).len();
        damage_root(&mut fixture.scene);
        Backend::maybe_composite(&mut fixture.scene.events.backend)
            .expect("submit copied source render through Backend");
        complete_copied_render(&mut fixture);
        let copied = records(&fixture, hardware)
            .into_iter()
            .skip(start)
            .find(|record| {
                matches!(
                    record.kind,
                    AtomicCommitKind::Scene | AtomicCommitKind::SceneWithCursor
                ) && accepted_event_id(record).is_some()
                    && record_property(record, properties.crtc_x).is_some()
            })
            .expect("copied sink commit carries cursor position");
        assert_position_only(&copied, properties, (binding.x, binding.y));
        assert_eq!(
            sprite_users(&fixture.scene.events.backend, 0, binding.lease),
            users
        );
        retire_cursor_record(&mut fixture, &mut delivered, 0, &copied, 53);
        finish_copied_fixture(fixture, &mut delivered);
    }
}

#[test]
#[ignore = "requires the lavapipe GPU scene fixture"]
// Mutation: use the legacy hide or release the binding before blocking detach succeeds.
fn p2_rollback_detaches_the_applied_plane_not_crtc_cursor() {
    for blocking_error in [None, Some(libc::EIO)] {
        let Some(mut fixture) =
            atomic_fixture("p2_rollback_detaches_the_applied_plane_not_crtc_cursor")
        else {
            return;
        };
        let mut delivered = HashSet::new();
        let hardware = establish(&mut fixture, &mut delivered, (0, 0));
        let binding = applied_cursor_binding(&fixture.scene.events.backend, 0)
            .expect("cursor binding is applied before scene rejection");
        let properties = cursor_properties_for_output(&fixture.scene.events.backend, 0);
        let (rejected, detach) =
            reject_position_carrying_scene(&mut fixture, hardware, blocking_error);
        assert_eq!(rejected.kind, AtomicCommitKind::SceneWithCursor);
        assert_ne!(
            rejected.result,
            crate::drm::atomic::tests_support::ScriptedResult::Ok
        );
        assert_eq!(detach.kind, AtomicCommitKind::CursorOnly);
        assert!(detach.flags.is_empty());
        assert!(detach.events.is_empty(), "blocking rollback owes no event");
        assert_eq!(detach.properties.len(), 2);
        assert!(
            detach
                .properties
                .iter()
                .all(|update| update.object == properties.plane.into())
        );
        assert_eq!(record_property(&detach, properties.fb_id), Some(0));
        assert_eq!(record_property(&detach, properties.crtc_id), Some(0));
        assert_eq!(fixture.legacy.hide_calls.get(), 0);
        if blocking_error.is_some() {
            assert_ne!(
                detach.result,
                crate::drm::atomic::tests_support::ScriptedResult::Ok
            );
            assert_eq!(
                applied_cursor_binding(&fixture.scene.events.backend, 0),
                Some(binding),
                "failed blocking detach retains the applied binding",
            );
            assert_eq!(
                sprite_users(&fixture.scene.events.backend, 0, binding.lease),
                Some(1)
            );
            assert_eq!(
                fixture
                    .scene
                    .events
                    .backend
                    .platform
                    .for_tests_cursor_hide_recovery_attempts(
                        fixture.scene.events.backend.platform.outputs[0]
                            .key
                            .device_key,
                        hardware.crtc,
                    ),
                Some(0),
            );
            Backend::warp_pointer_root(
                &mut fixture.scene.events.backend,
                &mut fixture.scene.events.state,
                141,
                121,
            );
            assert_eq!(
                applied_cursor_binding(&fixture.scene.events.backend, 0),
                None
            );
            assert_eq!(
                sprite_users(&fixture.scene.events.backend, 0, binding.lease),
                Some(0)
            );
        } else {
            assert_eq!(
                detach.result,
                crate::drm::atomic::tests_support::ScriptedResult::Ok
            );
            assert_eq!(
                applied_cursor_binding(&fixture.scene.events.backend, 0),
                None
            );
            assert_eq!(
                sprite_users(&fixture.scene.events.backend, 0, binding.lease),
                Some(0)
            );
        }
        finish_atomic_fixture(fixture, &mut delivered);
    }
}

#[test]
#[ignore = "requires the lavapipe GPU scene fixture"]
// Mutation: leave applied after successful recovery or release it when the three-attempt latch fires.
fn p2_motion_led_hide_recovery_settles_the_applied_state() {
    {
        let Some(mut fixture) =
            atomic_fixture("p2_motion_led_hide_recovery_settles_the_applied_state")
        else {
            return;
        };
        let mut delivered = HashSet::new();
        let hardware = establish(&mut fixture, &mut delivered, (0, 0));
        let binding = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();
        let (_, failed_detach) =
            reject_position_carrying_scene(&mut fixture, hardware, Some(libc::EIO));
        assert_ne!(
            failed_detach.result,
            crate::drm::atomic::tests_support::ScriptedResult::Ok
        );
        Backend::warp_pointer_root(
            &mut fixture.scene.events.backend,
            &mut fixture.scene.events.state,
            140,
            120,
        );
        assert_eq!(
            applied_cursor_binding(&fixture.scene.events.backend, 0),
            None
        );
        assert!(
            !fixture
                .scene
                .events
                .backend
                .platform
                .cursor_plane_visible_for_output(0)
        );
        assert_eq!(
            sprite_users(&fixture.scene.events.backend, 0, binding.lease),
            Some(0)
        );
        assert!(
            fixture
                .scene
                .events
                .backend
                .platform
                .for_tests_cursor_fallback_remaining(
                    fixture.scene.events.backend.platform.outputs[0]
                        .key
                        .device_key,
                    hardware.crtc,
                )
                .is_some()
        );
        finish_atomic_fixture(fixture, &mut delivered);
    }

    {
        let Some(mut fixture) =
            atomic_fixture("p2_motion_led_hide_recovery_settles_the_applied_state")
        else {
            return;
        };
        let mut delivered = HashSet::new();
        let hardware = establish(&mut fixture, &mut delivered, (0, 0));
        let binding = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();
        let recorder = Rc::clone(&fixture.scene.events.recorders[&hardware.device_fd]);
        recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
        recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
        let rejection_start = record_count(&fixture, hardware.device_fd);
        Backend::warp_pointer_root(
            &mut fixture.scene.events.backend,
            &mut fixture.scene.events.state,
            130,
            110,
        );
        Backend::maybe_composite(&mut fixture.scene.events.backend)
            .expect("cursor-only rejection runs through Backend");
        let failed_move = recorder
            .records()
            .into_iter()
            .skip(rejection_start)
            .find(|record| {
                record.kind == AtomicCommitKind::CursorOnly
                    && record.result != crate::drm::atomic::tests_support::ScriptedResult::Ok
                    && !record.flags.is_empty()
            })
            .expect("cursor-only move was rejected");
        assert_eq!(
            failed_move.result,
            crate::drm::atomic::tests_support::ScriptedResult::Error {
                kind: std::io::ErrorKind::InvalidInput,
                raw_os_error: Some(libc::EINVAL),
            }
        );
        assert_eq!(
            applied_cursor_binding(&fixture.scene.events.backend, 0),
            Some(binding)
        );
        assert_eq!(
            fixture
                .scene
                .events
                .backend
                .platform
                .for_tests_cursor_hide_recovery_attempts(
                    fixture.scene.events.backend.platform.outputs[0]
                        .key
                        .device_key,
                    hardware.crtc,
                ),
            Some(0),
        );
        for (x, y) in [(140, 120), (141, 121), (142, 122)] {
            recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
            Backend::warp_pointer_root(
                &mut fixture.scene.events.backend,
                &mut fixture.scene.events.state,
                x,
                y,
            );
        }
        assert!(
            fixture
                .scene
                .events
                .backend
                .platform
                .for_tests_cursor_permanently_disabled(
                    fixture.scene.events.backend.platform.outputs[0]
                        .key
                        .device_key,
                )
        );
        assert_eq!(
            applied_cursor_binding(&fixture.scene.events.backend, 0),
            Some(binding)
        );
        assert!(
            fixture
                .scene
                .events
                .backend
                .platform
                .cursor_plane_visible_for_output(0)
        );
        assert_eq!(
            sprite_users(&fixture.scene.events.backend, 0, binding.lease),
            Some(1)
        );

        let hide_start = record_count(&fixture, hardware.device_fd);
        Backend::set_cursor_hidden(&mut fixture.scene.events.backend, true);
        damage_root(&mut fixture.scene);
        let properties = cursor_properties_for_output(&fixture.scene.events.backend, 0);
        let detach = compose_until_cursor_commit_where(
            &mut fixture,
            &mut delivered,
            0,
            hide_start,
            properties.fb_id,
            |record| record_property(record, properties.fb_id) == Some(0),
        );
        assert_eq!(
            applied_cursor_binding(&fixture.scene.events.backend, 0),
            None
        );
        assert_eq!(
            sprite_users(&fixture.scene.events.backend, 0, binding.lease),
            Some(1)
        );
        retire_cursor_record(&mut fixture, &mut delivered, 0, &detach, 55);
        assert_eq!(
            sprite_users(&fixture.scene.events.backend, 0, binding.lease),
            Some(0)
        );
        finish_atomic_fixture(fixture, &mut delivered);
    }
}

#[test]
#[ignore = "requires the lavapipe GPU scene fixture"]
// Mutation: fail to re-enable the latest cursor-only retry after the next event.
fn p2_busy_stream_keeps_latest_intent_and_waits_for_the_event() {
    let Some(mut fixture) =
        atomic_fixture("p2_busy_stream_keeps_latest_intent_and_waits_for_the_event")
    else {
        return;
    };
    let mut delivered = HashSet::new();
    let hardware = establish(&mut fixture, &mut delivered, (0, 0));
    let recorder = std::rc::Rc::clone(&fixture.scene.events.recorders[&hardware.device_fd]);
    let start = records(&fixture, hardware).len();
    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        130,
        110,
    );
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("submit first cursor-only move");
    let first = records(&fixture, hardware)
        .into_iter()
        .skip(start)
        .find(|record| record.kind == AtomicCommitKind::CursorOnly)
        .expect("first cursor-only move accepted");
    for point in [(140, 120), (150, 130), (160, 140)] {
        Backend::warp_pointer_root(
            &mut fixture.scene.events.backend,
            &mut fixture.scene.events.state,
            point.0,
            point.1,
        );
        Backend::maybe_composite(&mut fixture.scene.events.backend)
            .expect("motion while a cursor commit is in flight");
    }
    assert_eq!(records(&fixture, hardware).len(), start + 1);
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EBUSY)));
    retire_cursor_record(
        &mut fixture,
        &mut delivered,
        hardware.output_idx,
        &first,
        44,
    );
    let attempts_before_busy_retry = recorder.records().len();
    Backend::before_block(&mut fixture.scene.events.backend);
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("latest move races the retiring event with EBUSY");
    assert!(
        fixture
            .scene
            .events
            .backend
            .platform
            .cursor_retry_waits_for_flip(hardware.output_idx)
    );
    assert_eq!(
        recorder.records().len(),
        attempts_before_busy_retry + 1,
        "one latest-position retry was rejected with EBUSY",
    );
    let attempts_after_busy = recorder.records().len();
    for _ in 0..3 {
        Backend::before_block(&mut fixture.scene.events.backend);
        Backend::maybe_composite(&mut fixture.scene.events.backend)
            .expect("retry wait does not resubmit before another event");
        assert_eq!(recorder.records().len(), attempts_after_busy);
    }

    let _latest = fixture
        .scene
        .events
        .backend
        .platform
        .for_tests_atomic_cursor_pending_position(
            fixture.scene.events.backend.platform.outputs[hardware.output_idx]
                .key
                .device_key,
            hardware.crtc,
        )
        .expect("latest position remains unsent after EBUSY");
    // The scene carries the latest point at submission. Motion during that
    // scene flight must remain pending and be sent by one cursor-only commit
    // after its event clears the EBUSY wait.
    let root = fixture.scene.events.backend.core.window_id;
    let width = fixture.scene.events.backend.platform.fb_w;
    let height = fixture.scene.events.backend.platform.fb_h;
    Backend::clear_area(
        &mut fixture.scene.events.backend,
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
    .expect("damage scene to produce the next event");
    let scene_start = records(&fixture, hardware).len();
    Backend::maybe_composite(&mut fixture.scene.events.backend).expect("submit scene event");
    let scene = records(&fixture, hardware)
        .into_iter()
        .skip(scene_start)
        .find(|record| {
            matches!(
                record.kind,
                AtomicCommitKind::Scene | AtomicCommitKind::SceneWithCursor
            )
        })
        .expect("scene commit accepted");
    let properties =
        cursor_properties_for_output(&fixture.scene.events.backend, hardware.output_idx);
    assert_position_only(&scene, properties, (160, 140));
    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        170,
        150,
    );
    retire_cursor_record(
        &mut fixture,
        &mut delivered,
        hardware.output_idx,
        &scene,
        46,
    );

    let cursor_retry_start = records(&fixture, hardware).len();
    Backend::before_block(&mut fixture.scene.events.backend);
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("event retirement re-enables one cursor-only retry");
    let cursor_retry = records(&fixture, hardware)
        .into_iter()
        .skip(cursor_retry_start)
        .find(|record| record.kind == AtomicCommitKind::CursorOnly)
        .expect("latest pending motion is retried as CursorOnly after the scene event");
    assert_position_only(&cursor_retry, properties, (170, 150));
    assert_eq!(
        records(&fixture, hardware)
            .into_iter()
            .skip(cursor_retry_start)
            .filter(|record| record.kind == AtomicCommitKind::CursorOnly)
            .count(),
        1,
        "one post-event cursor-only retry carries the latest position",
    );
    retire_cursor_record(
        &mut fixture,
        &mut delivered,
        hardware.output_idx,
        &cursor_retry,
        47,
    );
    assert!(
        fixture
            .scene
            .events
            .backend
            .platform
            .for_tests_atomic_cursor_pending_position(
                fixture.scene.events.backend.platform.outputs[hardware.output_idx]
                    .key
                    .device_key,
                hardware.crtc,
            )
            .is_none()
    );
    let settled = records(&fixture, hardware).len();
    for _ in 0..3 {
        Backend::before_block(&mut fixture.scene.events.backend);
        Backend::maybe_composite(&mut fixture.scene.events.backend)
            .expect("settled latest position is not submitted again");
        assert_eq!(records(&fixture, hardware).len(), settled);
    }
    finish(fixture, &mut delivered, hardware);
}

#[test]
// Mutation: route a Legacy device through Atomic position handling.
fn p2_legacy_motion_is_unchanged_on_a_legacy_path_device() {
    use super::super::commit_identity::support::AtomicEventFixture;
    use crate::kms::cursor_plane::topology::{
        CursorPlaneTestPlane, CursorPlaneTestSnapshot, CursorStatePath,
    };
    use std::rc::Rc;

    let mut backend = match crate::kms::render::backend::KmsBackend::for_tests_with_vk() {
        Ok(backend) => backend,
        Err(error) => {
            println!(
                "SKIP p2_legacy_motion_is_unchanged_on_a_legacy_path_device: no Vulkan test device ({error})"
            );
            return;
        }
    };
    let layouts = backend.platform.scene_output_layouts();
    backend.scene = crate::kms::render::scene::SceneCompositor::new(&backend.platform, &layouts)
        .expect("construct hardware-free live scene");
    let mut events = AtomicEventFixture::from_backend(backend);
    let crtc = events.backend.platform.outputs[0].output.crtc;
    let properties = [
        "FB_ID", "CRTC_ID", "CRTC_Y", "CRTC_W", "CRTC_H", "SRC_X", "SRC_Y", "SRC_W", "SRC_H",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let (io, ledger) = crate::kms::cursor_plane::ops::recording::RecordingCursorBufferIo::new(1);
    let legacy = Rc::new(super::support::RecordingLegacyCursor::default());
    events
        .backend
        .platform
        .for_tests_install_cursor_plane_from_snapshot(
            0,
            CursorPlaneTestSnapshot {
                crtcs: vec![u32::from(crtc)],
                planes: vec![CursorPlaneTestPlane {
                    handle: 21,
                    possible_crtcs: vec![u32::from(crtc)],
                    property_names: properties,
                }],
                cursor_caps: Some(crate::drm::CursorDeviceCaps::new(
                    false,
                    Some("amdgpu".to_owned()),
                )),
                has_buffer: true,
            },
            Rc::new(io),
            64,
            64,
        );
    let device = &mut events.backend.platform.devices[0];
    assert_eq!(device.cursor.state_path(), CursorStatePath::Legacy);
    let plane = device.cursor.plane.as_mut().expect("legacy path plane");
    plane.for_tests_set_visible(crtc, true);
    plane.for_tests_set_show_executor(legacy.clone());
    plane.for_tests_set_hide_executor(legacy.clone());

    let cursor = create_bitmap_cursor(&mut events.backend, true);
    define_root_cursor(&mut events.backend, cursor);
    events.backend.scene.for_tests_set_hw_cursor_mode();
    events.backend.scene.scene_structure_dirty = false;
    events.backend.platform.devices[0]
        .cursor
        .plane
        .as_mut()
        .expect("legacy path plane remains installed")
        .for_tests_set_visible(crtc, true);
    assert_eq!(events.backend.effective_cursor_xid, Some(cursor.as_raw()));
    assert!(!events.backend.effective_cursor_hidden());
    assert_eq!(
        events.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hw,
    );
    assert_ne!(
        (events.backend.core.cursor_x, events.backend.core.cursor_y),
        (130.0, 110.0),
    );
    Backend::warp_pointer_root(&mut events.backend, &mut events.state, 130, 110);
    assert_eq!(
        events.backend.platform.devices[0]
            .atomic_commit_sink
            .for_tests_motion_to_accept_sample_count(),
        1,
        "one Legacy-path move attempt is recorded by the existing motion telemetry",
    );
    assert!(events.recorders.values().all(|recorder| {
        !accepted_live_records(recorder)
            .iter()
            .any(|record| record.kind == AtomicCommitKind::CursorOnly)
    }));
    let mut delivered = HashSet::new();
    Backend::set_cursor_hidden(&mut events.backend, true);
    let root = events.backend.core.window_id;
    Backend::define_cursor(&mut events.backend, None, root, 0x7f10)
        .expect("restore the fixture's sticky root cursor");
    Backend::free_cursor(&mut events.backend, None, cursor.as_raw())
        .expect("free legacy motion test cursor");
    super::support::finish_fixture(&mut events, &mut delivered);
    assert_no_cursor_resources(&events.backend, Some(&ledger));
}
