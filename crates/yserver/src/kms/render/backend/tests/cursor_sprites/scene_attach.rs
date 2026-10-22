use super::{
    super::commit_identity::{recording_scene::P1SceneRoute, support::accepted_event_id},
    support::{
        AtomicPathFixture, LegacyPathFixture, PathKind, accept_cursor_detach,
        accepted_live_records, applied_cursor_binding, compose_until_cursor_commit,
        compose_until_cursor_commit_where, create_argb_cursor, damage_root, define_root_cursor,
        deliver_render_completion, drain_unretired_output_events, enter_held_direct,
        establish_hardware_cursor, has_unretired_accepted_event, record_count, record_property,
        retire_cursor_record, retire_pending_output, sprite_bytes, sprite_users,
    },
};
use crate::{
    drm::atomic::{AtomicCommitKind, tests_support::ScriptedResult},
    kms::{
        cursor_plane::ops::recording::CursorBufferOperation, render::platform::ConnectorSnapshot,
    },
};
use std::{
    collections::HashSet,
    rc::Rc,
    time::{Duration, Instant},
};
use yserver_core::backend::Backend;

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: forward ShowOnRetire's captured (x, y) to show; CRTC_X/CRTC_Y must use P1 in both schedules.
fn retire_show_after_motion_uses_current_pointer() {
    for (route, move_before_acceptance) in [
        (P1SceneRoute::CopiedSameDevice, true),
        (P1SceneRoute::Shared, false),
    ] {
        let Some(mut fixture) = AtomicPathFixture::with_scene_route(PathKind::Physical, 1, route)
            .expect("cursor-retirement scene fixture construction")
        else {
            return;
        };
        let mut delivered = HashSet::new();
        if move_before_acceptance {
            // Drain the fixture's initial frame so the tested copied frame has
            // a real source-render completion boundary before KMS acceptance.
            damage_root(&mut fixture);
            Backend::maybe_composite(&mut fixture.scene.events.backend)
                .expect("submit the copied fixture's initial scene frame");
            for job_id in fixture.scene.pending_render_job_ids() {
                deliver_render_completion(&mut fixture, job_id);
            }
            drain_unretired_output_events(&mut fixture, &mut delivered, 0);
            assert!(
                !fixture.scene.events.backend.scene.has_pending_page_flip(0),
                "copied fixture is ready for the tested frame"
            );
        }
        let cursor = create_argb_cursor(
            &mut fixture.scene.events.backend,
            (4, 4),
            [0x12, 0x34, 0x56, 0xff],
            (1, 2),
        );
        define_root_cursor(&mut fixture.scene.events.backend, cursor);
        Backend::warp_pointer_root(
            &mut fixture.scene.events.backend,
            &mut fixture.scene.events.state,
            120,
            80,
        );
        damage_root(&mut fixture);

        let output_idx = 0;
        if move_before_acceptance {
            assert!(matches!(
                fixture.scene.events.backend.platform.scanout_pools[output_idx].as_ref(),
                Some(crate::kms::vk::scanout::OutputScanout::Copied(_))
            ));
        }
        let device_fd = fixture.scene.kms_fd(output_idx);
        let properties =
            super::support::cursor_properties_for_output(&fixture.scene.events.backend, output_idx);
        let record_start = record_count(&fixture, device_fd);

        if move_before_acceptance {
            let deadline = Instant::now() + std::time::Duration::from_secs(5);
            let mut pending_job = None;
            for _ in 0..16 {
                Backend::before_block(&mut fixture.scene.events.backend);
                damage_root(&mut fixture);
                Backend::maybe_composite(&mut fixture.scene.events.backend)
                    .expect("compose copied ShowOnRetire at P0 through Backend");
                pending_job = fixture.scene.pending_render_job_ids().last().copied();
                if pending_job.is_some() {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "copied composition did not register a source render"
                );
            }
            let job_id = pending_job
                .unwrap_or_else(|| {
                    panic!(
                        "copied composition registers a source render within 16 Backend ticks; dirty={}, ready={}, pending_flip={}, cursor_mode={:?}, records={:?}",
                        fixture.scene.events.backend.scene.scene_structure_dirty,
                        fixture.scene.events.backend.scene.output_ready_for_submit(
                            output_idx,
                            &fixture.scene.events.backend.platform,
                        ),
                        fixture.scene.events.backend.scene.has_pending_page_flip(output_idx),
                        fixture.scene.events.backend.scene.cursor_mode(),
                        fixture.scene.events.recorders[&device_fd]
                            .records()
                            .iter()
                            .map(|record| (record.kind, record.result, record.flags))
                            .collect::<Vec<_>>(),
                    )
                });
            assert!(
                !accepted_live_records(&fixture.scene.events.recorders[&device_fd])
                    .iter()
                    .skip(record_start)
                    .any(|record| record.kind == AtomicCommitKind::Scene),
                "copied frame is not accepted before pointer motion"
            );
            Backend::warp_pointer_root(
                &mut fixture.scene.events.backend,
                &mut fixture.scene.events.state,
                180,
                130,
            );
            deliver_render_completion(&mut fixture, job_id);
        } else {
            Backend::maybe_composite(&mut fixture.scene.events.backend)
                .expect("compose shared ShowOnRetire at P0 through Backend");
            assert!(
                accepted_live_records(&fixture.scene.events.recorders[&device_fd])
                    .iter()
                    .skip(record_start)
                    .any(|record| record.kind == AtomicCommitKind::Scene),
                "the shared frame is accepted before pointer motion"
            );
            Backend::warp_pointer_root(
                &mut fixture.scene.events.backend,
                &mut fixture.scene.events.state,
                180,
                130,
            );
        }

        let scene_id =
            retire_pending_output(&mut fixture.scene.events, &mut delivered, output_idx, 1);
        assert_ne!(scene_id, 0, "retirement uses the accepted scene event ID");
        let attach = accepted_live_records(&fixture.scene.events.recorders[&device_fd])
            .into_iter()
            .skip(record_start)
            .find(|record| {
                record.kind == AtomicCommitKind::CursorOnly
                    && record_property(record, properties.fb_id).is_some_and(|fb| fb != 0)
            })
            .expect("retirement submits the hardware cursor attach");
        assert_eq!(record_property(&attach, properties.crtc_x), Some(179));
        assert_eq!(record_property(&attach, properties.crtc_y), Some(128));
        assert_ne!(record_property(&attach, properties.crtc_x), Some(119));
        assert_ne!(record_property(&attach, properties.crtc_y), Some(78));
        assert_eq!(
            retire_pending_output(&mut fixture.scene.events, &mut delivered, output_idx, 2),
            attach.user_data,
            "cursor attach retires by its accepted ID"
        );

        Backend::set_cursor_hidden(&mut fixture.scene.events.backend, true);
        damage_root(&mut fixture);
        Backend::maybe_composite(&mut fixture.scene.events.backend)
            .expect("compose cursor detach through Backend");
        for job_id in fixture.scene.pending_render_job_ids() {
            deliver_render_completion(&mut fixture, job_id);
        }
        super::support::finish_atomic_fixture(fixture, &mut delivered);
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: drop the captured-position fallback when no root pointer was recorded; CRTC_X/CRTC_Y must preserve the compose position.
fn retire_show_without_known_pointer_keeps_captured_position() {
    let Some(mut fixture) =
        AtomicPathFixture::new(PathKind::Physical).expect("cursor-retirement fixture construction")
    else {
        return;
    };
    let mut delivered = HashSet::new();
    let cursor = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [0x12, 0x34, 0x56, 0xff],
        (1, 2),
    );
    define_root_cursor(&mut fixture.scene.events.backend, cursor);
    let captured_root = (
        fixture.scene.events.backend.core.cursor_x as i32,
        fixture.scene.events.backend.core.cursor_y as i32,
    );
    damage_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("compose ShowOnRetire without a pointer dispatch");

    let output_idx = 0;
    let device_fd = fixture.scene.kms_fd(output_idx);
    let properties =
        super::support::cursor_properties_for_output(&fixture.scene.events.backend, output_idx);
    let scene_id = retire_pending_output(&mut fixture.scene.events, &mut delivered, output_idx, 1);
    assert_ne!(scene_id, 0, "retirement uses the accepted scene event ID");
    let attach = accepted_live_records(&fixture.scene.events.recorders[&device_fd])
        .into_iter()
        .find(|record| {
            record.kind == AtomicCommitKind::CursorOnly
                && record_property(record, properties.fb_id).is_some_and(|fb| fb != 0)
        })
        .expect("retirement submits the hardware cursor attach");
    assert_eq!(
        record_property(&attach, properties.crtc_x),
        Some(u64::try_from(captured_root.0 - 1).expect("captured x is nonnegative")),
    );
    assert_eq!(
        record_property(&attach, properties.crtc_y),
        Some(u64::try_from(captured_root.1 - 2).expect("captured y is nonnegative")),
    );
    assert_eq!(
        retire_pending_output(&mut fixture.scene.events, &mut delivered, output_idx, 2),
        attach.user_data,
        "cursor attach retires by its accepted ID"
    );

    Backend::set_cursor_hidden(&mut fixture.scene.events.backend, true);
    damage_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("compose cursor detach through Backend");
    super::support::finish_atomic_fixture(fixture, &mut delivered);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: omit output 1's nonzero origin when translating the latest root pointer to CRTC_X/CRTC_Y.
fn retire_show_uses_output_local_pointer_coordinates() {
    let Some(mut fixture) = AtomicPathFixture::with_output_count(PathKind::Physical, 2)
        .expect("two-output cursor-retirement fixture construction")
    else {
        return;
    };
    let mut delivered = HashSet::new();
    let cursor = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [0x12, 0x34, 0x56, 0xff],
        (1, 2),
    );
    define_root_cursor(&mut fixture.scene.events.backend, cursor);
    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        920,
        100,
    );
    damage_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("compose output 1 ShowOnRetire at P0 through Backend");
    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        980,
        140,
    );

    let output_idx = 1;
    let layout = &fixture.scene.events.backend.platform.outputs[output_idx];
    let (layout_origin, crtc) = ((layout.x, layout.y), layout.output.crtc);
    assert_eq!(layout_origin, (800, 0));
    let device_fd = fixture.scene.kms_fd(output_idx);
    let properties =
        super::support::cursor_properties_for_output(&fixture.scene.events.backend, output_idx);
    let scene_id = retire_pending_output(&mut fixture.scene.events, &mut delivered, output_idx, 1);
    assert_ne!(scene_id, 0, "retirement uses the accepted scene event ID");
    let attach = accepted_live_records(&fixture.scene.events.recorders[&device_fd])
        .into_iter()
        .find(|record| {
            record.kind == AtomicCommitKind::CursorOnly
                && record_property(record, properties.fb_id).is_some_and(|fb| fb != 0)
                && record_property(record, properties.crtc_id) == Some(u64::from(u32::from(crtc)))
        })
        .expect("output 1 retirement submits its hardware cursor attach");
    assert_eq!(record_property(&attach, properties.crtc_x), Some(179));
    assert_eq!(record_property(&attach, properties.crtc_y), Some(138));
    assert_ne!(record_property(&attach, properties.crtc_x), Some(979));
    assert_eq!(
        retire_pending_output(&mut fixture.scene.events, &mut delivered, output_idx, 2),
        attach.user_data,
        "output 1 cursor attach retires by its accepted ID"
    );
    assert_ne!(
        retire_pending_output(&mut fixture.scene.events, &mut delivered, 0, 3),
        0,
        "output 0 scene frame also retires by its accepted ID"
    );

    Backend::set_cursor_hidden(&mut fixture.scene.events.backend, true);
    damage_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("compose output cursor detach through Backend");
    super::support::finish_atomic_fixture(fixture, &mut delivered);
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

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: request an unflip for CursorShowError::Deferred; the direct-state assertion must fail.
fn direct_deferred_cursor_show_keeps_direct_and_retries_on_event() {
    let Some((mut fixture, mut delivered, hardware, _direct)) = held_direct_with_cursor(703) else {
        return;
    };
    let recorder = Rc::clone(&fixture.scene.events.recorders[&hardware.device_fd]);
    let properties = super::support::cursor_properties_for_output(&fixture.scene.events.backend, 0);

    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        140,
        120,
    );
    let move_start = record_count(&fixture, hardware.device_fd);
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("submit the direct cursor position commit");
    let position = accepted_live_records(&recorder)
        .into_iter()
        .skip(move_start)
        .find(|record| {
            record.kind == AtomicCommitKind::CursorOnly
                && record_property(record, properties.crtc_x).is_some()
                && record_property(record, properties.fb_id).is_none()
        })
        .expect("direct scanout accepts a position-only cursor commit");

    let cursor_b = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [49, 50, 51, 255],
        (1, 2),
    );
    let cursor_b_id = cursor_b.as_raw();
    define_root_cursor(&mut fixture.scene.events.backend, cursor_b);
    let version_b = fixture.scene.events.backend.cursor_records[&cursor_b_id].version;
    assert_eq!(
        fixture.scene.events.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprites()
            .for_tests_last_prepare_outcome(),
        Some(crate::kms::cursor_plane::sprites::SpriteStage::Ready),
        "the image is prepared before the in-flight show is deferred"
    );

    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        220,
        170,
    );
    let stayed_direct = fixture.scene.events.backend.pstate.scanout_m2.active()
        && !fixture
            .scene
            .events
            .backend
            .pstate
            .scanout_m2
            .unflip_requested;

    let retry_start = record_count(&fixture, hardware.device_fd);
    retire_cursor_record(&mut fixture, &mut delivered, 0, &position, 11);
    let refreshed = accepted_live_records(&recorder)
        .into_iter()
        .skip(retry_start)
        .rev()
        .find(|record| {
            record.kind == AtomicCommitKind::CursorOnly
                && record_property(record, properties.fb_id).is_some_and(|fb| fb != 0)
                && record_property(record, properties.crtc_x).is_some()
        });
    let retried_latest = refreshed.as_ref().is_some_and(|record| {
        applied_cursor_binding(&fixture.scene.events.backend, 0)
            .is_some_and(|binding| binding.lease.version() == version_b)
            && record_property(record, properties.crtc_x) == Some(219)
            && record_property(record, properties.crtc_y) == Some(168)
    });
    if let Some(refreshed) = refreshed.as_ref() {
        retire_cursor_record(&mut fixture, &mut delivered, 0, refreshed, 12);
    }
    if fixture.scene.events.backend.pstate.scanout_m2.active() {
        fixture
            .scene
            .events
            .backend
            .stop_direct_after_scanout_replaced("test teardown");
    }

    let detach = accept_cursor_detach(&mut fixture, &mut delivered, 0);
    retire_cursor_record(&mut fixture, &mut delivered, 0, &detach, 13);
    super::support::finish_atomic_fixture(fixture, &mut delivered);
    assert!(
        stayed_direct,
        "a deferred cursor show must keep direct scanout active"
    );
    assert!(
        retried_latest,
        "the matching CRTC event retries the live image at the current pointer"
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: treat a real atomic cursor show error as Deferred; this unflip assertion must fail.
fn direct_cursor_real_error_still_unflips() {
    let Some((mut fixture, mut delivered, hardware, _direct)) = held_direct_with_cursor(704) else {
        return;
    };
    let recorder = Rc::clone(&fixture.scene.events.recorders[&hardware.device_fd]);
    let properties = super::support::cursor_properties_for_output(&fixture.scene.events.backend, 0);
    let record_start = record_count(&fixture, hardware.device_fd);
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));

    let cursor_b = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [52, 53, 54, 255],
        (1, 2),
    );
    define_root_cursor(&mut fixture.scene.events.backend, cursor_b);
    let unflipped_on_error = fixture.scene.events.backend.pstate.scanout_m2.active()
        && fixture
            .scene
            .events
            .backend
            .pstate
            .scanout_m2
            .unflip_requested
        && recorder
            .records()
            .into_iter()
            .skip(record_start)
            .any(|record| {
                record.kind == AtomicCommitKind::CursorOnly
                    && record_property(&record, properties.fb_id).is_some_and(|fb| fb != 0)
                    && matches!(
                        record.result,
                        ScriptedResult::Error {
                            raw_os_error: Some(errno),
                            ..
                        } if errno == libc::EIO
                    )
            });

    if fixture.scene.events.backend.pstate.scanout_m2.active() {
        fixture
            .scene
            .events
            .backend
            .stop_direct_after_scanout_replaced("test teardown");
    }
    super::support::finish_atomic_fixture(fixture, &mut delivered);
    assert!(
        unflipped_on_error,
        "real cursor show errors must keep the unflip fallback"
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: omit pending-refresh cleanup from direct teardown; the Backend-driven unflip assertion must fail.
fn direct_pending_refresh_cleared_on_unflip_or_output_removal() {
    {
        let Some((mut fixture, mut delivered, hardware, direct)) = held_direct_with_cursor(705)
        else {
            eprintln!(
                "SKIP direct_pending_refresh_cleared_on_unflip_or_output_removal: Vulkan fixture unavailable"
            );
            return;
        };
        let recorder = Rc::clone(&fixture.scene.events.recorders[&hardware.device_fd]);
        let replacement =
            accept_direct_cursor_replacement(&mut fixture, hardware, [55, 56, 57, 255]);
        let cursor_c = create_argb_cursor(
            &mut fixture.scene.events.backend,
            (4, 4),
            [58, 59, 60, 255],
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
            Some(crate::kms::cursor_plane::sprites::SpriteStage::Deferred),
            "the second live image causes a real deferred refresh"
        );
        assert!(
            fixture
                .scene
                .events
                .backend
                .pstate
                .scanout_m2
                .direct_cursor_refresh_pending
                .contains(&0)
        );

        // An ineligible Present uses the normal Backend path to request an
        // unflip while the cursor commit still owns its CRTC.
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
            .expect("ineligible successor asks Backend to unflip")
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
        retire_cursor_record(&mut fixture, &mut delivered, 0, &replacement, 30);
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
                .pstate
                .scanout_m2
                .direct_cursor_refresh_pending
                .contains(&0)
        );

        let unflip_start = record_count(&fixture, hardware.device_fd);
        Backend::maybe_composite(&mut fixture.scene.events.backend)
            .expect("submit the composed unflip through Backend");
        assert!(
            accepted_live_records(&recorder)
                .into_iter()
                .skip(unflip_start)
                .any(|record| record.kind != AtomicCommitKind::CursorOnly),
            "the Backend path accepts the composed unflip"
        );
        retire_pending_output(&mut fixture.scene.events, &mut delivered, 0, 31);
        assert!(!fixture.scene.events.backend.pstate.scanout_m2.active());
        assert!(
            fixture
                .scene
                .events
                .backend
                .pstate
                .scanout_m2
                .direct_cursor_refresh_pending
                .is_empty(),
            "retiring the Backend unflip clears pending cursor retries"
        );
        assert!(
            !fixture
                .scene
                .events
                .backend
                .platform
                .any_cursor_commit_in_flight()
        );

        let detach = accept_cursor_detach(&mut fixture, &mut delivered, 0);
        retire_cursor_record(&mut fixture, &mut delivered, 0, &detach, 32);
        super::support::finish_atomic_fixture(fixture, &mut delivered);
    }

    {
        let Some((mut fixture, mut delivered, hardware, _direct)) = held_direct_with_cursor(706)
        else {
            eprintln!(
                "SKIP direct_pending_refresh_cleared_on_unflip_or_output_removal: Vulkan fixture unavailable"
            );
            return;
        };
        let replacement =
            accept_direct_cursor_replacement(&mut fixture, hardware, [61, 62, 63, 255]);
        let cursor_c = create_argb_cursor(
            &mut fixture.scene.events.backend,
            (4, 4),
            [64, 65, 66, 255],
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
            Some(crate::kms::cursor_plane::sprites::SpriteStage::Deferred),
            "the output-removal case starts from a real deferred refresh"
        );
        assert!(
            fixture
                .scene
                .events
                .backend
                .pstate
                .scanout_m2
                .direct_cursor_refresh_pending
                .contains(&0)
        );
        fixture.scene.events.queue_commit_completion(
            hardware.device_fd,
            hardware.crtc,
            accepted_event_id(&replacement).expect("replacement has an accepted event ID"),
            33,
            Duration::from_millis(33),
        );

        let mut snapshots: Vec<_> = fixture
            .scene
            .events
            .backend
            .platform
            .outputs
            .iter()
            .map(|output| ConnectorSnapshot {
                key: output.key.clone(),
                modes: vec![output.output.picked.clone()],
                mm_width: output.output.mm_width,
                mm_height: output.output.mm_height,
                edid: output.output.edid.clone(),
                connector_type: output.output.connector_type.clone(),
            })
            .collect();
        snapshots[0].modes.clear();
        super::lifecycle::install_logical_topology(&mut fixture.scene.events.backend, snapshots);
        fixture
            .scene
            .events
            .backend
            .platform
            .for_tests_accept_output_power_commits();
        fixture.scene.events.backend.pstate.hotplug_rescan_deadline =
            Some(Instant::now() - Duration::from_millis(1));
        Backend::poll_deferred_input(
            &mut fixture.scene.events.backend,
            &mut fixture.scene.events.state,
        );

        assert!(fixture.scene.events.backend.platform.outputs.is_empty());
        assert!(!fixture.scene.events.backend.pstate.scanout_m2.active());
        assert!(
            fixture
                .scene
                .events
                .backend
                .pstate
                .scanout_m2
                .direct_cursor_refresh_pending
                .is_empty(),
            "the Backend hotplug removal clears pending cursor retries before output compaction"
        );
        assert!(
            !fixture
                .scene
                .events
                .backend
                .platform
                .any_cursor_commit_in_flight()
        );
        super::support::finish_atomic_fixture(fixture, &mut delivered);
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: skip the CRTC-event retry or overwrite a live sprite slot under pool pressure.
fn p2_direct_refresh_reaches_pool_pressure_and_keeps_only_the_newest_wanted_image() {
    let Some((mut fixture, mut delivered, hardware, _direct)) = held_direct_with_cursor(701) else {
        return;
    };
    let recorder = Rc::clone(&fixture.scene.events.recorders[&hardware.device_fd]);
    let properties = super::support::cursor_properties_for_output(&fixture.scene.events.backend, 0);
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
        !fixture
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
            .pstate
            .scanout_m2
            .direct_cursor_refresh_pending
            .contains(&0)
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

    let retry_start = record_count(&fixture, hardware.device_fd);
    retire_cursor_record(&mut fixture, &mut delivered, 0, &replacement, 9);
    assert!(
        sprite_users(&fixture.scene.events.backend, 0, binding_a.lease)
            .is_none_or(|users| users == 0)
    );
    let refreshed = accepted_live_records(&recorder)
        .into_iter()
        .skip(retry_start)
        .find(|record| {
            record.kind == AtomicCommitKind::CursorOnly
                && record_property(record, properties.fb_id).is_some_and(|fb| fb != 0)
        })
        .expect("the matching CRTC event retries the newest deferred image");
    assert_eq!(
        applied_cursor_binding(&fixture.scene.events.backend, 0)
            .expect("deferred image is applied after the old commit retires")
            .lease
            .version(),
        fixture.scene.events.backend.cursor_records[&cursor_d_id].version
    );
    assert!(fixture.scene.events.backend.pstate.scanout_m2.active());
    assert!(
        !fixture
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
            .pstate
            .scanout_m2
            .direct_cursor_refresh_pending
            .is_empty()
    );
    retire_cursor_record(&mut fixture, &mut delivered, 0, &refreshed, 10);
    fixture
        .scene
        .events
        .backend
        .stop_direct_after_scanout_replaced("test teardown");
    let detach = accept_cursor_detach(&mut fixture, &mut delivered, 0);
    retire_cursor_record(&mut fixture, &mut delivered, 0, &detach, 11);
    super::support::finish_atomic_fixture(fixture, &mut delivered);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: keep Deferred as a direct-refresh failure; the state and retry assertions must fail.
fn p2_direct_hook_keeps_scanout_until_deferred_refresh_retries() {
    let Some((mut fixture, mut delivered, hardware, _direct)) = held_direct_with_cursor(702) else {
        return;
    };
    let recorder = Rc::clone(&fixture.scene.events.recorders[&hardware.device_fd]);
    let properties = super::support::cursor_properties_for_output(&fixture.scene.events.backend, 0);
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
    let cursor_c_id = cursor_c.as_raw();
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
        !fixture
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
            .pstate
            .scanout_m2
            .direct_cursor_refresh_pending
            .contains(&0)
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

    let retry_start = record_count(&fixture, hardware.device_fd);
    retire_cursor_record(&mut fixture, &mut delivered, 0, &replacement, 9);
    let refreshed = accepted_live_records(&recorder)
        .into_iter()
        .skip(retry_start)
        .find(|record| {
            record.kind == AtomicCommitKind::CursorOnly
                && record_property(record, properties.fb_id).is_some_and(|fb| fb != 0)
        })
        .expect("the matching CRTC event retries the deferred cursor image");
    assert_eq!(
        applied_cursor_binding(&fixture.scene.events.backend, 0)
            .expect("cursor refresh is applied")
            .lease
            .version(),
        fixture.scene.events.backend.cursor_records[&cursor_c_id].version
    );
    assert!(fixture.scene.events.backend.pstate.scanout_m2.active());
    assert!(
        !fixture
            .scene
            .events
            .backend
            .pstate
            .scanout_m2
            .unflip_requested
    );
    retire_cursor_record(&mut fixture, &mut delivered, 0, &refreshed, 10);
    fixture
        .scene
        .events
        .backend
        .stop_direct_after_scanout_replaced("test teardown");
    let detach = accept_cursor_detach(&mut fixture, &mut delivered, 0);
    retire_cursor_record(&mut fixture, &mut delivered, 0, &detach, 11);
    super::support::finish_atomic_fixture(fixture, &mut delivered);
}

fn held_direct_with_two_outputs(
    present_id: u64,
) -> Option<(
    AtomicPathFixture,
    HashSet<super::support::CompletionKey>,
    super::support::HardwareCursor,
    super::super::commit_identity::recording_scene::DirectPresentSetup,
)> {
    let mut fixture = AtomicPathFixture::with_output_count(PathKind::Physical, 2)
        .expect("two-output atomic direct cursor fixture construction")?;
    let cursor = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [71, 72, 73, 255],
        (1, 2),
    );
    let mut delivered = HashSet::new();
    let hardware = establish_hardware_cursor(&mut fixture, &mut delivered, cursor, (100, 90));
    drain_unretired_output_events(&mut fixture, &mut delivered, 1);
    let direct = enter_held_direct(&mut fixture, &mut delivered, present_id)
        .expect("enter two-output direct scanout through Backend");
    for output_idx in 0..2 {
        drain_unretired_output_events(&mut fixture, &mut delivered, output_idx);
    }
    assert!(fixture.scene.events.backend.pstate.scanout_m2.active());
    assert!(
        (0..2).all(|output_idx| fixture
            .scene
            .events
            .backend
            .platform
            .cursor_plane_visible_for_output(output_idx)),
        "the direct cursor is bound on both outputs"
    );
    Some((fixture, delivered, hardware, direct))
}

fn count_cursor_commits_for_output(fixture: &AtomicPathFixture, output_idx: usize) -> usize {
    let device_fd = fixture.scene.kms_fd(output_idx);
    let crtc = fixture.scene.events.backend.platform.outputs[output_idx]
        .output
        .crtc;
    accepted_live_records(&fixture.scene.events.recorders[&device_fd])
        .into_iter()
        .filter(|record| {
            record.kind == AtomicCommitKind::CursorOnly
                && record.events.iter().any(|event| event.crtc == crtc)
        })
        .count()
}

fn finish_direct_fixture(
    mut fixture: AtomicPathFixture,
    delivered: &mut HashSet<super::support::CompletionKey>,
) {
    if fixture.scene.events.backend.pstate.scanout_m2.active() {
        fixture
            .scene
            .events
            .backend
            .stop_direct_after_scanout_replaced("test teardown");
    }
    for output_idx in 0..fixture.scene.events.backend.platform.outputs.len() {
        drain_unretired_output_events(&mut fixture, delivered, output_idx);
    }
    for output_idx in 0..fixture.scene.events.backend.platform.outputs.len() {
        if fixture
            .scene
            .events
            .backend
            .platform
            .cursor_plane_visible_for_output(output_idx)
        {
            fixture
                .scene
                .events
                .backend
                .platform
                .cursor_plane_hide_on_crtc(output_idx)
                .expect("detach test cursor during teardown");
        }
    }
    for output_idx in 0..fixture.scene.events.backend.platform.outputs.len() {
        drain_unretired_output_events(&mut fixture, delivered, output_idx);
    }
    super::support::finish_atomic_fixture(fixture, delivered);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: retry through all outputs; the output-1 cursor-commit count must stay unchanged.
fn direct_cursor_retry_on_crtc_event_touches_only_its_pending_output() {
    let Some((mut fixture, mut delivered, hardware, _direct)) = held_direct_with_two_outputs(707)
    else {
        eprintln!(
            "SKIP direct_cursor_retry_on_crtc_event_touches_only_its_pending_output: Vulkan fixture unavailable"
        );
        return;
    };
    let device_fd = hardware.device_fd;
    let properties0 =
        super::support::cursor_properties_for_output(&fixture.scene.events.backend, 0);
    let properties1 =
        super::support::cursor_properties_for_output(&fixture.scene.events.backend, 1);
    let start = accepted_live_records(&fixture.scene.events.recorders[&device_fd]).len();

    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        140,
        120,
    );
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("submit cursor positions on both direct outputs");
    let position0 = accepted_live_records(&fixture.scene.events.recorders[&device_fd])
        .into_iter()
        .skip(start)
        .find(|record| {
            record.kind == AtomicCommitKind::CursorOnly
                && record_property(record, properties0.crtc_x).is_some()
                && record_property(record, properties0.fb_id).is_none()
                && record.events.iter().any(|event| {
                    event.crtc == fixture.scene.events.backend.platform.outputs[0].output.crtc
                })
        })
        .expect("output 0 accepted a position-only cursor commit");
    let position1 = accepted_live_records(&fixture.scene.events.recorders[&device_fd])
        .into_iter()
        .skip(start)
        .find(|record| {
            record.kind == AtomicCommitKind::CursorOnly
                && record_property(record, properties1.crtc_x).is_some()
                && record_property(record, properties1.fb_id).is_none()
                && record.events.iter().any(|event| {
                    event.crtc == fixture.scene.events.backend.platform.outputs[1].output.crtc
                })
        })
        .expect("output 1 accepted a position-only cursor commit");
    retire_cursor_record(&mut fixture, &mut delivered, 1, &position1, 40);

    let cursor_b = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [74, 75, 76, 255],
        (1, 2),
    );
    define_root_cursor(&mut fixture.scene.events.backend, cursor_b);
    assert_eq!(
        fixture
            .scene
            .events
            .backend
            .pstate
            .scanout_m2
            .direct_cursor_refresh_pending,
        HashSet::from([0]),
        "only output 0's in-flight position defers its cursor show"
    );
    let before_output1_event = count_cursor_commits_for_output(&fixture, 1);
    let output1_show = accepted_live_records(&fixture.scene.events.recorders[&device_fd])
        .into_iter()
        .rev()
        .find(|record| {
            record.kind == AtomicCommitKind::CursorOnly
                && record_property(record, properties1.fb_id).is_some_and(|fb| fb != 0)
                && record.events.iter().any(|event| {
                    event.crtc == fixture.scene.events.backend.platform.outputs[1].output.crtc
                })
        })
        .expect("output 1 accepted the new cursor image");
    retire_cursor_record(&mut fixture, &mut delivered, 1, &output1_show, 41);
    let output1_event_did_not_retry_output0 =
        count_cursor_commits_for_output(&fixture, 1) == before_output1_event;
    let output0_remained_pending_after_output1_event = fixture
        .scene
        .events
        .backend
        .pstate
        .scanout_m2
        .direct_cursor_refresh_pending
        .contains(&0);

    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        160,
        140,
    );
    let output0_before_retry = count_cursor_commits_for_output(&fixture, 0);
    let output1_before_retry = count_cursor_commits_for_output(&fixture, 1);
    retire_cursor_record(&mut fixture, &mut delivered, 0, &position0, 42);
    let output0_retry_count = count_cursor_commits_for_output(&fixture, 0) - output0_before_retry;
    let output1_retry_count = count_cursor_commits_for_output(&fixture, 1) - output1_before_retry;
    let pending_cleared_after_retry = fixture
        .scene
        .events
        .backend
        .pstate
        .scanout_m2
        .direct_cursor_refresh_pending
        .is_empty();
    let retry = accepted_live_records(&fixture.scene.events.recorders[&device_fd])
        .into_iter()
        .rev()
        .find(|record| {
            record.kind == AtomicCommitKind::CursorOnly
                && record_property(record, properties0.fb_id).is_some_and(|fb| fb != 0)
                && record.events.iter().any(|event| {
                    event.crtc == fixture.scene.events.backend.platform.outputs[0].output.crtc
                })
        });
    let remained_direct = fixture.scene.events.backend.pstate.scanout_m2.active()
        && !fixture
            .scene
            .events
            .backend
            .pstate
            .scanout_m2
            .unflip_requested;
    finish_direct_fixture(fixture, &mut delivered);
    assert!(
        output1_event_did_not_retry_output0,
        "output 1's CRTC event cannot retry output 0's pending cursor"
    );
    assert!(
        output0_remained_pending_after_output1_event,
        "output 0 remains pending until its own CRTC event"
    );
    assert_eq!(output0_retry_count, 1, "output 0 retries exactly once");
    assert_eq!(output1_retry_count, 0, "output 0 does not touch output 1");
    assert!(pending_cleared_after_retry);
    assert!(retry.is_some(), "output 0 accepts its deferred show");
    assert!(
        remained_direct,
        "cursor refresh deferral keeps direct scanout"
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: retry pending cursor refreshes through all outputs; a CRTC event must exceed its per-output bound.
fn direct_cursor_retries_do_not_delay_direct_successors() {
    let Some((mut fixture, mut delivered, hardware, direct)) = held_direct_with_two_outputs(708)
    else {
        eprintln!(
            "SKIP direct_cursor_retries_do_not_delay_direct_successors: Vulkan fixture unavailable"
        );
        return;
    };
    let device_fd = hardware.device_fd;
    let start = accepted_live_records(&fixture.scene.events.recorders[&device_fd]).len();

    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        150,
        130,
    );
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("submit cursor position commits on each output");
    let positions: Vec<_> = (0..2)
        .map(|output_idx| {
            let properties = super::support::cursor_properties_for_output(
                &fixture.scene.events.backend,
                output_idx,
            );
            let crtc = fixture.scene.events.backend.platform.outputs[output_idx]
                .output
                .crtc;
            accepted_live_records(&fixture.scene.events.recorders[&device_fd])
                .into_iter()
                .skip(start)
                .find(|record| {
                    record.kind == AtomicCommitKind::CursorOnly
                        && record_property(record, properties.crtc_x).is_some()
                        && record_property(record, properties.fb_id).is_none()
                        && record.events.iter().any(|event| event.crtc == crtc)
                })
                .expect("output accepted its cursor position commit")
        })
        .collect();

    let cursor_b = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [77, 78, 79, 255],
        (1, 2),
    );
    define_root_cursor(&mut fixture.scene.events.backend, cursor_b);
    assert_eq!(
        fixture
            .scene
            .events
            .backend
            .pstate
            .scanout_m2
            .direct_cursor_refresh_pending,
        HashSet::from([0, 1]),
        "both in-flight positions defer their replacement shows"
    );

    let mut successor = direct.candidate;
    successor.present_id += 1;
    let mut successor_event = direct.event.clone();
    successor_event.present_id = successor.present_id;
    successor_event.serial = successor_event.serial.wrapping_add(1);
    assert!(
        Backend::try_present_direct(
            &mut fixture.scene.events.backend,
            successor,
            successor_event,
        )
        .expect("queue direct successor through Backend")
    );
    assert!(
        fixture
            .scene
            .events
            .backend
            .pstate
            .scanout_m2
            .queued_successor
            .is_some()
    );
    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        180,
        160,
    );

    let before_first_event = [
        count_cursor_commits_for_output(&fixture, 0),
        count_cursor_commits_for_output(&fixture, 1),
    ];
    let mut first_event_did_not_submit_cursor_retry = true;
    for (output_idx, position) in positions.iter().enumerate() {
        retire_cursor_record(
            &mut fixture,
            &mut delivered,
            output_idx,
            position,
            50 + output_idx as u32,
        );
        if output_idx == 0 {
            first_event_did_not_submit_cursor_retry = [
                count_cursor_commits_for_output(&fixture, 0),
                count_cursor_commits_for_output(&fixture, 1),
            ] == before_first_event;
        }
    }
    let records_after_cursor_events =
        accepted_live_records(&fixture.scene.events.recorders[&device_fd]);
    let direct_successor_record = records_after_cursor_events
        .iter()
        .skip(start)
        .rev()
        .find(|record| record.kind != AtomicCommitKind::CursorOnly);
    let Some(direct_successor_record) = direct_successor_record else {
        finish_direct_fixture(fixture, &mut delivered);
        panic!("queued direct successor is submitted before deferred cursor retries");
    };
    assert_eq!(direct_successor_record.events.len(), 2);
    assert_eq!(
        [
            count_cursor_commits_for_output(&fixture, 0),
            count_cursor_commits_for_output(&fixture, 1),
        ],
        before_first_event,
        "the direct successor event is not preceded by cursor-only retry commits"
    );
    let direct_id = accepted_event_id(direct_successor_record)
        .expect("direct successor carries its accepted commit ID");

    let before_direct_events = [
        count_cursor_commits_for_output(&fixture, 0),
        count_cursor_commits_for_output(&fixture, 1),
    ];
    assert_eq!(
        retire_pending_output(&mut fixture.scene.events, &mut delivered, 0, 52),
        direct_id
    );
    assert_eq!(
        count_cursor_commits_for_output(&fixture, 0) - before_direct_events[0],
        1
    );
    assert_eq!(
        count_cursor_commits_for_output(&fixture, 1) - before_direct_events[1],
        0
    );
    let output0_retry = accepted_live_records(&fixture.scene.events.recorders[&device_fd])
        .into_iter()
        .rev()
        .find(|record| {
            record.kind == AtomicCommitKind::CursorOnly
                && record.events.iter().any(|event| {
                    event.crtc == fixture.scene.events.backend.platform.outputs[0].output.crtc
                })
                && accepted_event_id(record).is_some_and(|id| id != direct_id)
        })
        .expect("direct output 0 event retries only its pending cursor");
    assert!(
        fixture
            .scene
            .events
            .backend
            .pstate
            .scanout_m2
            .direct_cursor_refresh_pending
            .contains(&1)
    );

    let before_output1_event = [
        count_cursor_commits_for_output(&fixture, 0),
        count_cursor_commits_for_output(&fixture, 1),
    ];
    assert_eq!(
        retire_pending_output(&mut fixture.scene.events, &mut delivered, 1, 53),
        direct_id
    );
    assert_eq!(
        count_cursor_commits_for_output(&fixture, 0) - before_output1_event[0],
        0
    );
    assert_eq!(
        count_cursor_commits_for_output(&fixture, 1) - before_output1_event[1],
        1
    );
    assert!(
        fixture
            .scene
            .events
            .backend
            .pstate
            .scanout_m2
            .direct_cursor_refresh_pending
            .is_empty()
    );
    for (output_idx, retry) in [
        (0, output0_retry),
        (
            1,
            accepted_live_records(&fixture.scene.events.recorders[&device_fd])
                .into_iter()
                .rev()
                .find(|record| {
                    record.kind == AtomicCommitKind::CursorOnly
                        && record.events.iter().any(|event| {
                            event.crtc
                                == fixture.scene.events.backend.platform.outputs[1].output.crtc
                        })
                        && accepted_event_id(record).is_some_and(|id| id != direct_id)
                })
                .expect("direct output 1 event retries only its pending cursor"),
        ),
    ] {
        retire_cursor_record(
            &mut fixture,
            &mut delivered,
            output_idx,
            &retry,
            54 + output_idx as u32,
        );
    }
    let still_direct = fixture.scene.events.backend.pstate.scanout_m2.active()
        && !fixture
            .scene
            .events
            .backend
            .pstate
            .scanout_m2
            .unflip_requested;
    finish_direct_fixture(fixture, &mut delivered);
    assert!(
        first_event_did_not_submit_cursor_retry,
        "a queued direct successor prevents cursor-only retries on both CRTCs"
    );
    assert!(still_direct);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: remove the two-event retry bound; the pending show and hide must fail to land.
fn direct_cursor_retry_lands_under_continuous_successor_stream() {
    for hide in [false, true] {
        let Some(mut fixture) = AtomicPathFixture::new(PathKind::Physical)
            .expect("one-output atomic direct cursor fixture construction")
        else {
            eprintln!(
                "SKIP direct_cursor_retry_lands_under_continuous_successor_stream: Vulkan fixture unavailable"
            );
            return;
        };
        let mut delivered = HashSet::new();
        let cursor = create_argb_cursor(
            &mut fixture.scene.events.backend,
            (4, 4),
            [71, 72, 73, 255],
            (1, 2),
        );
        let hardware = establish_hardware_cursor(&mut fixture, &mut delivered, cursor, (100, 90));
        let direct = enter_held_direct(&mut fixture, &mut delivered, if hide { 713 } else { 712 })
            .expect("enter one-output direct scanout through Backend");
        drain_unretired_output_events(&mut fixture, &mut delivered, 0);
        let device_fd = hardware.device_fd;
        let properties =
            super::support::cursor_properties_for_output(&fixture.scene.events.backend, 0);
        let accepted_start =
            accepted_live_records(&fixture.scene.events.recorders[&device_fd]).len();

        Backend::warp_pointer_root(
            &mut fixture.scene.events.backend,
            &mut fixture.scene.events.state,
            140,
            120,
        );
        Backend::maybe_composite(&mut fixture.scene.events.backend)
            .expect("submit in-flight cursor position");
        accepted_live_records(&fixture.scene.events.recorders[&device_fd])
            .into_iter()
            .skip(accepted_start)
            .find(|record| {
                record.kind == AtomicCommitKind::CursorOnly
                    && record_property(record, properties.crtc_x).is_some()
                    && record_property(record, properties.fb_id).is_none()
            })
            .expect("cursor position commit is in flight");

        if hide {
            Backend::set_cursor_hidden(&mut fixture.scene.events.backend, true);
        } else {
            let cursor_b = create_argb_cursor(
                &mut fixture.scene.events.backend,
                (4, 4),
                [74, 75, 76, 255],
                (1, 2),
            );
            define_root_cursor(&mut fixture.scene.events.backend, cursor_b);
        }
        assert!(
            fixture
                .scene
                .events
                .backend
                .pstate
                .scanout_m2
                .direct_cursor_refresh_pending
                .contains(&0)
        );
        assert!(
            !fixture
                .scene
                .events
                .backend
                .pstate
                .scanout_m2
                .unflip_requested
        );

        let mut next_present_id = direct.candidate.present_id.wrapping_add(1);
        let queue_successor = |fixture: &mut AtomicPathFixture, present_id: u64| {
            let mut candidate = direct.candidate;
            candidate.present_id = present_id;
            let mut event = direct.event.clone();
            event.present_id = present_id;
            event.serial = event.serial.wrapping_add(1);
            assert!(
                Backend::try_present_direct(&mut fixture.scene.events.backend, candidate, event,)
                    .expect("queue direct successor through Backend")
            );
            assert!(
                fixture
                    .scene
                    .events
                    .backend
                    .pstate
                    .scanout_m2
                    .queued_successor
                    .is_some()
            );
        };
        let direct_count = |fixture: &AtomicPathFixture| {
            accepted_live_records(&fixture.scene.events.recorders[&device_fd])
                .into_iter()
                .filter(|record| record.kind != AtomicCommitKind::CursorOnly)
                .count()
        };
        let cursor_count =
            |fixture: &AtomicPathFixture| count_cursor_commits_for_output(fixture, 0);

        for event_idx in 0..5 {
            if fixture
                .scene
                .events
                .backend
                .pstate
                .scanout_m2
                .queued_successor
                .is_none()
            {
                queue_successor(&mut fixture, next_present_id);
                next_present_id = next_present_id.wrapping_add(1);
            }
            assert!(
                fixture
                    .scene
                    .events
                    .backend
                    .pstate
                    .scanout_m2
                    .queued_successor
                    .is_some()
            );
            let direct_before = direct_count(&fixture);
            let cursor_before = cursor_count(&fixture);
            retire_pending_output(&mut fixture.scene.events, &mut delivered, 0, 60 + event_idx);
            let new_direct = direct_count(&fixture) - direct_before;
            let new_cursor = cursor_count(&fixture) - cursor_before;
            assert!(
                new_direct > 0
                    || new_cursor > 0
                    || fixture
                        .scene
                        .events
                        .backend
                        .pstate
                        .scanout_m2
                        .queued_successor
                        .is_some(),
                "event {event_idx} advances a successor or the bounded cursor retry"
            );
            if event_idx == 0 {
                assert_eq!(
                    new_direct, 1,
                    "the first postponed event submits its successor"
                );
                assert_eq!(new_cursor, 0, "direct frame keeps priority once");
                assert!(
                    fixture
                        .scene
                        .events
                        .backend
                        .pstate
                        .scanout_m2
                        .direct_cursor_refresh_pending
                        .contains(&0)
                );
            }
            if event_idx == 1 {
                assert_eq!(new_direct, 0);
                assert_eq!(
                    new_cursor, 1,
                    "retry lands after exactly two postponed events"
                );
                assert!(
                    fixture
                        .scene
                        .events
                        .backend
                        .pstate
                        .scanout_m2
                        .direct_cursor_refresh_pending
                        .is_empty()
                );
                assert!(
                    accepted_live_records(&fixture.scene.events.recorders[&device_fd])
                        .into_iter()
                        .skip(accepted_start)
                        .any(|record| {
                            record.kind == AtomicCommitKind::CursorOnly
                                && record_property(&record, properties.fb_id)
                                    .is_some_and(|fb_id| if hide { fb_id == 0 } else { fb_id != 0 })
                        }),
                    "bounded retry accepted the requested cursor state"
                );
            }
            if event_idx >= 2 {
                assert!(new_direct > 0, "successors continue after the cursor retry");
            }
        }
        assert!(fixture.scene.events.backend.pstate.scanout_m2.active());
        assert!(
            !fixture
                .scene
                .events
                .backend
                .pstate
                .scanout_m2
                .unflip_requested
        );

        fixture
            .scene
            .events
            .backend
            .stop_direct_after_scanout_replaced("continuous cursor retry test teardown");
        drain_unretired_output_events(&mut fixture, &mut delivered, 0);
        if fixture
            .scene
            .events
            .backend
            .platform
            .cursor_plane_visible_for_output(0)
        {
            fixture
                .scene
                .events
                .backend
                .platform
                .cursor_plane_hide_on_crtc(0)
                .expect("detach test cursor during teardown");
        }
        drain_unretired_output_events(&mut fixture, &mut delivered, 0);
        assert!(
            fixture
                .scene
                .events
                .backend
                .pstate
                .scanout_m2
                .direct_cursor_refresh_pending
                .is_empty()
        );
        assert!(
            fixture
                .scene
                .events
                .backend
                .pstate
                .scanout_m2
                .direct_cursor_refresh_successor_deferrals
                .is_empty()
        );
        super::support::assert_no_cursor_resources(&fixture.scene.events.backend, None);
        super::support::finish_atomic_fixture(fixture, &mut delivered);
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: treat a deferred direct hide as a failure; both direct outputs must remain active and pending.
fn direct_deferred_cursor_hide_keeps_direct_and_retries_on_event() {
    let Some((mut fixture, mut delivered, hardware, _direct)) = held_direct_with_two_outputs(709)
    else {
        eprintln!(
            "SKIP direct_deferred_cursor_hide_keeps_direct_and_retries_on_event: Vulkan fixture unavailable"
        );
        return;
    };
    let device_fd = hardware.device_fd;
    let properties: Vec<_> = (0..2)
        .map(|output_idx| {
            super::support::cursor_properties_for_output(&fixture.scene.events.backend, output_idx)
        })
        .collect();
    assert_eq!(device_fd, fixture.scene.kms_fd(1));
    let start = record_count(&fixture, device_fd);
    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        140,
        120,
    );
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("submit in-flight cursor positions on both direct outputs");
    let positions: Vec<_> = (0..2)
        .map(|output_idx| {
            let crtc = fixture.scene.events.backend.platform.outputs[output_idx]
                .output
                .crtc;
            accepted_live_records(&fixture.scene.events.recorders[&device_fd])
                .into_iter()
                .skip(start)
                .find(|record| {
                    record.kind == AtomicCommitKind::CursorOnly
                        && record_property(record, properties[output_idx].crtc_x).is_some()
                        && record_property(record, properties[output_idx].fb_id).is_none()
                        && record.events.iter().any(|event| event.crtc == crtc)
                })
                .expect("direct output accepted a cursor position commit")
        })
        .collect();

    let before_hide = [
        count_cursor_commits_for_output(&fixture, 0),
        count_cursor_commits_for_output(&fixture, 1),
    ];
    Backend::set_cursor_hidden(&mut fixture.scene.events.backend, true);
    let deferred_hide_kept_direct = fixture.scene.events.backend.pstate.scanout_m2.active()
        && !fixture
            .scene
            .events
            .backend
            .pstate
            .scanout_m2
            .unflip_requested;
    let pending_after_hide = fixture
        .scene
        .events
        .backend
        .pstate
        .scanout_m2
        .direct_cursor_refresh_pending
        .clone();
    let hide_deferred_without_second_commit = [
        count_cursor_commits_for_output(&fixture, 0),
        count_cursor_commits_for_output(&fixture, 1),
    ] == before_hide;

    retire_cursor_record(&mut fixture, &mut delivered, 0, &positions[0], 60);
    let counts_after_output0_event = [
        count_cursor_commits_for_output(&fixture, 0),
        count_cursor_commits_for_output(&fixture, 1),
    ];
    let hide0 = accepted_live_records(&fixture.scene.events.recorders[&device_fd])
        .into_iter()
        .skip(start)
        .rev()
        .find(|record| {
            record.kind == AtomicCommitKind::CursorOnly
                && record_property(record, properties[0].fb_id) == Some(0)
                && record.events.iter().any(|event| {
                    event.crtc == fixture.scene.events.backend.platform.outputs[0].output.crtc
                })
        });
    let output0_retry_is_scoped = counts_after_output0_event[0] - before_hide[0] == 1
        && counts_after_output0_event[1] - before_hide[1] == 0;
    let pending_after_output0_event = fixture
        .scene
        .events
        .backend
        .pstate
        .scanout_m2
        .direct_cursor_refresh_pending
        .clone();
    let visibility_after_output0_event = [
        fixture
            .scene
            .events
            .backend
            .platform
            .cursor_plane_visible_for_output(0),
        fixture
            .scene
            .events
            .backend
            .platform
            .cursor_plane_visible_for_output(1),
    ];
    if let Some(hide) = &hide0 {
        retire_cursor_record(&mut fixture, &mut delivered, 0, hide, 61);
    }

    retire_cursor_record(&mut fixture, &mut delivered, 1, &positions[1], 62);
    let counts_after_output1_event = [
        count_cursor_commits_for_output(&fixture, 0),
        count_cursor_commits_for_output(&fixture, 1),
    ];
    let hide1 = accepted_live_records(&fixture.scene.events.recorders[&device_fd])
        .into_iter()
        .skip(start)
        .rev()
        .find(|record| {
            record.kind == AtomicCommitKind::CursorOnly
                && record_property(record, properties[1].fb_id) == Some(0)
                && record.events.iter().any(|event| {
                    event.crtc == fixture.scene.events.backend.platform.outputs[1].output.crtc
                })
        });
    if let Some(hide) = &hide1 {
        retire_cursor_record(&mut fixture, &mut delivered, 1, hide, 63);
    }
    let pending_after_output1_event = fixture
        .scene
        .events
        .backend
        .pstate
        .scanout_m2
        .direct_cursor_refresh_pending
        .clone();
    let hidden_after_both_events = (0..2).all(|output_idx| {
        !fixture
            .scene
            .events
            .backend
            .platform
            .cursor_plane_visible_for_output(output_idx)
    });
    let no_cursor_commit_in_flight = !fixture
        .scene
        .events
        .backend
        .platform
        .any_cursor_commit_in_flight();
    let hide_retries_are_scoped = counts_after_output1_event[0] - counts_after_output0_event[0]
        == 0
        && counts_after_output1_event[1] - counts_after_output0_event[1] == 1;
    let still_direct = fixture.scene.events.backend.pstate.scanout_m2.active()
        && !fixture
            .scene
            .events
            .backend
            .pstate
            .scanout_m2
            .unflip_requested;
    finish_direct_fixture(fixture, &mut delivered);
    assert!(
        deferred_hide_kept_direct,
        "a deferred hide keeps direct scanout"
    );
    assert!(
        pending_after_hide == HashSet::from([0, 1]),
        "both outputs record the deferred hide"
    );
    assert!(hide_deferred_without_second_commit);
    assert!(
        output0_retry_is_scoped,
        "output 0's event hides only output 0"
    );
    assert_eq!(pending_after_output0_event, HashSet::from([1]));
    assert_eq!(visibility_after_output0_event, [false, true]);
    assert!(hide0.is_some(), "output 0 accepted its deferred hide");
    assert!(
        hide_retries_are_scoped,
        "output 1's event hides only output 1"
    );
    assert_eq!(pending_after_output1_event, HashSet::new());
    assert!(hide1.is_some(), "output 1 accepted its deferred hide");
    assert!(hidden_after_both_events);
    assert!(no_cursor_commit_in_flight);
    assert!(
        still_direct,
        "successful deferred hides preserve direct scanout"
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: replay the deferred hide instead of the current shown intent; both cursors must remain visible.
fn direct_deferred_hide_then_show_applies_newest_intent() {
    let Some((mut fixture, mut delivered, hardware, _direct)) = held_direct_with_two_outputs(710)
    else {
        eprintln!(
            "SKIP direct_deferred_hide_then_show_applies_newest_intent: Vulkan fixture unavailable"
        );
        return;
    };
    let device_fd = hardware.device_fd;
    let properties: Vec<_> = (0..2)
        .map(|output_idx| {
            super::support::cursor_properties_for_output(&fixture.scene.events.backend, output_idx)
        })
        .collect();
    let start = record_count(&fixture, device_fd);
    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        140,
        120,
    );
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("submit in-flight cursor positions on both direct outputs");
    let positions: Vec<_> = (0..2)
        .map(|output_idx| {
            let crtc = fixture.scene.events.backend.platform.outputs[output_idx]
                .output
                .crtc;
            accepted_live_records(&fixture.scene.events.recorders[&device_fd])
                .into_iter()
                .skip(start)
                .find(|record| {
                    record.kind == AtomicCommitKind::CursorOnly
                        && record_property(record, properties[output_idx].crtc_x).is_some()
                        && record_property(record, properties[output_idx].fb_id).is_none()
                        && record.events.iter().any(|event| event.crtc == crtc)
                })
                .expect("direct output accepted a cursor position commit")
        })
        .collect();

    Backend::set_cursor_hidden(&mut fixture.scene.events.backend, true);
    let hide_deferred_on_both = fixture
        .scene
        .events
        .backend
        .pstate
        .scanout_m2
        .direct_cursor_refresh_pending
        == HashSet::from([0, 1]);
    Backend::set_cursor_hidden(&mut fixture.scene.events.backend, false);
    let shown_intent_is_current = !fixture.scene.events.backend.effective_cursor_hidden()
        && !fixture
            .scene
            .events
            .backend
            .pstate
            .scanout_m2
            .unflip_requested;

    let before_retry = [
        count_cursor_commits_for_output(&fixture, 0),
        count_cursor_commits_for_output(&fixture, 1),
    ];
    let retry_start = record_count(&fixture, device_fd);
    retire_cursor_record(&mut fixture, &mut delivered, 0, &positions[0], 62);
    let after_output0_event = [
        count_cursor_commits_for_output(&fixture, 0),
        count_cursor_commits_for_output(&fixture, 1),
    ];
    let pending_after_output0_event = fixture
        .scene
        .events
        .backend
        .pstate
        .scanout_m2
        .direct_cursor_refresh_pending
        .clone();
    retire_cursor_record(&mut fixture, &mut delivered, 1, &positions[1], 63);
    let after_output1_event = [
        count_cursor_commits_for_output(&fixture, 0),
        count_cursor_commits_for_output(&fixture, 1),
    ];
    let pending_after_output1_event = fixture
        .scene
        .events
        .backend
        .pstate
        .scanout_m2
        .direct_cursor_refresh_pending
        .clone();
    let cursor_remained_visible = (0..2).all(|output_idx| {
        fixture
            .scene
            .events
            .backend
            .platform
            .cursor_plane_visible_for_output(output_idx)
    });
    let stale_hide_submitted = accepted_live_records(&fixture.scene.events.recorders[&device_fd])
        .into_iter()
        .skip(retry_start)
        .any(|record| {
            record.kind == AtomicCommitKind::CursorOnly
                && properties
                    .iter()
                    .any(|property| record_property(&record, property.fb_id) == Some(0))
        });
    let retry_commits_are_scoped =
        after_output0_event == before_retry && after_output1_event == after_output0_event;
    let no_cursor_commit_in_flight = !fixture
        .scene
        .events
        .backend
        .platform
        .any_cursor_commit_in_flight();
    let still_direct = fixture.scene.events.backend.pstate.scanout_m2.active()
        && !fixture
            .scene
            .events
            .backend
            .pstate
            .scanout_m2
            .unflip_requested;
    finish_direct_fixture(fixture, &mut delivered);
    assert!(hide_deferred_on_both);
    assert!(shown_intent_is_current);
    assert_eq!(
        after_output0_event, before_retry,
        "output 0 needs no show commit"
    );
    assert_eq!(
        after_output1_event, after_output0_event,
        "output 1 needs no show commit"
    );
    assert_eq!(pending_after_output0_event, HashSet::from([1]));
    assert!(pending_after_output1_event.is_empty());
    assert!(retry_commits_are_scoped);
    assert!(cursor_remained_visible);
    assert!(
        !stale_hide_submitted,
        "the event does not apply the stale hide"
    );
    assert!(no_cursor_commit_in_flight);
    assert!(still_direct);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: treat a real hide error as deferred; output 0 must request unflip while output 1 hides.
fn direct_cursor_hide_real_error_still_unflips() {
    let Some((mut fixture, mut delivered, hardware, _direct)) = held_direct_with_two_outputs(711)
    else {
        eprintln!("SKIP direct_cursor_hide_real_error_still_unflips: Vulkan fixture unavailable");
        return;
    };
    let device_fd = hardware.device_fd;
    let properties0 =
        super::support::cursor_properties_for_output(&fixture.scene.events.backend, 0);
    let properties1 =
        super::support::cursor_properties_for_output(&fixture.scene.events.backend, 1);
    let recorder = Rc::clone(&fixture.scene.events.recorders[&device_fd]);
    let start = record_count(&fixture, device_fd);
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
    Backend::set_cursor_hidden(&mut fixture.scene.events.backend, true);
    let real_error = recorder.records().into_iter().skip(start).any(|record| {
        record.kind == AtomicCommitKind::CursorOnly
            && record_property(&record, properties0.fb_id) == Some(0)
            && matches!(
                record.result,
                ScriptedResult::Error {
                    raw_os_error: Some(errno),
                    ..
                } if errno == libc::EIO
            )
    });
    let still_requested_unflip = fixture.scene.events.backend.pstate.scanout_m2.active()
        && fixture
            .scene
            .events
            .backend
            .pstate
            .scanout_m2
            .unflip_requested;
    let no_deferred_retry = fixture
        .scene
        .events
        .backend
        .pstate
        .scanout_m2
        .direct_cursor_refresh_pending
        .is_empty();
    let output1_hide_succeeded = recorder.records().into_iter().skip(start).any(|record| {
        record.kind == AtomicCommitKind::CursorOnly
            && record_property(&record, properties1.fb_id) == Some(0)
            && record.events.iter().any(|event| {
                event.crtc == fixture.scene.events.backend.platform.outputs[1].output.crtc
            })
            && record.result == ScriptedResult::Ok
    });
    let output1_is_hidden = !fixture
        .scene
        .events
        .backend
        .platform
        .cursor_plane_visible_for_output(1);
    finish_direct_fixture(fixture, &mut delivered);
    assert!(
        real_error,
        "the scripted EIO reached the atomic cursor hide"
    );
    assert!(
        still_requested_unflip,
        "a real cursor hide error still requests unflip"
    );
    assert!(no_deferred_retry, "a real error is not treated as deferred");
    assert!(output1_hide_succeeded, "output 1 still accepts its hide");
    assert!(output1_is_hidden, "output 1's accepted hide is reflected");
}
