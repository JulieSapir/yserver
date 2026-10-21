use super::support::{
    AtomicPathFixture, LegacyPathFixture, PathKind, accept_cursor_detach, accepted_live_records,
    applied_cursor_binding, compose_until_cursor_commit, compose_until_cursor_commit_where,
    create_argb_cursor, define_root_cursor, drain_unretired_output_events, enter_held_direct,
    establish_hardware_cursor, has_unretired_accepted_event, record_count, record_property,
    retire_cursor_record, retire_pending_output, sprite_bytes, sprite_users,
};
use crate::{
    drm::atomic::{AtomicCommitKind, tests_support::ScriptedResult},
    kms::cursor_plane::ops::recording::CursorBufferOperation,
};
use std::{collections::HashSet, rc::Rc};
use yserver_core::backend::Backend;

fn damage_root(fixture: &mut AtomicPathFixture) {
    let backend = &mut fixture.scene.events.backend;
    let root = backend.core.window_id;
    let width = backend.platform.fb_w;
    let height = backend.platform.fb_h;
    Backend::clear_area(backend, None, root, 0, None, 0, 0, width, height, (0, 0))
        .expect("damage root through Backend");
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: skip the Atomic CursorOnly submit after show; the per-step kind assertion loses CursorOnly.
fn p2_cursor_establishment_sequence_matches_legacy_baseline() {
    let (legacy_after_scene_retire, legacy_sequence, legacy_show_calls) = {
        let Some(mut fixture) = LegacyPathFixture::new().expect("legacy baseline fixture") else {
            return;
        };
        let cursor = create_argb_cursor(
            &mut fixture.scene.events.backend,
            (4, 4),
            [0x12, 0x34, 0x56, 0xff],
            (1, 2),
        );
        let cursor_id = cursor.as_raw();
        define_root_cursor(&mut fixture.scene.events.backend, cursor);
        Backend::warp_pointer_root(
            &mut fixture.scene.events.backend,
            &mut fixture.scene.events.state,
            100,
            90,
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
        .expect("damage Legacy baseline scene through Backend");
        let device_fd = fixture.scene.kms_fd(0);
        let crtc = fixture.scene.events.backend.platform.outputs[0].output.crtc;
        let device_key = fixture.scene.events.backend.platform.outputs[0]
            .key
            .device_key;
        let mut delivered = HashSet::new();
        Backend::maybe_composite(&mut fixture.scene.events.backend)
            .expect("compose Legacy baseline cursor scene");
        retire_pending_output(&mut fixture.scene.events, &mut delivered, 0, 1);
        let legacy_after_scene_retire: Vec<_> =
            accepted_live_records(&fixture.scene.events.recorders[&device_fd])
                .into_iter()
                .filter(|record| record.events.iter().any(|event| event.crtc == crtc))
                .map(|record| record.kind)
                .collect();
        assert_eq!(legacy_after_scene_retire, [AtomicCommitKind::Scene]);
        for frame in 2..=4 {
            Backend::maybe_composite(&mut fixture.scene.events.backend)
                .expect("compose any Legacy-path position update");
            if has_unretired_accepted_event(&fixture.scene.events, &delivered, 0) {
                retire_pending_output(&mut fixture.scene.events, &mut delivered, 0, frame);
            }
            if fixture
                .scene
                .events
                .backend
                .platform
                .for_tests_atomic_cursor_pending_position(device_key, crtc)
                .is_none()
                && !has_unretired_accepted_event(&fixture.scene.events, &delivered, 0)
            {
                break;
            }
        }
        assert_eq!(
            fixture
                .scene
                .events
                .backend
                .platform
                .for_tests_atomic_cursor_pending_position(device_key, crtc),
            None,
            "Legacy baseline must settle its position-only cursor update"
        );
        let legacy_sequence: Vec<_> =
            accepted_live_records(&fixture.scene.events.recorders[&device_fd])
                .into_iter()
                .filter(|record| record.events.iter().any(|event| event.crtc == crtc))
                .map(|record| record.kind)
                .collect();
        assert_eq!(
            legacy_sequence,
            [AtomicCommitKind::Scene, AtomicCommitKind::CursorOnly]
        );
        let legacy_show_calls = fixture.legacy.show_calls.get();
        assert_eq!(legacy_show_calls, 1);

        Backend::define_cursor(&mut fixture.scene.events.backend, None, root, 0x7f10)
            .expect("restore sticky default cursor in Legacy baseline");
        Backend::free_cursor(&mut fixture.scene.events.backend, None, cursor_id)
            .expect("free Legacy baseline cursor");
        super::support::finish_fixture(&mut fixture.scene.events, &mut delivered);
        for device in &fixture.scene.events.backend.platform.devices {
            assert_eq!(
                device
                    .atomic_commit_sink
                    .for_tests_pending_page_flip_entry_count(),
                0
            );
        }
        let ledger = Rc::clone(&fixture.ledger);
        fixture.scene.finish_after_normal_drain();
        assert_eq!(ledger.end_state(), (0, 0, 0));
        (
            legacy_after_scene_retire,
            legacy_sequence,
            legacy_show_calls,
        )
    };
    assert_eq!(
        legacy_after_scene_retire,
        [AtomicCommitKind::Scene],
        "Legacy show does not add a cursor commit during the scene retirement step"
    );
    assert_eq!(
        legacy_sequence,
        [AtomicCommitKind::Scene, AtomicCommitKind::CursorOnly]
    );
    assert_eq!(legacy_show_calls, 1);

    let Some(mut fixture) =
        AtomicPathFixture::new(PathKind::Physical).expect("Atomic sequence fixture")
    else {
        return;
    };
    let cursor = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [0x12, 0x34, 0x56, 0xff],
        (1, 2),
    );
    let mut delivered = HashSet::new();
    let hardware = establish_hardware_cursor(&mut fixture, &mut delivered, cursor, (100, 90));
    let atomic_sequence: Vec<_> =
        accepted_live_records(&fixture.scene.events.recorders[&hardware.device_fd])
            .into_iter()
            .filter(|record| {
                record
                    .events
                    .iter()
                    .any(|event| event.crtc == hardware.crtc)
            })
            .map(|record| record.kind)
            .collect();
    assert_eq!(
        atomic_sequence,
        [AtomicCommitKind::Scene, AtomicCommitKind::CursorOnly],
        "Atomic helper sequence must add one ID-tagged cursor-only commit after the Legacy baseline"
    );
    assert_eq!(
        &atomic_sequence[..legacy_sequence.len()],
        legacy_sequence.as_slice(),
        "Atomic scene sequence must retain the Legacy scene event"
    );
    let detach = accept_cursor_detach(&mut fixture, &mut delivered, hardware.output_idx);
    retire_cursor_record(
        &mut fixture,
        &mut delivered,
        hardware.output_idx,
        &detach,
        3,
    );
    super::support::finish_atomic_fixture(fixture, &mut delivered);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: reconstruct the upload error from its kind and message; the unsupported-driver latch assertion fails.
fn p2_atomic_cursor_upload_preserves_unsupported_driver_errno() {
    let Some(mut fixture) =
        AtomicPathFixture::new(PathKind::Physical).expect("Atomic upload fixture")
    else {
        return;
    };
    fixture
        .ledger
        .fail_after(CursorBufferOperation::Create, 0, libc::ENODEV);
    let cursor = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [0x21, 0x43, 0x65, 0xff],
        (1, 2),
    );
    define_root_cursor(&mut fixture.scene.events.backend, cursor);
    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        100,
        90,
    );
    damage_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("submit scene before the failing cursor upload");
    let mut delivered = HashSet::new();
    retire_pending_output(&mut fixture.scene.events, &mut delivered, 0, 1);
    let device_key = fixture.scene.events.backend.platform.outputs[0]
        .key
        .device_key;
    assert!(
        fixture
            .scene
            .events
            .backend
            .platform
            .for_tests_cursor_permanently_disabled(device_key),
        "ENODEV from sprite allocation must permanently disable the cursor path"
    );
    assert_eq!(
        fixture.ledger.failures_taken(),
        [CursorBufferOperation::Create]
    );
    Backend::set_cursor_hidden(&mut fixture.scene.events.backend, true);
    damage_root(&mut fixture);
    super::support::finish_atomic_fixture(fixture, &mut delivered);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: reconstruct the hide error (permanent latch fails), or classify EINVAL twice (fallback remaining is 2).
fn p2_rejected_hide_preserves_errno_and_advances_fallback_once() {
    let run = |error_number: i32| {
        let mut fixture =
            AtomicPathFixture::new(PathKind::Physical).expect("Atomic hide fixture")?;
        let cursor = create_argb_cursor(
            &mut fixture.scene.events.backend,
            (4, 4),
            [0x32, 0x54, 0x76, 0xff],
            (1, 2),
        );
        let mut delivered = HashSet::new();
        let hardware = establish_hardware_cursor(&mut fixture, &mut delivered, cursor, (100, 90));
        let device_key = fixture.scene.events.backend.platform.outputs[0]
            .key
            .device_key;
        let recorder = fixture.scene.events.recorders[&hardware.device_fd].clone();
        let offset = record_count(&fixture, hardware.device_fd);
        Backend::set_cursor_hidden(&mut fixture.scene.events.backend, true);
        damage_root(&mut fixture);
        Backend::maybe_composite(&mut fixture.scene.events.backend)
            .expect("submit scene before the rejected hide");
        recorder.script_result(Err(std::io::Error::from_raw_os_error(error_number)));
        retire_pending_output(
            &mut fixture.scene.events,
            &mut delivered,
            hardware.output_idx,
            2,
        );
        assert!(recorder.records().iter().skip(offset).any(|record| {
            record.kind == AtomicCommitKind::CursorOnly
                && record.result
                    == ScriptedResult::Error {
                        kind: std::io::Error::from_raw_os_error(error_number).kind(),
                        raw_os_error: Some(error_number),
                    }
        }));
        let state = &fixture.scene.events.backend;
        let permanent = state
            .platform
            .for_tests_cursor_permanently_disabled(device_key);
        let fallback = state
            .platform
            .for_tests_cursor_fallback_remaining(device_key, hardware.crtc);
        assert_eq!(
            applied_cursor_binding(state, hardware.output_idx),
            None,
            "successful blocking rollback clears the applied binding"
        );
        assert_eq!(fixture.legacy.hide_calls.get(), 0);
        Backend::set_cursor_hidden(&mut fixture.scene.events.backend, true);
        damage_root(&mut fixture);
        let ledger = Rc::clone(&fixture.ledger);
        super::support::finish_atomic_fixture(fixture, &mut delivered);
        assert_eq!(ledger.end_state(), (0, 0, 0));
        Some((permanent, fallback))
    };

    let Some((permanent, permanent_fallback)) = run(libc::EOPNOTSUPP) else {
        return;
    };
    assert!(
        permanent,
        "the original EOPNOTSUPP errno must latch the cursor path"
    );
    assert_eq!(permanent_fallback, None);

    let Some((permanent, fallback)) = run(libc::EINVAL) else {
        return;
    };
    assert!(!permanent);
    assert_eq!(fallback, Some(1), "one rejected hide advances one fallback");
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: route show through the legacy executor, or drop the SRC_W 16-bit shift.
fn p2_first_hardware_cursor_attaches_with_full_state_and_retires_by_id() {
    let Some(mut fixture) =
        AtomicPathFixture::new(PathKind::Physical).expect("atomic path fixture construction")
    else {
        return;
    };
    let cursor_id = {
        let backend = &mut fixture.scene.events.backend;
        let cursor = create_argb_cursor(backend, (4, 4), [0x12, 0x34, 0x56, 0xff], (1, 2));
        let cursor_id = cursor.as_raw();
        define_root_cursor(backend, cursor);
        cursor_id
    };
    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        100,
        90,
    );
    damage_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("compose initial cursor scene");

    let output_idx = 0;
    let layout = &fixture.scene.events.backend.platform.outputs[output_idx];
    let crtc = layout.output.crtc;
    let device_fd = fixture.scene.kms_fd(output_idx);
    let properties =
        super::support::cursor_properties_for_output(&fixture.scene.events.backend, output_idx);
    let mut delivered = HashSet::new();
    let scene_id = retire_pending_output(&mut fixture.scene.events, &mut delivered, output_idx, 1);
    assert_ne!(scene_id, 0);

    let recorder = &fixture.scene.events.recorders[&device_fd];
    let accepted = accepted_live_records(recorder);
    let attach = accepted
        .iter()
        .find(|record| {
            record.kind == AtomicCommitKind::CursorOnly
                && record_property(record, properties.fb_id).is_some()
        })
        .cloned()
        .expect("Atomic first hardware cursor show must submit a CursorOnly attach");

    assert_eq!(attach.kind, AtomicCommitKind::CursorOnly);
    assert_eq!(
        attach.flags,
        ::drm::control::AtomicCommitFlags::NONBLOCK
            | ::drm::control::AtomicCommitFlags::PAGE_FLIP_EVENT
    );
    assert_ne!(attach.user_data, 0);
    assert_eq!(
        record_property(&attach, properties.crtc_id),
        Some(u64::from(u32::from(crtc)))
    );
    assert_eq!(record_property(&attach, properties.crtc_x), Some(99));
    assert_eq!(record_property(&attach, properties.crtc_y), Some(88));
    assert_eq!(record_property(&attach, properties.src_x), Some(0));
    assert_eq!(record_property(&attach, properties.src_y), Some(0));
    assert_eq!(
        record_property(&attach, properties.src_w),
        Some(64_u64 << 16)
    );
    assert_eq!(
        record_property(&attach, properties.src_h),
        Some(64_u64 << 16)
    );
    assert_eq!(record_property(&attach, properties.crtc_w), Some(64));
    assert_eq!(record_property(&attach, properties.crtc_h), Some(64));
    if let Some(hotspot_x) = properties.hotspot_x {
        assert_eq!(record_property(&attach, hotspot_x), None);
    }
    if let Some(hotspot_y) = properties.hotspot_y {
        assert_eq!(record_property(&attach, hotspot_y), None);
    }
    assert_eq!(attach.properties.len(), 10);
    assert_eq!(fixture.legacy.show_calls.get(), 0);
    assert_eq!(fixture.legacy.move_calls.get(), 0);
    assert_eq!(fixture.legacy.hide_calls.get(), 0);
    assert!(
        fixture
            .scene
            .events
            .backend
            .platform
            .any_cursor_commit_in_flight()
    );

    let attach_id = attach.user_data;
    assert_eq!(
        retire_pending_output(&mut fixture.scene.events, &mut delivered, output_idx, 2),
        attach_id,
        "the attach is retired by its accepted ID"
    );
    assert!(
        !fixture
            .scene
            .events
            .backend
            .platform
            .any_cursor_commit_in_flight()
    );
    let applied = super::support::applied_cursor_binding(&fixture.scene.events.backend, output_idx)
        .expect("accepted cursor binding");
    assert_eq!((applied.x, applied.y), (99, 88));
    assert_eq!(
        super::support::sprite_users(&fixture.scene.events.backend, output_idx, applied.lease),
        Some(1)
    );

    Backend::set_cursor_hidden(&mut fixture.scene.events.backend, true);
    damage_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.scene.events.backend).expect("compose cursor hide scene");
    retire_pending_output(&mut fixture.scene.events, &mut delivered, output_idx, 3);
    let detach = accepted_live_records(&fixture.scene.events.recorders[&device_fd])
        .into_iter()
        .find(|record| {
            record.kind == AtomicCommitKind::CursorOnly
                && record_property(record, properties.fb_id) == Some(0)
                && record_property(record, properties.crtc_id) == Some(0)
        })
        .expect("hide must submit an atomic plane detach");
    assert!(
        !fixture
            .scene
            .events
            .backend
            .platform
            .cursor_plane_visible_for_output(output_idx)
    );
    assert_eq!(
        super::support::sprite_users(&fixture.scene.events.backend, output_idx, applied.lease),
        Some(1),
        "the accepted detach retains the old sprite until its event"
    );
    assert_eq!(
        retire_pending_output(&mut fixture.scene.events, &mut delivered, output_idx, 4),
        detach.user_data
    );
    assert_eq!(
        super::support::sprite_users(&fixture.scene.events.backend, output_idx, applied.lease),
        Some(0)
    );
    let root = fixture.scene.events.backend.core.window_id;
    Backend::define_cursor(&mut fixture.scene.events.backend, None, root, 0x7f10)
        .expect("restore recording scene's sticky default cursor");
    Backend::free_cursor(&mut fixture.scene.events.backend, None, cursor_id)
        .expect("free test cursor after detach");
    super::support::finish_fixture(&mut fixture.scene.events, &mut delivered);
    fixture.scene.finish_after_normal_drain();
    assert_eq!(fixture.ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: omit HOTSPOT_X/HOTSPOT_Y on an Atomic plane that exposes them.
fn p2_paravirtualized_attach_carries_hotspot_properties() {
    let Some(mut fixture) = AtomicPathFixture::new(PathKind::Paravirtualized)
        .expect("paravirtualized cursor fixture construction")
    else {
        return;
    };
    let cursor_a = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [0x23, 0x45, 0x67, 0xff],
        (1, 2),
    );
    let mut delivered = HashSet::new();
    let hardware = establish_hardware_cursor(&mut fixture, &mut delivered, cursor_a, (100, 90));
    let properties = super::support::cursor_properties_for_output(
        &fixture.scene.events.backend,
        hardware.output_idx,
    );
    let attach = accepted_live_records(&fixture.scene.events.recorders[&hardware.device_fd])
        .into_iter()
        .find(|record| record.user_data == hardware.attach_commit_id)
        .expect("accepted attach record remains in the recorder");
    assert_eq!(
        record_property(&attach, properties.hotspot_x.unwrap()),
        Some(1)
    );
    assert_eq!(
        record_property(&attach, properties.hotspot_y.unwrap()),
        Some(2)
    );

    let first = applied_cursor_binding(&fixture.scene.events.backend, hardware.output_idx)
        .expect("first cursor is applied");
    let cursor_b = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [0x23, 0x45, 0x67, 0xff],
        (3, 4),
    );
    let cursor_b_id = cursor_b.as_raw();
    define_root_cursor(&mut fixture.scene.events.backend, cursor_b);
    let offset = record_count(&fixture, hardware.device_fd);
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
    .expect("damage root for hotspot replacement");
    let replacement = compose_until_cursor_commit(
        &mut fixture,
        &mut delivered,
        hardware.output_idx,
        offset,
        properties.hotspot_x.unwrap(),
        3,
    );
    assert_ne!(
        record_property(&replacement, properties.fb_id),
        Some(first.framebuffer.into())
    );
    assert_eq!(
        record_property(&replacement, properties.hotspot_x.unwrap()),
        Some(3)
    );
    assert_eq!(
        record_property(&replacement, properties.hotspot_y.unwrap()),
        Some(4)
    );
    let second = applied_cursor_binding(&fixture.scene.events.backend, hardware.output_idx)
        .expect("second cursor is applied at acceptance");
    assert_eq!(
        second.lease.version(),
        fixture.scene.events.backend.cursor_records[&cursor_b_id].version
    );
    assert_eq!(
        sprite_users(
            &fixture.scene.events.backend,
            hardware.output_idx,
            first.lease
        ),
        Some(1)
    );
    assert_eq!(
        sprite_users(
            &fixture.scene.events.backend,
            hardware.output_idx,
            second.lease
        ),
        Some(1)
    );
    retire_cursor_record(
        &mut fixture,
        &mut delivered,
        hardware.output_idx,
        &replacement,
        6,
    );
    assert_eq!(
        sprite_users(
            &fixture.scene.events.backend,
            hardware.output_idx,
            first.lease
        ),
        Some(0)
    );

    let detach = accept_cursor_detach(&mut fixture, &mut delivered, hardware.output_idx);
    assert!(
        !fixture
            .scene
            .events
            .backend
            .platform
            .cursor_plane_visible_for_output(hardware.output_idx)
    );
    assert_eq!(
        sprite_users(
            &fixture.scene.events.backend,
            hardware.output_idx,
            second.lease
        ),
        Some(1)
    );
    retire_cursor_record(
        &mut fixture,
        &mut delivered,
        hardware.output_idx,
        &detach,
        7,
    );
    assert_eq!(
        sprite_users(
            &fixture.scene.events.backend,
            hardware.output_idx,
            second.lease
        ),
        Some(0)
    );

    super::support::finish_atomic_fixture(fixture, &mut delivered);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: release the old sprite when the replacement is accepted instead of when its ID retires.
fn p2_image_replacement_keeps_the_old_sprite_until_the_replacing_event() {
    let Some(mut fixture) =
        AtomicPathFixture::new(PathKind::Physical).expect("atomic cursor fixture construction")
    else {
        return;
    };
    let cursor_a = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [0x31, 0x41, 0x59, 0xff],
        (1, 2),
    );
    let mut delivered = HashSet::new();
    let hardware = establish_hardware_cursor(&mut fixture, &mut delivered, cursor_a, (100, 90));
    let properties = super::support::cursor_properties_for_output(
        &fixture.scene.events.backend,
        hardware.output_idx,
    );
    let binding_a = applied_cursor_binding(&fixture.scene.events.backend, hardware.output_idx)
        .expect("cursor A applied");
    let bytes_a = sprite_bytes(
        &fixture.scene.events.backend,
        hardware.output_idx,
        binding_a.lease,
    )
    .expect("cursor A sprite bytes");

    let cursor_b = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [0x62, 0x71, 0x82, 0xff],
        (1, 2),
    );
    define_root_cursor(&mut fixture.scene.events.backend, cursor_b);
    let offset = record_count(&fixture, hardware.device_fd);
    damage_root(&mut fixture);
    let replacement = compose_until_cursor_commit_where(
        &mut fixture,
        &mut delivered,
        hardware.output_idx,
        offset,
        properties.fb_id,
        |record| {
            record_property(record, properties.fb_id) != Some(u64::from(binding_a.framebuffer))
        },
    );
    assert_eq!(replacement.properties.len(), 1);
    assert_eq!(replacement.properties[0].property, properties.fb_id);
    let binding_b = applied_cursor_binding(&fixture.scene.events.backend, hardware.output_idx)
        .expect("cursor B becomes applied at acceptance");
    assert_ne!(binding_a.lease, binding_b.lease);
    assert_eq!(
        sprite_users(
            &fixture.scene.events.backend,
            hardware.output_idx,
            binding_a.lease
        ),
        Some(1)
    );
    assert_eq!(
        sprite_users(
            &fixture.scene.events.backend,
            hardware.output_idx,
            binding_b.lease
        ),
        Some(1)
    );
    assert_eq!(
        sprite_bytes(
            &fixture.scene.events.backend,
            hardware.output_idx,
            binding_a.lease
        ),
        Some(bytes_a.clone())
    );
    assert!(!fixture.ledger.records().iter().any(|record| matches!(
        record,
        crate::kms::cursor_plane::ops::recording::CursorBufferRecord::RemoveFb { fb, .. }
            if *fb == binding_a.framebuffer
    )));

    retire_cursor_record(
        &mut fixture,
        &mut delivered,
        hardware.output_idx,
        &replacement,
        6,
    );
    assert_eq!(
        sprite_users(
            &fixture.scene.events.backend,
            hardware.output_idx,
            binding_a.lease
        ),
        Some(0)
    );
    let replacement_id = replacement.user_data;
    fixture.scene.events.queue_commit_completion(
        hardware.device_fd,
        hardware.crtc,
        replacement_id,
        7,
        std::time::Duration::from_millis(70),
    );
    Backend::on_page_flip_ready(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        hardware.device_fd,
    );
    assert_eq!(
        sprite_users(
            &fixture.scene.events.backend,
            hardware.output_idx,
            binding_a.lease
        ),
        Some(0)
    );
    assert_eq!(
        sprite_users(
            &fixture.scene.events.backend,
            hardware.output_idx,
            binding_b.lease
        ),
        Some(1)
    );

    let detach = accept_cursor_detach(&mut fixture, &mut delivered, hardware.output_idx);
    assert_eq!(
        sprite_users(
            &fixture.scene.events.backend,
            hardware.output_idx,
            binding_b.lease
        ),
        Some(1)
    );
    retire_cursor_record(
        &mut fixture,
        &mut delivered,
        hardware.output_idx,
        &detach,
        8,
    );
    assert_eq!(
        sprite_users(
            &fixture.scene.events.backend,
            hardware.output_idx,
            binding_b.lease
        ),
        Some(0)
    );
    super::support::finish_atomic_fixture(fixture, &mut delivered);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: allow an unmatched explicit cookie to retire the pending cursor replacement.
fn p2_wrong_explicit_id_is_dropped_before_cursor_retirement() {
    let Some(mut fixture) =
        AtomicPathFixture::new(PathKind::Physical).expect("atomic cursor fixture construction")
    else {
        return;
    };
    let cursor_a = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [1, 2, 3, 255],
        (1, 2),
    );
    let mut delivered = HashSet::new();
    let hardware = establish_hardware_cursor(&mut fixture, &mut delivered, cursor_a, (100, 90));
    let properties = super::support::cursor_properties_for_output(&fixture.scene.events.backend, 0);
    let binding_a = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();
    let cursor_b = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [4, 5, 6, 255],
        (1, 2),
    );
    define_root_cursor(&mut fixture.scene.events.backend, cursor_b);
    let offset = record_count(&fixture, hardware.device_fd);
    damage_root(&mut fixture);
    let replacement = compose_until_cursor_commit_where(
        &mut fixture,
        &mut delivered,
        0,
        offset,
        properties.fb_id,
        |record| {
            record_property(record, properties.fb_id) != Some(u64::from(binding_a.framebuffer))
        },
    );
    let binding_b = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();
    fixture.scene.events.queue_commit_completion(
        hardware.device_fd,
        hardware.crtc,
        u64::MAX,
        6,
        std::time::Duration::from_millis(60),
    );
    Backend::on_page_flip_ready(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        hardware.device_fd,
    );
    assert!(
        fixture
            .scene
            .events
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, binding_a.lease),
        Some(1)
    );
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, binding_b.lease),
        Some(1)
    );
    retire_cursor_record(&mut fixture, &mut delivered, 0, &replacement, 7);
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, binding_a.lease),
        Some(0)
    );
    let detach = accept_cursor_detach(&mut fixture, &mut delivered, 0);
    retire_cursor_record(&mut fixture, &mut delivered, 0, &detach, 8);
    super::support::finish_atomic_fixture(fixture, &mut delivered);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: refuse zero-cookie FIFO attribution or release the prior before a matching event.
fn p2_zero_cookie_event_retires_the_pending_replacement_exactly_once() {
    let Some(mut fixture) =
        AtomicPathFixture::new(PathKind::Physical).expect("atomic cursor fixture construction")
    else {
        return;
    };
    let cursor_a = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [7, 8, 9, 255],
        (1, 2),
    );
    let mut delivered = HashSet::new();
    let hardware = establish_hardware_cursor(&mut fixture, &mut delivered, cursor_a, (100, 90));
    let properties = super::support::cursor_properties_for_output(&fixture.scene.events.backend, 0);
    let binding_a = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();
    let cursor_b = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [10, 11, 12, 255],
        (1, 2),
    );
    define_root_cursor(&mut fixture.scene.events.backend, cursor_b);
    let offset = record_count(&fixture, hardware.device_fd);
    damage_root(&mut fixture);
    let replacement = compose_until_cursor_commit_where(
        &mut fixture,
        &mut delivered,
        0,
        offset,
        properties.fb_id,
        |record| {
            record_property(record, properties.fb_id) != Some(u64::from(binding_a.framebuffer))
        },
    );
    let binding_b = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();
    fixture.scene.events.queue_commit_completion(
        hardware.device_fd,
        hardware.crtc,
        0,
        6,
        std::time::Duration::from_millis(60),
    );
    Backend::on_page_flip_ready(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        hardware.device_fd,
    );
    assert!(
        !fixture
            .scene
            .events
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, binding_a.lease),
        Some(0)
    );
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, binding_b.lease),
        Some(1)
    );
    assert_ne!(replacement.user_data, 0);
    drain_unretired_output_events(&mut fixture, &mut delivered, 0);
    let sink_pending = fixture.scene.events.backend.platform.devices[0]
        .atomic_commit_sink
        .for_tests_pending_page_flip_entry_count();
    assert_eq!(
        sink_pending, 0,
        "the duplicate zero cookie has no pending association"
    );
    fixture.scene.events.queue_commit_completion(
        hardware.device_fd,
        hardware.crtc,
        0,
        7,
        std::time::Duration::from_millis(70),
    );
    Backend::on_page_flip_ready(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        hardware.device_fd,
    );
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, binding_a.lease),
        Some(0)
    );
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, binding_b.lease),
        Some(1)
    );
    let detach = accept_cursor_detach(&mut fixture, &mut delivered, 0);
    retire_cursor_record(&mut fixture, &mut delivered, 0, &detach, 8);
    super::support::finish_atomic_fixture(fixture, &mut delivered);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: leave remember_atomic_position's pending move after the accepted show.
fn p2_accepted_show_leaves_no_pending_move() {
    let Some(mut fixture) =
        AtomicPathFixture::new(PathKind::Physical).expect("atomic cursor fixture construction")
    else {
        return;
    };
    let cursor = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [13, 14, 15, 255],
        (1, 2),
    );
    let mut delivered = HashSet::new();
    let hardware = establish_hardware_cursor(&mut fixture, &mut delivered, cursor, (100, 90));
    let device_key = fixture.scene.events.backend.platform.outputs[0]
        .key
        .device_key;
    assert_eq!(
        fixture
            .scene
            .events
            .backend
            .platform
            .for_tests_atomic_cursor_pending_position(device_key, hardware.crtc),
        None
    );
    let offset = record_count(&fixture, hardware.device_fd);
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("composite without motion after the accepted attach");
    assert!(
        !accepted_live_records(&fixture.scene.events.recorders[&hardware.device_fd])
            .into_iter()
            .skip(offset)
            .any(|record| record.kind == AtomicCommitKind::CursorOnly)
    );
    let detach = accept_cursor_detach(&mut fixture, &mut delivered, 0);
    retire_cursor_record(&mut fixture, &mut delivered, 0, &detach, 7);
    super::support::finish_atomic_fixture(fixture, &mut delivered);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: hide the scene cursor without submitting its atomic FB_ID/CRTC_ID detach.
fn p2_hide_detaches_and_the_event_releases_the_sprite() {
    let Some(mut fixture) =
        AtomicPathFixture::new(PathKind::Physical).expect("atomic cursor fixture construction")
    else {
        return;
    };
    let cursor = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [16, 17, 18, 255],
        (1, 2),
    );
    let mut delivered = HashSet::new();
    let hardware = establish_hardware_cursor(&mut fixture, &mut delivered, cursor, (100, 90));
    let properties = super::support::cursor_properties_for_output(&fixture.scene.events.backend, 0);
    let binding = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();
    let detach = accept_cursor_detach(&mut fixture, &mut delivered, 0);
    assert_eq!(detach.kind, AtomicCommitKind::CursorOnly);
    assert_eq!(record_property(&detach, properties.fb_id), Some(0));
    assert_eq!(record_property(&detach, properties.crtc_id), Some(0));
    assert_eq!(detach.properties.len(), 2);
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
        Some(1)
    );
    retire_cursor_record(&mut fixture, &mut delivered, 0, &detach, 7);
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, binding.lease),
        Some(0)
    );
    assert!(
        !fixture
            .scene
            .events
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );
    let _ = hardware;
    super::support::finish_atomic_fixture(fixture, &mut delivered);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: accept the new lease or reveal the software cursor when the Atomic show returns EBUSY.
fn p2_ebusy_show_defers_without_consuming_a_lease() {
    let Some(mut fixture) =
        AtomicPathFixture::new(PathKind::Physical).expect("atomic cursor fixture construction")
    else {
        return;
    };
    let cursor = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [19, 20, 21, 255],
        (1, 2),
    );
    define_root_cursor(&mut fixture.scene.events.backend, cursor);
    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        100,
        90,
    );
    damage_root(&mut fixture);
    let device_fd = fixture.scene.kms_fd(0);
    let recorder = fixture.scene.events.recorders[&device_fd].clone();
    let start = record_count(&fixture, device_fd);
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("submit scene transition before the scripted cursor result");
    let mode_before = fixture.scene.events.backend.scene.cursor_mode();
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EBUSY)));
    let mut delivered = HashSet::new();
    retire_pending_output(&mut fixture.scene.events, &mut delivered, 0, 1);
    let failed = recorder
        .records()
        .into_iter()
        .skip(start)
        .find(|record| {
            record.device_fd == device_fd
                && record.kind == AtomicCommitKind::CursorOnly
                && record.result != ScriptedResult::Ok
        })
        .expect("scripted Atomic cursor show must return EBUSY");
    let properties = super::support::cursor_properties_for_output(&fixture.scene.events.backend, 0);
    let device_key = fixture.scene.events.backend.platform.outputs[0]
        .key
        .device_key;
    let crtc = fixture.scene.events.backend.platform.outputs[0].output.crtc;
    let candidate = {
        let plane = fixture.scene.events.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap();
        assert_eq!(plane.sprites().prepared_user_count(), 0);
        let candidate = plane.sprites().candidate_lease().unwrap();
        assert_eq!(plane.sprites().user_count(candidate), Some(0));
        candidate
    };
    assert_ne!(failed.user_data, 0);
    assert!(
        !fixture
            .scene
            .events
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );
    assert_eq!(
        applied_cursor_binding(&fixture.scene.events.backend, 0),
        None
    );
    assert_eq!(
        fixture.scene.events.backend.scene.cursor_mode(),
        mode_before
    );
    assert!(!matches!(
        mode_before,
        yserver_render::kms::render::scene::CursorPlaneMode::Sw
    ));
    assert!(!matches!(
        mode_before,
        yserver_render::kms::render::scene::CursorPlaneMode::Hidden
    ));
    assert_eq!(
        fixture
            .scene
            .events
            .backend
            .platform
            .for_tests_atomic_cursor_pending_position(device_key, crtc),
        None
    );

    let offset = record_count(&fixture, device_fd);
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
    .expect("request the forced cursor retry repaint");
    let attach = compose_until_cursor_commit_where(
        &mut fixture,
        &mut delivered,
        0,
        offset,
        properties.fb_id,
        |record| record_property(record, properties.fb_id).is_some_and(|fb| fb != 0),
    );
    assert_ne!(
        attach.user_data, failed.user_data,
        "retry receives a fresh commit ID"
    );
    let plane = fixture.scene.events.backend.platform.devices[0]
        .cursor
        .plane
        .as_ref()
        .unwrap();
    assert_eq!(plane.sprites().user_count(candidate), Some(1));
    retire_cursor_record(&mut fixture, &mut delivered, 0, &attach, 3);
    let detach = accept_cursor_detach(&mut fixture, &mut delivered, 0);
    retire_cursor_record(&mut fixture, &mut delivered, 0, &detach, 4);
    super::support::finish_atomic_fixture(fixture, &mut delivered);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: restore the pending.version < version check; B-created-first must fail the named retry-version assertion.
fn p2_current_cursor_bind_clears_a_superseded_deferred_show_retry() {
    p2_current_cursor_bind_clears_a_superseded_deferred_show_retry_scenario(false);
    p2_current_cursor_bind_clears_a_superseded_deferred_show_retry_scenario(true);
}

fn p2_current_cursor_bind_clears_a_superseded_deferred_show_retry_scenario(
    create_b_before_a: bool,
) {
    let Some(mut fixture) =
        AtomicPathFixture::new(PathKind::Physical).expect("atomic cursor fixture construction")
    else {
        return;
    };
    let cursor_b = create_b_before_a.then(|| {
        create_argb_cursor(
            &mut fixture.scene.events.backend,
            (4, 4),
            [32, 33, 34, 255],
            (1, 2),
        )
    });
    let cursor_a = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [19, 20, 21, 255],
        (1, 2),
    );
    let cursor_a_xid = cursor_a.as_raw();
    define_root_cursor(&mut fixture.scene.events.backend, cursor_a);
    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        100,
        90,
    );
    assert!(matches!(
        fixture.scene.events.backend.scene.cursor_mode(),
        yserver_render::kms::render::scene::CursorPlaneMode::Hidden
            | yserver_render::kms::render::scene::CursorPlaneMode::Sw
    ));
    damage_root(&mut fixture);

    let device_fd = fixture.scene.kms_fd(0);
    let recorder = fixture.scene.events.recorders[&device_fd].clone();
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("submit A's Hidden/Sw-to-Hw scene transition");
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EBUSY)));
    let mut delivered = HashSet::new();
    retire_pending_output(&mut fixture.scene.events, &mut delivered, 0, 1);

    let retry_a = fixture
        .scene
        .events
        .backend
        .scene
        .for_tests_cursor_retry_version(0)
        .expect("A's deferred show leaves a retry version");
    let cursor_b = cursor_b.unwrap_or_else(|| {
        create_argb_cursor(
            &mut fixture.scene.events.backend,
            (4, 4),
            [32, 33, 34, 255],
            (1, 2),
        )
    });
    let cursor_b_xid = cursor_b.as_raw();
    let version_a = fixture.scene.events.backend.cursor_records[&cursor_a_xid].version;
    let version_b = fixture.scene.events.backend.cursor_records[&cursor_b_xid].version;
    assert_eq!(retry_a, version_a, "the deferred retry belongs to A");
    assert_eq!(
        version_b < version_a,
        create_b_before_a,
        "B's cursor version reflects whether B was created before A"
    );
    assert_eq!(
        fixture.scene.events.backend.scene.cursor_mode(),
        yserver_render::kms::render::scene::CursorPlaneMode::Mixed,
        "a deferred Hidden/Sw show keeps the scene out of steady Hw mode"
    );
    assert_eq!(
        fixture.scene.events.backend.effective_cursor_xid,
        Some(cursor_a_xid),
        "A is current when its show is deferred"
    );
    assert_eq!(
        applied_cursor_binding(&fixture.scene.events.backend, 0),
        None,
        "A's deferred show did not bind"
    );

    define_root_cursor(&mut fixture.scene.events.backend, cursor_b);
    assert_eq!(
        fixture.scene.events.backend.effective_cursor_xid,
        Some(cursor_b_xid),
        "select B before retrying A's deferred show"
    );
    assert_eq!(
        fixture
            .scene
            .events
            .backend
            .scene
            .for_tests_cursor_retry_version(0),
        Some(retry_a),
        "B selection is still carrying A's superseded retry marker"
    );

    let offset = record_count(&fixture, device_fd);
    damage_root(&mut fixture);
    let properties = super::support::cursor_properties_for_output(&fixture.scene.events.backend, 0);
    let bind_b = compose_until_cursor_commit_where(
        &mut fixture,
        &mut delivered,
        0,
        offset,
        properties.fb_id,
        |record| record_property(record, properties.fb_id).is_some_and(|fb_id| fb_id != 0),
    );
    assert_eq!(
        fixture
            .scene
            .events
            .backend
            .scene
            .for_tests_cursor_retry_version(0),
        None,
        "successful B bind clears the superseded deferred retry version"
    );
    assert_eq!(
        fixture.scene.events.backend.scene.cursor_mode(),
        yserver_render::kms::render::scene::CursorPlaneMode::Hw,
        "the scene leaves Mixed after current cursor B binds"
    );
    assert!(
        fixture.scene.events.backend.direct_cursor_eligible(),
        "a bound current cursor makes the direct cursor gate eligible"
    );
    assert_eq!(
        applied_cursor_binding(&fixture.scene.events.backend, 0)
            .map(|binding| binding.lease.version()),
        Some(
            fixture.scene.events.backend.platform.devices[0]
                .cursor
                .plane
                .as_ref()
                .unwrap()
                .sprites()
                .candidate_version()
                .expect("B is the current sprite candidate")
        ),
        "the applied binding names the current B sprite"
    );
    retire_cursor_record(&mut fixture, &mut delivered, 0, &bind_b, 3);

    let motion_offset = record_count(&fixture, device_fd);
    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        108,
        96,
    );
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("process pointer motion after the deferred retry settles");
    let motion_records: Vec<_> = recorder
        .records()
        .into_iter()
        .skip(motion_offset)
        .filter(|record| record.device_fd == device_fd && record.result == ScriptedResult::Ok)
        .collect();
    assert!(
        motion_records.iter().all(|record| {
            !matches!(
                record.kind,
                AtomicCommitKind::Scene | AtomicCommitKind::SceneWithCursor
            )
        }),
        "pointer motion after B binds does not force a composed scene frame"
    );
    assert_eq!(
        fixture.scene.events.backend.scene.cursor_mode(),
        yserver_render::kms::render::scene::CursorPlaneMode::Hw
    );

    if let Some(motion) = motion_records
        .iter()
        .find(|record| record.kind == AtomicCommitKind::CursorOnly)
    {
        retire_cursor_record(&mut fixture, &mut delivered, 0, motion, 4);
    }
    let detach = accept_cursor_detach(&mut fixture, &mut delivered, 0);
    retire_cursor_record(&mut fixture, &mut delivered, 0, &detach, 5);
    super::support::finish_atomic_fixture(fixture, &mut delivered);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: call a Legacy hide or keep the attach reservation after its cursor-only submit is rejected.
fn p2_rejected_attach_rolls_back_without_touching_legacy() {
    let Some(mut fixture) =
        AtomicPathFixture::new(PathKind::Physical).expect("atomic cursor fixture construction")
    else {
        return;
    };
    let cursor = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [22, 23, 24, 255],
        (1, 2),
    );
    define_root_cursor(&mut fixture.scene.events.backend, cursor);
    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        100,
        90,
    );
    damage_root(&mut fixture);
    let device_fd = fixture.scene.kms_fd(0);
    let recorder = fixture.scene.events.recorders[&device_fd].clone();
    let start = record_count(&fixture, device_fd);
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("submit scene transition before the scripted cursor result");
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
    let mut delivered = HashSet::new();
    retire_pending_output(&mut fixture.scene.events, &mut delivered, 0, 1);
    let records = recorder.records();
    let rejected = records
        .iter()
        .skip(start)
        .find(|record| {
            record.device_fd == device_fd
                && record.kind == AtomicCommitKind::CursorOnly
                && record.result != ScriptedResult::Ok
        })
        .expect("scripted Atomic attach rejection");
    let device_key = fixture.scene.events.backend.platform.outputs[0]
        .key
        .device_key;
    let crtc = fixture.scene.events.backend.platform.outputs[0].output.crtc;
    let plane = fixture.scene.events.backend.platform.devices[0]
        .cursor
        .plane
        .as_ref()
        .unwrap();
    assert_ne!(rejected.user_data, 0);
    assert_eq!(plane.sprites().prepared_user_count(), 0);
    assert!(
        plane
            .sprites()
            .candidate_lease()
            .is_some_and(|lease| plane.sprites().user_count(lease) == Some(0))
    );
    assert!(
        !fixture
            .scene
            .events
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );
    assert!(records.iter().skip(start).all(|record| {
        record.flags != ::drm::control::AtomicCommitFlags::empty() || !record.events.is_empty()
    }));
    assert_eq!(fixture.legacy.show_calls.get(), 0);
    assert_eq!(fixture.legacy.move_calls.get(), 0);
    assert_eq!(fixture.legacy.hide_calls.get(), 0);
    assert_eq!(
        fixture
            .scene
            .events
            .backend
            .platform
            .for_tests_cursor_fallback_remaining(device_key, crtc),
        Some(1)
    );
    Backend::set_cursor_hidden(&mut fixture.scene.events.backend, true);
    damage_root(&mut fixture);
    let ledger = Rc::clone(&fixture.ledger);
    super::support::finish_atomic_fixture(fixture, &mut delivered);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: roll back a rejected replacement through Legacy hide or release the applied lease before blocking detach succeeds.
fn p2_rejected_replace_detaches_the_applied_plane_and_releases_after_it() {
    let run = |blocking_error: Option<i32>| {
        let Some(mut fixture) =
            AtomicPathFixture::new(PathKind::Physical).expect("atomic cursor fixture construction")
        else {
            return;
        };
        let cursor_a = create_argb_cursor(
            &mut fixture.scene.events.backend,
            (4, 4),
            [25, 26, 27, 255],
            (1, 2),
        );
        let mut delivered = HashSet::new();
        let hardware = establish_hardware_cursor(&mut fixture, &mut delivered, cursor_a, (100, 90));
        let properties =
            super::support::cursor_properties_for_output(&fixture.scene.events.backend, 0);
        let binding_a = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();
        let cursor_b = create_argb_cursor(
            &mut fixture.scene.events.backend,
            (4, 4),
            [28, 29, 30, 255],
            (1, 2),
        );
        define_root_cursor(&mut fixture.scene.events.backend, cursor_b);
        let offset = record_count(&fixture, hardware.device_fd);
        damage_root(&mut fixture);
        Backend::maybe_composite(&mut fixture.scene.events.backend)
            .expect("submit scene transition before scripted cursor rejection");
        let recorder = fixture.scene.events.recorders[&hardware.device_fd].clone();
        recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
        if let Some(raw_error) = blocking_error {
            recorder.script_result(Err(std::io::Error::from_raw_os_error(raw_error)));
        }
        retire_pending_output(&mut fixture.scene.events, &mut delivered, 0, 5);
        let records = recorder.records();
        let failed_show = records
            .iter()
            .skip(offset)
            .find(|record| {
                record.device_fd == hardware.device_fd
                    && record.kind == AtomicCommitKind::CursorOnly
                    && record.result != ScriptedResult::Ok
                    && !record.flags.is_empty()
            })
            .expect("replacement cursor commit rejected with EINVAL");
        assert_ne!(failed_show.user_data, 0);
        let blocking_detach = records
            .iter()
            .skip(offset)
            .find(|record| {
                record.device_fd == hardware.device_fd
                    && record.kind == AtomicCommitKind::CursorOnly
                    && record.flags.is_empty()
                    && record.events.is_empty()
            })
            .cloned();
        let device_key = fixture.scene.events.backend.platform.outputs[0]
            .key
            .device_key;
        let crtc = fixture.scene.events.backend.platform.outputs[0].output.crtc;
        assert_eq!(fixture.legacy.hide_calls.get(), 0);
        assert_eq!(fixture.legacy.show_calls.get(), 0);
        assert_eq!(fixture.legacy.move_calls.get(), 0);
        if blocking_error.is_some() {
            let blocking_detach = blocking_detach.expect("failed blocking Atomic rollback record");
            assert_ne!(blocking_detach.result, ScriptedResult::Ok);
            assert!(blocking_detach.flags.is_empty());
            assert!(blocking_detach.events.is_empty());
            assert_eq!(
                applied_cursor_binding(&fixture.scene.events.backend, 0),
                Some(binding_a)
            );
            assert_eq!(
                sprite_users(&fixture.scene.events.backend, 0, binding_a.lease),
                Some(1)
            );
            assert_eq!(
                fixture
                    .scene
                    .events
                    .backend
                    .platform
                    .for_tests_cursor_hide_recovery_attempts(device_key, crtc),
                Some(0)
            );
        } else {
            let blocking_detach = blocking_detach.expect("successful blocking Atomic rollback");
            assert_eq!(blocking_detach.result, ScriptedResult::Ok);
            assert_eq!(
                blocking_detach.flags,
                ::drm::control::AtomicCommitFlags::empty()
            );
            assert!(blocking_detach.events.is_empty());
            assert_eq!(blocking_detach.properties.len(), 2);
            assert!(
                blocking_detach
                    .properties
                    .iter()
                    .all(|update| update.object == properties.plane.into())
            );
            assert_eq!(record_property(&blocking_detach, properties.fb_id), Some(0));
            assert_eq!(
                record_property(&blocking_detach, properties.crtc_id),
                Some(0)
            );
            assert_eq!(
                applied_cursor_binding(&fixture.scene.events.backend, 0),
                None
            );
            assert_eq!(
                sprite_users(&fixture.scene.events.backend, 0, binding_a.lease),
                Some(0)
            );
        }

        if blocking_error.is_some() {
            let detach = accept_cursor_detach(&mut fixture, &mut delivered, 0);
            retire_cursor_record(&mut fixture, &mut delivered, 0, &detach, 6);
        }
        super::support::finish_atomic_fixture(fixture, &mut delivered);
    };
    run(None);
    run(Some(libc::EIO));
}

fn held_direct_with_cursor(
    present_id: u64,
) -> Option<(
    AtomicPathFixture,
    HashSet<super::support::CompletionKey>,
    super::support::HardwareCursor,
    super::super::commit_identity::recording_scene::DirectPresentSetup,
)> {
    let mut fixture = AtomicPathFixture::new(PathKind::Physical)
        .expect("atomic direct cursor fixture construction")?;
    let cursor = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [31, 32, 33, 255],
        (1, 2),
    );
    let mut delivered = HashSet::new();
    let hardware = establish_hardware_cursor(&mut fixture, &mut delivered, cursor, (100, 90));
    let binding_before_direct =
        applied_cursor_binding(&fixture.scene.events.backend, hardware.output_idx)
            .expect("A is applied before direct scanout");
    let direct = match enter_held_direct(&mut fixture, &mut delivered, present_id) {
        Ok(direct) => direct,
        Err(gate_values) => {
            let detach = accept_cursor_detach(&mut fixture, &mut delivered, hardware.output_idx);
            retire_cursor_record(
                &mut fixture,
                &mut delivered,
                hardware.output_idx,
                &detach,
                29,
            );
            super::support::finish_atomic_fixture(fixture, &mut delivered);
            panic!("{gate_values}");
        }
    };
    assert!(fixture.scene.events.backend.pstate.scanout_m2.active());
    let applied = applied_cursor_binding(&fixture.scene.events.backend, hardware.output_idx)
        .expect("A is shown on the held-direct path");
    assert_eq!(applied, binding_before_direct);
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, applied.lease),
        Some(1)
    );
    Some((fixture, delivered, hardware, direct))
}

fn accept_direct_cursor_replacement(
    fixture: &mut AtomicPathFixture,
    hardware: super::support::HardwareCursor,
    pixel: [u8; 4],
) -> crate::drm::atomic::tests_support::AtomicCommitRecord {
    let binding_a = applied_cursor_binding(&fixture.scene.events.backend, hardware.output_idx)
        .expect("cursor A binding before direct replacement");
    let properties = super::support::cursor_properties_for_output(
        &fixture.scene.events.backend,
        hardware.output_idx,
    );
    let cursor_b = create_argb_cursor(&mut fixture.scene.events.backend, (4, 4), pixel, (1, 2));
    define_root_cursor(&mut fixture.scene.events.backend, cursor_b);
    let record = fixture.scene.events.recorders[&hardware.device_fd]
        .records()
        .into_iter()
        .rev()
        .find(|record| {
            record.device_fd == hardware.device_fd
                && record.result == ScriptedResult::Ok
                && record.kind == AtomicCommitKind::CursorOnly
                && record
                    .events
                    .iter()
                    .any(|event| event.crtc == hardware.crtc)
                && record_property(record, properties.fb_id)
                    .is_some_and(|fb| fb != u64::from(binding_a.framebuffer))
        })
        .expect("held-direct cursor refresh accepts a CursorOnly replacement");
    let binding_b = applied_cursor_binding(&fixture.scene.events.backend, hardware.output_idx)
        .expect("cursor B applied on the held-direct path");
    assert_ne!(binding_a.lease, binding_b.lease);
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, binding_a.lease),
        Some(1)
    );
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, binding_b.lease),
        Some(1)
    );
    record
}

fn drive_requested_unflip(
    fixture: &mut AtomicPathFixture,
    delivered: &mut HashSet<super::support::CompletionKey>,
) {
    for _ in 0..8 {
        Backend::before_block(&mut fixture.scene.events.backend);
        damage_root(fixture);
        Backend::maybe_composite(&mut fixture.scene.events.backend)
            .expect("drive held-direct unflip through Backend");
        drain_unretired_output_events(fixture, delivered, 0);
        if !fixture.scene.events.backend.pstate.scanout_m2.active()
            && !fixture
                .scene
                .events
                .backend
                .pstate
                .scanout_m2
                .unflip_requested
            && !fixture
                .scene
                .events
                .backend
                .platform
                .any_cursor_commit_in_flight()
        {
            return;
        }
    }
    panic!("held-direct cursor refresh did not complete its requested unflip");
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: gate staging behind the cursor-commit retry or overwrite a live sprite slot under pool pressure.
fn p2_direct_refresh_reaches_pool_pressure_and_keeps_only_the_newest_wanted_image() {
    let Some((mut fixture, mut delivered, hardware, _direct)) = held_direct_with_cursor(701) else {
        return;
    };
    let binding_a = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();
    let replacement = accept_direct_cursor_replacement(&mut fixture, hardware, [34, 35, 36, 255]);
    let binding_b = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();
    let bytes_a = sprite_bytes(&fixture.scene.events.backend, 0, binding_a.lease).unwrap();
    let bytes_b = sprite_bytes(&fixture.scene.events.backend, 0, binding_b.lease).unwrap();
    assert_eq!(
        fixture.scene.events.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprites()
            .for_tests_max_buffer_count(),
        2
    );
    assert!(
        fixture
            .scene
            .events
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, binding_a.lease),
        Some(1)
    );
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, binding_b.lease),
        Some(1)
    );

    let cursor_c = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [37, 38, 39, 255],
        (1, 2),
    );
    let cursor_c_id = cursor_c.as_raw();
    define_root_cursor(&mut fixture.scene.events.backend, cursor_c);
    let pool = fixture.scene.events.backend.platform.devices[0]
        .cursor
        .plane
        .as_ref()
        .unwrap()
        .sprites();
    assert_eq!(
        pool.for_tests_last_prepare_outcome(),
        Some(crate::kms::cursor_plane::sprites::SpriteStage::Deferred)
    );
    assert_eq!(
        pool.for_tests_wanted_identity(),
        Some((
            fixture.scene.events.backend.cursor_records[&cursor_c_id].version,
            1,
            2
        ))
    );
    assert!(
        fixture
            .scene
            .events
            .backend
            .pstate
            .scanout_m2
            .unflip_requested
    );
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, binding_a.lease),
        Some(1)
    );
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, binding_b.lease),
        Some(1)
    );
    assert_eq!(
        sprite_bytes(&fixture.scene.events.backend, 0, binding_a.lease),
        Some(bytes_a.clone())
    );
    assert_eq!(
        sprite_bytes(&fixture.scene.events.backend, 0, binding_b.lease),
        Some(bytes_b.clone())
    );

    let cursor_d = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [40, 41, 42, 255],
        (1, 2),
    );
    let cursor_d_id = cursor_d.as_raw();
    define_root_cursor(&mut fixture.scene.events.backend, cursor_d);
    let pool = fixture.scene.events.backend.platform.devices[0]
        .cursor
        .plane
        .as_ref()
        .unwrap()
        .sprites();
    assert_eq!(
        pool.for_tests_last_prepare_outcome(),
        Some(crate::kms::cursor_plane::sprites::SpriteStage::Deferred)
    );
    assert_eq!(
        pool.for_tests_wanted_identity(),
        Some((
            fixture.scene.events.backend.cursor_records[&cursor_d_id].version,
            1,
            2
        ))
    );
    assert!(fixture.scene.events.backend.pstate.scanout_m2.active());
    assert!(
        fixture
            .scene
            .events
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, binding_a.lease),
        Some(1)
    );
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, binding_b.lease),
        Some(1)
    );
    assert_eq!(
        sprite_bytes(&fixture.scene.events.backend, 0, binding_a.lease),
        Some(bytes_a)
    );
    assert_eq!(
        sprite_bytes(&fixture.scene.events.backend, 0, binding_b.lease),
        Some(bytes_b)
    );

    retire_cursor_record(&mut fixture, &mut delivered, 0, &replacement, 9);
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, binding_a.lease),
        Some(0)
    );
    drive_requested_unflip(&mut fixture, &mut delivered);
    let detach = accept_cursor_detach(&mut fixture, &mut delivered, 0);
    retire_cursor_record(&mut fixture, &mut delivered, 0, &detach, 10);
    super::support::finish_atomic_fixture(fixture, &mut delivered);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: report Deferred as direct-refresh success so unflip_requested is never set.
fn p2_direct_hook_treats_deferred_as_failure_and_unflips() {
    let Some((mut fixture, mut delivered, hardware, _direct)) = held_direct_with_cursor(702) else {
        return;
    };
    let binding_a = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();
    let replacement = accept_direct_cursor_replacement(&mut fixture, hardware, [43, 44, 45, 255]);
    let binding_b = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();
    let bytes_a = sprite_bytes(&fixture.scene.events.backend, 0, binding_a.lease).unwrap();
    let bytes_b = sprite_bytes(&fixture.scene.events.backend, 0, binding_b.lease).unwrap();
    let cursor_c = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [46, 47, 48, 255],
        (1, 2),
    );
    define_root_cursor(&mut fixture.scene.events.backend, cursor_c);
    assert_eq!(
        fixture.scene.events.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprites()
            .for_tests_last_prepare_outcome(),
        Some(crate::kms::cursor_plane::sprites::SpriteStage::Deferred)
    );
    assert!(
        fixture
            .scene
            .events
            .backend
            .pstate
            .scanout_m2
            .unflip_requested
    );
    assert!(
        fixture
            .scene
            .events
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, binding_a.lease),
        Some(1)
    );
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, binding_b.lease),
        Some(1)
    );
    assert_eq!(
        sprite_bytes(&fixture.scene.events.backend, 0, binding_a.lease),
        Some(bytes_a)
    );
    assert_eq!(
        sprite_bytes(&fixture.scene.events.backend, 0, binding_b.lease),
        Some(bytes_b)
    );

    retire_cursor_record(&mut fixture, &mut delivered, 0, &replacement, 9);
    drive_requested_unflip(&mut fixture, &mut delivered);
    let detach = accept_cursor_detach(&mut fixture, &mut delivered, 0);
    retire_cursor_record(&mut fixture, &mut delivered, 0, &detach, 10);
    super::support::finish_atomic_fixture(fixture, &mut delivered);
}
