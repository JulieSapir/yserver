use super::*;

use crate::drm::atomic::{
    AtomicCommitKind, AtomicCommitSink,
    tests_support::{AtomicCommitRecord, RecordingAtomicCommitExecutor},
};

fn recording_sink(
    backend: &mut KmsBackend,
    device_idx: usize,
) -> Rc<RecordingAtomicCommitExecutor> {
    let recorder = Rc::new(RecordingAtomicCommitExecutor::default());
    backend.platform.devices[device_idx].atomic_commit_sink =
        Rc::new(AtomicCommitSink::new(recorder.clone()));
    recorder
}

fn raw<T: ::drm::control::ResourceHandle>(value: u32) -> T {
    ::drm::control::from_u32(value).expect("nonzero test handle")
}

fn assert_cursor_properties(
    record: &AtomicCommitRecord,
    position: crate::drm::atomic::CursorPlanePosition,
) {
    for (property, value) in [
        (position.crtc_x, i64::from(position.x) as u64),
        (position.crtc_y, i64::from(position.y) as u64),
    ] {
        assert!(
            record.properties.iter().any(|update| {
                update.object == position.plane.into()
                    && update.property == property
                    && update.value == value
            }),
            "missing cursor property {property:?}={value} on {:?}: {:?}",
            position.plane,
            record.properties,
        );
    }
}

fn set_scene_fence_properties(output: &mut crate::platform::drm::Output) {
    output.plane_in_fence_fd_prop = Some(raw(81));
    output.crtc_out_fence_ptr_prop = Some(raw(82));
}

#[test]
// Mutation: omit `request.add_cursor_position` from the shared scene-flip
// builder. The recorded request must then miss these latest-position values.
fn shared_scene_flip_records_the_latest_cursor_position() {
    let mut backend = KmsBackend::for_tests();
    let recorder = recording_sink(&mut backend, 0);
    let device_key = backend.platform.devices[0].key;
    let position = backend.platform.for_tests_seed_atomic_cursor_position(
        0,
        raw(51),
        raw(61),
        raw(62),
        123,
        45,
        3,
        4,
    );
    assert_eq!((position.x, position.y), (120, 41));
    let positions = backend
        .platform
        .cursor_plane_atomic_positions_for_outputs(&[0]);
    assert_eq!(positions, vec![position]);

    set_scene_fence_properties(&mut backend.platform.outputs[0].output);
    let output = &backend.platform.outputs[0].output;
    let mut out_fence = -1;
    crate::drm::page_flip::submit_scene_flip_with_fences(
        &backend.platform.devices[0].device,
        &backend.platform.devices[0].atomic_commit_sink,
        output,
        raw(71),
        -1,
        &mut out_fence,
        &positions,
        crate::drm::async_flip::FlipTiming::Vsync,
    )
    .expect("record shared scene flip");

    let records = recorder.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, AtomicCommitKind::SceneWithCursor);
    assert!(
        records[0]
            .flags
            .contains(::drm::control::AtomicCommitFlags::NONBLOCK)
    );
    assert_cursor_properties(&records[0], position);
    backend
        .platform
        .cursor_plane_scene_commit_submitted(&positions);
    assert!(
        !backend
            .platform
            .for_tests_atomic_position_is_unsent(device_key, position.crtc)
    );
}

#[test]
// Mutation: source the copied flip's cursor plane from the renderer device.
// Both devices intentionally reuse the raw CRTC handle with different planes.
fn copied_scene_flip_uses_the_sink_devices_cursor_plane() {
    let mut backend = KmsBackend::for_tests();
    let renderer_key = backend.platform.devices[0].key;
    let sink_key = test_device_key(77);
    push_test_device(&mut backend, sink_key);
    backend.platform.outputs[0].key.device_key = renderer_key;
    let renderer_position = backend.platform.for_tests_seed_atomic_cursor_position(
        0,
        raw(151),
        raw(161),
        raw(162),
        100,
        90,
        0,
        0,
    );
    backend.platform.outputs[0].key.device_key = sink_key;
    let sink_position = backend.platform.for_tests_seed_atomic_cursor_position(
        0,
        raw(251),
        raw(261),
        raw(262),
        42,
        33,
        0,
        0,
    );
    assert_ne!(renderer_position.plane, sink_position.plane);

    let recorder = recording_sink(&mut backend, 1);
    set_scene_fence_properties(&mut backend.platform.outputs[0].output);
    let positions = backend
        .platform
        .cursor_plane_atomic_positions_for_outputs(&[0]);
    assert_eq!(positions, vec![sink_position]);
    let sink_device = &backend.platform.devices[1];
    let mut out_fence = -1;
    crate::drm::page_flip::submit_scene_flip_with_fences(
        &sink_device.device,
        &sink_device.atomic_commit_sink,
        &backend.platform.outputs[0].output,
        raw(71),
        -1,
        &mut out_fence,
        &positions,
        crate::drm::async_flip::FlipTiming::Vsync,
    )
    .expect("record copied sink flip");

    let records = recorder.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, AtomicCommitKind::SceneWithCursor);
    assert_cursor_properties(&records[0], sink_position);
    backend
        .platform
        .cursor_plane_scene_commit_submitted(&positions);
    assert!(
        backend
            .platform
            .for_tests_atomic_position_is_unsent(renderer_key, renderer_position.crtc)
    );
    assert!(
        !backend
            .platform
            .for_tests_atomic_position_is_unsent(sink_key, sink_position.crtc)
    );
}

#[test]
// Mutation: return atomic positions for every visible plane without checking
// the per-device atomic capability. The legacy request would gain cursor fields.
fn legacy_scene_flip_has_no_atomic_cursor_properties() {
    let mut backend = KmsBackend::for_tests();
    let recorder = recording_sink(&mut backend, 0);
    let device_key = backend.platform.devices[0].key;
    let position = backend.platform.for_tests_seed_atomic_cursor_position(
        0,
        raw(51),
        raw(61),
        raw(62),
        123,
        45,
        3,
        4,
    );
    backend.platform.devices[0].cursor.atomic_cursor = false;
    let positions = backend
        .platform
        .cursor_plane_atomic_positions_for_outputs(&[0]);
    assert!(positions.is_empty());

    set_scene_fence_properties(&mut backend.platform.outputs[0].output);
    let mut out_fence = -1;
    crate::drm::page_flip::submit_scene_flip_with_fences(
        &backend.platform.devices[0].device,
        &backend.platform.devices[0].atomic_commit_sink,
        &backend.platform.outputs[0].output,
        raw(71),
        -1,
        &mut out_fence,
        &positions,
        crate::drm::async_flip::FlipTiming::Vsync,
    )
    .expect("record legacy scene flip");

    let records = recorder.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, AtomicCommitKind::Scene);
    assert!(
        !records[0]
            .properties
            .iter()
            .any(|update| update.object == position.plane.into())
    );
    assert!(
        backend
            .platform
            .for_tests_atomic_position_is_unsent(device_key, position.crtc)
    );
}

fn backend_with_live_scene() -> Option<KmsBackend> {
    let mut backend = match KmsBackend::for_tests_with_vk() {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("skipping: no Vulkan test device: {error}");
            return None;
        }
    };
    let layouts = backend.platform.scene_output_layouts();
    backend.scene = crate::kms::render::scene::SceneCompositor::new(&backend.platform, &layouts)
        .expect("construct hardware-free live scene");
    install_visible_cursor_via_backend(&mut backend);
    backend.scene.for_tests_set_hw_cursor_mode();
    backend.scene.scene_structure_dirty = false;
    for drawable in backend.store.armed_damaged_ids() {
        if let Some(snapshot) = backend.store.peek_presentation_damage(drawable) {
            backend.store.ack_presentation_damage(snapshot);
        }
    }
    Some(backend)
}

fn install_visible_cursor_via_backend(backend: &mut KmsBackend) {
    use yserver_core::backend::Backend;

    let source = Backend::create_pixmap(backend, None, 1, 1, 1).expect("cursor source pixmap");
    Backend::put_image(backend, None, source.as_raw(), 1, 1, 1, 0, 0, &[1, 0, 0, 0])
        .expect("paint visible cursor pixel");
    let cursor = Backend::create_cursor(
        backend,
        None,
        source,
        None,
        (u16::MAX, u16::MAX, u16::MAX),
        (0, 0, 0),
        0,
        0,
    )
    .expect("create visible cursor");
    Backend::define_cursor(backend, None, backend.core.window_id, cursor.as_raw())
        .expect("define visible root cursor");
    assert!(!backend.effective_cursor_hidden());
}

fn backend_with_hidden_dead_zone_scene() -> Option<KmsBackend> {
    let mut backend = match KmsBackend::for_tests_with_vk() {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("skipping: no Vulkan test device: {error}");
            return None;
        }
    };
    push_test_output(&mut backend, 0xCA11);
    backend.platform.outputs[1].x = 900;
    backend.platform.outputs[1].width = 800;
    backend.platform.outputs[1].height = 400;
    backend.platform.fb_w = 1700;
    backend.platform.fb_h = 600;
    let layouts = backend.platform.scene_output_layouts();
    backend.scene = crate::kms::render::scene::SceneCompositor::new(&backend.platform, &layouts)
        .expect("construct dead-zone live scene");
    backend.scene.scene_structure_dirty = false;
    for drawable in backend.store.armed_damaged_ids() {
        if let Some(snapshot) = backend.store.peek_presentation_damage(drawable) {
            backend.store.ack_presentation_damage(snapshot);
        }
    }
    Some(backend)
}

fn seed_cursor_position(backend: &mut KmsBackend) -> crate::drm::atomic::CursorPlanePosition {
    backend.platform.for_tests_seed_atomic_cursor_position(
        0,
        raw(51),
        raw(61),
        raw(62),
        100,
        100,
        0,
        0,
    )
}

fn warp(backend: &mut KmsBackend, state: &mut yserver_core::server::ServerState, x: i32, y: i32) {
    use yserver_core::backend::Backend;
    Backend::warp_pointer_root(backend, state, x, y);
}

fn maybe_composite(backend: &mut KmsBackend) {
    use yserver_core::backend::Backend;
    Backend::maybe_composite(backend).expect("run backend scheduling pass");
}

fn retire_test_flip(
    backend: &mut KmsBackend,
    state: &mut yserver_core::server::ServerState,
    frame: u32,
) {
    use std::os::fd::{AsFd as _, AsRawFd as _};
    use yserver_core::backend::Backend;

    let output = &backend.platform.outputs[0].output;
    let crtc = output.crtc;
    backend.platform.devices[0]
        .test_page_flip_event_source
        .as_ref()
        .expect("test event source")
        .queue_flip(crtc, frame);
    let drm_fd = backend.platform.devices[0].device.as_fd().as_raw_fd();
    Backend::on_page_flip_ready(backend, state, drm_fd);
}

fn assert_no_cursor_work(backend: &KmsBackend, device_key: crate::platform::drm::DrmDeviceKey) {
    let crtc = backend.platform.outputs[0].output.crtc;
    assert!(
        !backend
            .platform
            .for_tests_atomic_cursor_commit_is_in_flight(device_key, crtc)
    );
    assert!(
        backend
            .platform
            .for_tests_atomic_cursor_pending_position(device_key, crtc)
            .is_none()
    );
}

#[test]
// Mutation: retain the first position instead of replacing it with the latest.
fn idle_motion_coalesces_until_cursor_flip_retires() {
    use yserver_core::server::ServerState;

    let Some(mut backend) = backend_with_live_scene() else {
        return;
    };
    let recorder = recording_sink(&mut backend, 0);
    let device_key = backend.platform.devices[0].key;
    seed_cursor_position(&mut backend);
    let mut state = ServerState::new();

    warp(&mut backend, &mut state, 120, 80);
    warp(&mut backend, &mut state, 180, 130);
    let first_pending = backend
        .platform
        .cursor_plane_atomic_positions_for_outputs(&[0]);
    assert_eq!(first_pending.len(), 1);
    assert!(first_pending[0].motion_timestamp.is_some());
    assert!(
        recorder.records().is_empty(),
        "motion must not submit directly"
    );
    assert!(backend.platform.has_eligible_atomic_cursor_move());
    assert!(backend.scanout_allowed());
    assert!(backend.kms_outputs_active);
    maybe_composite(&mut backend);
    assert_eq!(
        backend.platform.devices[0]
            .atomic_commit_sink
            .for_tests_motion_to_visible_sample_count(),
        0,
        "motion is not visible until its commit retires"
    );

    let first_records = recorder.records();
    assert_eq!(first_records.len(), 1);
    assert_eq!(first_records[0].kind, AtomicCommitKind::CursorOnly);
    assert_cursor_properties(&first_records[0], first_pending[0]);
    assert!(
        backend
            .platform
            .for_tests_atomic_cursor_commit_is_in_flight(device_key, first_pending[0].crtc)
    );
    assert!(
        !backend
            .platform
            .for_tests_atomic_position_is_unsent(device_key, first_pending[0].crtc)
    );

    warp(&mut backend, &mut state, 260, 170);
    let latest = backend
        .platform
        .for_tests_atomic_cursor_pending_position(device_key, first_pending[0].crtc)
        .expect("motion during the cursor flip stays pending");
    assert!(latest.motion_timestamp.is_some());
    maybe_composite(&mut backend);
    assert_eq!(
        recorder.records().len(),
        1,
        "the in-flight commit owns the CRTC"
    );

    retire_test_flip(&mut backend, &mut state, 1);
    assert_eq!(
        backend.platform.devices[0]
            .atomic_commit_sink
            .for_tests_motion_to_visible_sample_count(),
        1,
        "the first cursor flip records motion-to-visible"
    );
    assert!(
        !backend
            .platform
            .for_tests_atomic_cursor_commit_is_in_flight(device_key, latest.crtc)
    );
    maybe_composite(&mut backend);

    let records = recorder.records();
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].kind, AtomicCommitKind::CursorOnly);
    assert_cursor_properties(&records[1], latest);
    assert!(
        backend
            .platform
            .for_tests_atomic_cursor_commit_is_in_flight(device_key, latest.crtc)
    );
    retire_test_flip(&mut backend, &mut state, 2);
    assert_eq!(
        backend.platform.devices[0]
            .atomic_commit_sink
            .for_tests_motion_to_visible_sample_count(),
        2,
        "the latest cursor flip records motion-to-visible"
    );
    assert_no_cursor_work(&backend, device_key);
}

#[test]
// Mutation: restore Task 6's Hidden fast-path skip. Re-entry must wake the
// scene through the Backend pointer entry and make this predicate true.
fn hidden_cursor_motion_from_dead_zone_wakes_scene_on_reentry() {
    use yserver_core::{backend::Backend, server::ServerState};

    let Some(mut backend) = backend_with_hidden_dead_zone_scene() else {
        return;
    };
    install_visible_cursor_via_backend(&mut backend);
    let mut state = ServerState::new();
    Backend::warp_pointer_root(&mut backend, &mut state, 850, 450);
    // Model the initial retirement at a location covered by neither output.
    backend.scene.scene_structure_dirty = false;
    assert!(!backend.cursor_hidden, "this is not XFIXES HideCursor");
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden,
    );
    assert!(!backend.for_tests_scene_wants_compose());

    // The gap and the shorter output's lower edge form the dead zone.
    assert_eq!(
        (backend.core.cursor_x, backend.core.cursor_y),
        (850.0, 450.0)
    );
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden,
    );
    // Model the scene consuming that wake. The next pointer event alone must
    // re-arm the scheduling predicate when it returns to output 1.
    backend.scene.scene_structure_dirty = false;
    assert!(!backend.for_tests_scene_wants_compose());
    Backend::warp_pointer_root(&mut backend, &mut state, 950, 350);
    assert_eq!(
        (backend.core.cursor_x, backend.core.cursor_y),
        (950.0, 350.0)
    );
    assert!(backend.for_tests_scene_wants_compose());
}

#[test]
// Mutation: ignore the scene flip blocker and send the position as cursor-only.
fn motion_during_scene_flip_waits_and_scene_request_carries_latest_position() {
    use yserver_core::server::ServerState;

    let Some(mut backend) = backend_with_live_scene() else {
        return;
    };
    let recorder = recording_sink(&mut backend, 0);
    let device_key = backend.platform.devices[0].key;
    seed_cursor_position(&mut backend);
    let crtc = backend.platform.outputs[0].output.crtc;
    backend.scene.test_set_flip_in_flight(true);
    let mut state = ServerState::new();

    warp(&mut backend, &mut state, 210, 95);
    let latest = backend
        .platform
        .for_tests_atomic_cursor_pending_position(device_key, crtc)
        .expect("motion remains unsent while a scene flip is pending");
    maybe_composite(&mut backend);
    assert!(recorder.records().is_empty());

    retire_test_flip(&mut backend, &mut state, 1);
    backend.scene.test_set_flip_in_flight(false);
    let positions = backend
        .platform
        .cursor_plane_atomic_positions_for_outputs(&[0]);
    assert_eq!(positions, vec![latest]);
    set_scene_fence_properties(&mut backend.platform.outputs[0].output);
    let mut out_fence = -1;
    crate::drm::page_flip::submit_scene_flip_with_fences(
        &backend.platform.devices[0].device,
        &backend.platform.devices[0].atomic_commit_sink,
        &backend.platform.outputs[0].output,
        raw(71),
        -1,
        &mut out_fence,
        &positions,
        crate::drm::async_flip::FlipTiming::Vsync,
    )
    .expect("record the next scene request");
    backend
        .platform
        .cursor_plane_scene_commit_submitted(&positions);
    retire_test_flip(&mut backend, &mut state, 2);

    let records = recorder.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, AtomicCommitKind::SceneWithCursor);
    assert_cursor_properties(&records[0], latest);
    assert_no_cursor_work(&backend, device_key);
}

#[test]
// Mutation: make `output_commit_in_flight` always false in the backend sink.
fn cursor_only_commit_blocks_scene_readiness_until_its_flip_retires() {
    use crate::kms::render::scene::OutputSink;
    use yserver_core::server::ServerState;

    let Some(mut backend) = backend_with_live_scene() else {
        return;
    };
    let _recorder = recording_sink(&mut backend, 0);
    let device_key = backend.platform.devices[0].key;
    seed_cursor_position(&mut backend);
    let mut state = ServerState::new();
    warp(&mut backend, &mut state, 230, 160);
    maybe_composite(&mut backend);

    assert!(OutputSink::output_commit_in_flight(&backend.platform, 0));
    assert!(!backend.scene.has_output_ready_for_submit(&backend.platform));
    retire_test_flip(&mut backend, &mut state, 1);
    assert!(!OutputSink::output_commit_in_flight(&backend.platform, 0));
    assert!(backend.scene.has_output_ready_for_submit(&backend.platform));
    assert_no_cursor_work(&backend, device_key);
}

#[test]
// Mutation: let pointer motion submit cursor-only immediately, bypassing scene priority.
fn scene_damage_after_cursor_retirement_submits_scene_with_latest_position() {
    use yserver_core::server::ServerState;

    let Some(mut backend) = backend_with_live_scene() else {
        return;
    };
    let recorder = recording_sink(&mut backend, 0);
    let device_key = backend.platform.devices[0].key;
    seed_cursor_position(&mut backend);
    let mut state = ServerState::new();

    warp(&mut backend, &mut state, 90, 70);
    maybe_composite(&mut backend);
    assert_eq!(recorder.records().len(), 1);
    Backend::mark_dirty(&mut backend);
    maybe_composite(&mut backend);
    assert_eq!(
        recorder.records().len(),
        1,
        "scene waits for cursor retirement"
    );

    retire_test_flip(&mut backend, &mut state, 1);
    warp(&mut backend, &mut state, 340, 210);
    let latest = backend
        .platform
        .for_tests_atomic_cursor_pending_position(
            device_key,
            backend.platform.outputs[0].output.crtc,
        )
        .expect("post-retirement motion remains pending for the scene");
    maybe_composite(&mut backend);
    assert_eq!(
        recorder.records().len(),
        1,
        "scene arbitration precedes cursor-only"
    );

    let positions = backend
        .platform
        .cursor_plane_atomic_positions_for_outputs(&[0]);
    assert_eq!(positions, vec![latest]);
    set_scene_fence_properties(&mut backend.platform.outputs[0].output);
    let mut out_fence = -1;
    crate::drm::page_flip::submit_scene_flip_with_fences(
        &backend.platform.devices[0].device,
        &backend.platform.devices[0].atomic_commit_sink,
        &backend.platform.outputs[0].output,
        raw(72),
        -1,
        &mut out_fence,
        &positions,
        crate::drm::async_flip::FlipTiming::Vsync,
    )
    .expect("record prioritized scene request");
    backend
        .platform
        .cursor_plane_scene_commit_submitted(&positions);
    let records = recorder.records();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].kind, AtomicCommitKind::CursorOnly);
    assert_eq!(records[1].kind, AtomicCommitKind::SceneWithCursor);
    assert_cursor_properties(&records[1], latest);
    retire_test_flip(&mut backend, &mut state, 2);
    assert_no_cursor_work(&backend, device_key);
}

#[test]
// Mutation: route an atomic-capable backend through its legacy move branch.
fn legacy_path_pointer_motion_keeps_one_legacy_move_and_no_atomic_commit() {
    use yserver_core::server::ServerState;

    let Some(mut backend) = backend_with_live_scene() else {
        return;
    };
    let recorder = recording_sink(&mut backend, 0);
    let device_key = backend.platform.devices[0].key;
    let crtc = backend.platform.outputs[0].output.crtc;
    let mut plane = crate::kms::cursor_plane::CursorPlane::for_tests_stub(
        Rc::clone(&backend.platform.devices[0].device),
        64,
        64,
    );
    plane.for_tests_set_visible(crtc, true);
    backend.platform.devices[0].cursor.plane = Some(plane);
    backend.platform.devices[0].cursor.atomic_cursor = false;
    let mut state = ServerState::new();

    warp(&mut backend, &mut state, 150, 120);

    assert_eq!(recorder.records().len(), 0);
    assert_eq!(
        backend.platform.devices[0]
            .atomic_commit_sink
            .for_tests_motion_to_accept_sample_count(),
        1,
        "one visible legacy cursor move was attempted",
    );
    assert_no_cursor_work(&backend, device_key);
}

#[test]
// Mutation: do not set `cursor_retry_waits_for_flip` after a cursor-only EBUSY.
fn cursor_only_ebusy_waits_for_the_next_flip_before_retrying() {
    use yserver_core::server::ServerState;

    let Some(mut backend) = backend_with_live_scene() else {
        return;
    };
    let recorder = recording_sink(&mut backend, 0);
    let device_key = backend.platform.devices[0].key;
    let position = seed_cursor_position(&mut backend);
    let mut state = ServerState::new();
    recorder.script_result(Err(io::Error::from_raw_os_error(libc::EBUSY)));

    warp(&mut backend, &mut state, 180, 130);
    maybe_composite(&mut backend);
    let crtc = position.crtc;
    assert_eq!(recorder.records().len(), 1);
    assert_eq!(recorder.records()[0].kind, AtomicCommitKind::CursorOnly);
    assert_eq!(
        backend.platform.devices[0]
            .atomic_commit_sink
            .for_tests_ebusy_count(),
        1,
    );
    assert!(backend.platform.cursor_retry_waits_for_flip(0));
    assert!(
        backend
            .platform
            .for_tests_atomic_position_is_unsent(device_key, crtc)
    );
    assert!(
        !backend
            .platform
            .for_tests_atomic_cursor_commit_is_in_flight(device_key, crtc)
    );
    assert!(!backend.for_tests_scene_wants_compose());

    for _ in 0..3 {
        maybe_composite(&mut backend);
        assert_eq!(
            recorder.records().len(),
            1,
            "no cursor-only retry occurs before a flip event",
        );
        assert!(!backend.for_tests_scene_wants_compose());
    }

    warp(&mut backend, &mut state, 260, 170);
    let latest = backend
        .platform
        .for_tests_atomic_cursor_pending_position(device_key, crtc)
        .expect("new motion replaces the unsent position");
    assert!(backend.platform.cursor_retry_waits_for_flip(0));
    maybe_composite(&mut backend);
    assert_eq!(recorder.records().len(), 1);
    assert!(!backend.for_tests_scene_wants_compose());

    retire_test_flip(&mut backend, &mut state, 1);
    assert!(!backend.platform.cursor_retry_waits_for_flip(0));
    assert!(backend.for_tests_scene_wants_compose());
    maybe_composite(&mut backend);
    let records = recorder.records();
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].kind, AtomicCommitKind::CursorOnly);
    assert_cursor_properties(&records[1], latest);
    assert!(
        backend
            .platform
            .for_tests_atomic_cursor_commit_is_in_flight(device_key, crtc)
    );
    retire_test_flip(&mut backend, &mut state, 2);
    assert_no_cursor_work(&backend, device_key);
}

fn has_cursor_property(
    record: &AtomicCommitRecord,
    position: crate::drm::atomic::CursorPlanePosition,
) -> bool {
    record
        .properties
        .iter()
        .any(|update| update.object == position.plane.into())
}

#[test]
// Mutation: blame every failed scene-with-cursor commit on the cursor. A
// buffer, bandwidth or EBUSY failure must not hide the cursor; only a request
// TEST_ONLY rejects as sent and accepts without the cursor fields is the cursor's.
fn scene_commit_failure_blames_the_cursor_only_when_its_fields_cause_it() {
    use ::drm::control::AtomicCommitFlags;

    let einval = || Err(io::Error::from_raw_os_error(libc::EINVAL));
    let cases: [(&str, Vec<io::Result<()>>, bool, usize); 4] = [
        (
            "cursor fields rejected",
            vec![einval(), einval(), Ok(())],
            true,
            3,
        ),
        (
            "scene rejected",
            vec![einval(), einval(), einval()],
            false,
            3,
        ),
        (
            "commit-phase failure",
            vec![Err(io::Error::from_raw_os_error(libc::ENOMEM)), Ok(())],
            false,
            2,
        ),
        (
            "pending commit",
            vec![Err(io::Error::from_raw_os_error(libc::EBUSY))],
            false,
            1,
        ),
    ];
    for (name, results, cursor_blamed, record_count) in cases {
        let mut backend = KmsBackend::for_tests();
        let recorder = recording_sink(&mut backend, 0);
        let position = seed_cursor_position(&mut backend);
        for result in results {
            recorder.script_result(result);
        }
        set_scene_fence_properties(&mut backend.platform.outputs[0].output);
        let mut out_fence = -1;
        let result = crate::drm::page_flip::submit_scene_flip_with_fences(
            &backend.platform.devices[0].device,
            &backend.platform.devices[0].atomic_commit_sink,
            &backend.platform.outputs[0].output,
            raw(71),
            -1,
            &mut out_fence,
            &[position],
            crate::drm::async_flip::FlipTiming::Vsync,
        );
        assert!(result.is_err(), "{name}: the scene commit itself failed");
        assert_eq!(
            backend.platform.devices[0]
                .atomic_commit_sink
                .take_last_cursor_rejection(),
            cursor_blamed,
            "{name}",
        );
        let records = recorder.records();
        assert_eq!(records.len(), record_count, "{name}");
        assert_eq!(records[0].kind, AtomicCommitKind::SceneWithCursor);
        if let Some(full) = records.get(1) {
            assert_eq!(full.flags, AtomicCommitFlags::TEST_ONLY, "{name}");
            assert_eq!(full.properties, records[0].properties, "{name}");
        }
        if let Some(stripped) = records.get(2) {
            assert_eq!(stripped.flags, AtomicCommitFlags::TEST_ONLY, "{name}");
            assert!(!has_cursor_property(stripped, position), "{name}");
            assert_eq!(
                stripped.properties.len() + 2,
                records[0].properties.len(),
                "{name}: only the cursor fields are dropped",
            );
        }
    }
}

#[test]
// Mutation: build the M1 TEST_ONLY request without the cursor fields the live
// direct commit carries. The probe must test exactly what is committed.
fn direct_scanout_probe_tests_the_request_it_commits() {
    let mut backend = KmsBackend::for_tests();
    let recorder = recording_sink(&mut backend, 0);
    let position = seed_cursor_position(&mut backend);
    let platform = &backend.platform;
    let planes = [crate::drm::modeset::DirectScanoutPlaneState {
        output: &platform.outputs[0].output,
        src_x: 0,
        src_y: 0,
        src_w: u32::from(platform.outputs[0].width),
        src_h: u32::from(platform.outputs[0].height),
    }];
    let device = &platform.devices[0].device;
    let sink = &platform.devices[0].atomic_commit_sink;
    crate::drm::modeset::test_direct_scanout(device, sink, raw(91), &planes, &[position])
        .expect("record M1 probe");
    crate::drm::modeset::submit_direct_scanout(
        device,
        sink,
        raw(91),
        &planes,
        &[position],
        crate::drm::async_flip::FlipTiming::Vsync,
        crate::drm::atomic::TearingCommitContext::NONE,
    )
    .expect("record live direct commit");

    let records = recorder.records();
    assert_eq!(records.len(), 2);
    assert_eq!(
        records[0].flags,
        ::drm::control::AtomicCommitFlags::TEST_ONLY
    );
    assert_eq!(records[0].kind, records[1].kind);
    assert_eq!(records[0].properties, records[1].properties);
    assert_cursor_properties(&records[0], position);
}

#[test]
// Mutation: return Ok from a hide on a permanently disabled device without
// issuing it. The scene would then draw the SW cursor beside a stuck HW one.
fn permanently_disabled_cursor_still_hides_a_plane_left_visible() {
    let mut backend = KmsBackend::for_tests();
    let position = seed_cursor_position(&mut backend);
    let hide = Rc::new(super::RecordingCursorHideExecutor::default());
    hide.script([
        super::TestCursorHideResult::Error(libc::EIO),
        super::TestCursorHideResult::Error(libc::EIO),
        super::TestCursorHideResult::Error(libc::EIO),
        super::TestCursorHideResult::Error(libc::EIO),
        super::TestCursorHideResult::Error(libc::EIO),
        super::TestCursorHideResult::Ok,
    ]);
    super::install_recording_cursor_hide_executor(&mut backend, 0, hide.clone());
    backend
        .platform
        .reject_atomic_cursor_commit(&[position], "Invalid argument");
    for x in [101, 102, 103] {
        backend
            .platform
            .cursor_plane_move(x, 100, 0, 0)
            .expect("motion-led hide retry");
    }
    assert!(
        backend
            .platform
            .for_tests_cursor_permanently_disabled(position.device_key)
    );
    assert_eq!(hide.calls(), vec![position.crtc; 4]);
    assert!(backend.platform.cursor_plane_visible_for_output(0));

    assert!(backend.platform.cursor_plane_hide_on_crtc(0).is_err());
    assert_eq!(hide.calls(), vec![position.crtc; 5]);
    assert!(backend.platform.cursor_plane_visible_for_output(0));

    backend
        .platform
        .cursor_plane_hide_all()
        .expect("global hide");
    assert_eq!(hide.calls(), vec![position.crtc; 6]);
    assert!(!backend.platform.cursor_plane_visible_for_output(0));

    backend
        .platform
        .cursor_plane_hide_on_crtc(0)
        .expect("nothing left to detach");
    assert_eq!(hide.calls(), vec![position.crtc; 6]);
}
