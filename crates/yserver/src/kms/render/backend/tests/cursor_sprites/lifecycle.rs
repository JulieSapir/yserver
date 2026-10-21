use super::support::{
    AtomicPathFixture, CompletionKey, PathKind, accept_cursor_detach, accepted_live_records,
    applied_cursor_binding, assert_no_cursor_resources, compose_until_cursor_commit_where,
    create_argb_cursor, define_root_cursor, finish_atomic_fixture, has_unretired_accepted_event,
    record_count, record_property, retire_cursor_record, retire_pending_output, sprite_users,
};
use crate::{
    drm::{
        atomic::{
            AtomicCommitKind,
            tests_support::{AtomicCommitRecord, ScriptedResult},
        },
        modeset::Output,
    },
    kms::render::{
        backend::{KmsBackend, kms::session::VtResumeExecutor},
        platform::{ConnectorSnapshot, power::OutputPowerExecutor},
    },
};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    io,
    os::fd::{AsFd as _, AsRawFd as _, RawFd},
    rc::Rc,
    time::{Duration, Instant},
};
use yserver_core::backend::Backend;

fn fixture(output_count: usize) -> Option<AtomicPathFixture> {
    AtomicPathFixture::with_output_count(PathKind::Physical, output_count)
        .expect("Atomic lifecycle fixture construction")
}

fn damage_root(backend: &mut KmsBackend) {
    let root = backend.core.window_id;
    let width = backend.platform.fb_w;
    let height = backend.platform.fb_h;
    Backend::clear_area(backend, None, root, 0, None, 0, 0, width, height, (0, 0))
        .expect("damage root through Backend");
}

fn install_logical_topology(backend: &mut KmsBackend, snapshots: Vec<ConnectorSnapshot>) {
    backend.for_tests_set_vt_resume_executor(Rc::new(StaticTopology { snapshots }));
}

struct StaticTopology {
    snapshots: Vec<ConnectorSnapshot>,
}

impl VtResumeExecutor for StaticTopology {
    fn acquire_master(&self, _device: &crate::kms::render::platform::KmsDevice) -> io::Result<()> {
        Ok(())
    }

    fn release_master(&self, _device: &crate::kms::render::platform::KmsDevice) -> io::Result<()> {
        Ok(())
    }

    fn probe_connector_snapshot(
        &self,
        _platform: &crate::kms::render::platform::PlatformBackend,
    ) -> io::Result<Vec<ConnectorSnapshot>> {
        Ok(self.snapshots.clone())
    }
}

fn current_snapshots(backend: &KmsBackend) -> Vec<ConnectorSnapshot> {
    backend
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
        .collect()
}

fn device_fd(backend: &KmsBackend, output_idx: usize) -> RawFd {
    let key = backend.platform.outputs[output_idx].key.device_key;
    backend
        .platform
        .device_for_key(key)
        .expect("test output has a KMS device")
        .device
        .as_fd()
        .as_raw_fd()
}

fn live_cursor_record(
    fixture: &AtomicPathFixture,
    output_idx: usize,
    property: ::drm::control::property::Handle,
    value: u64,
) -> AtomicCommitRecord {
    let fd = device_fd(&fixture.scene.events.backend, output_idx);
    accepted_live_records(&fixture.scene.events.recorders[&fd])
        .into_iter()
        .find(|record| {
            record.kind == AtomicCommitKind::CursorOnly
                && record.events.iter().any(|event| {
                    event.crtc
                        == fixture.scene.events.backend.platform.outputs[output_idx]
                            .output
                            .crtc
                })
                && record_property(record, property) == Some(value)
        })
        .expect("accepted cursor-only record")
}

fn live_cursor_record_after(
    fixture: &AtomicPathFixture,
    output_idx: usize,
    accepted_offset: usize,
    property: ::drm::control::property::Handle,
    value: u64,
) -> AtomicCommitRecord {
    let fd = device_fd(&fixture.scene.events.backend, output_idx);
    accepted_live_records(&fixture.scene.events.recorders[&fd])
        .into_iter()
        .skip(accepted_offset)
        .find(|record| {
            record.kind == AtomicCommitKind::CursorOnly
                && record.events.iter().any(|event| {
                    event.crtc
                        == fixture.scene.events.backend.platform.outputs[output_idx]
                            .output
                            .crtc
                })
                && record_property(record, property) == Some(value)
        })
        .expect("new accepted cursor-only record")
}

fn eventless_detach_records(
    fixture: &AtomicPathFixture,
    output_idx: usize,
) -> Vec<AtomicCommitRecord> {
    let fd = device_fd(&fixture.scene.events.backend, output_idx);
    let properties =
        super::support::cursor_properties_for_output(&fixture.scene.events.backend, output_idx);
    fixture.scene.events.recorders[&fd]
        .records()
        .into_iter()
        .filter(|record| {
            record.kind == AtomicCommitKind::CursorOnly
                && record.flags.is_empty()
                && record.events.is_empty()
                && record_property(record, properties.fb_id) == Some(0)
                && record_property(record, properties.crtc_id) == Some(0)
        })
        .collect()
}

fn assert_full_attach(
    record: &AtomicCommitRecord,
    binding: crate::kms::render::platform::cursor_state::CursorBinding,
    crtc: ::drm::control::crtc::Handle,
) {
    let properties = binding.properties;
    let expected = [
        properties.fb_id,
        properties.crtc_id,
        properties.crtc_x,
        properties.crtc_y,
        properties.crtc_w,
        properties.crtc_h,
        properties.src_x,
        properties.src_y,
        properties.src_w,
        properties.src_h,
    ];
    for property in expected {
        assert!(
            record.properties.iter().any(|update| {
                update.object == properties.plane.into() && update.property == property
            }),
            "full attach omitted property {property:?}: {:?}",
            record.properties,
        );
    }
    for (property, value) in [
        (properties.fb_id, u64::from(binding.framebuffer)),
        (properties.crtc_id, u64::from(u32::from(crtc))),
        (properties.crtc_x, i64::from(binding.x) as u64),
        (properties.crtc_y, i64::from(binding.y) as u64),
        (properties.crtc_w, 64),
        (properties.crtc_h, 64),
        (properties.src_x, 0),
        (properties.src_y, 0),
        (properties.src_w, 64_u64 << 16),
        (properties.src_h, 64_u64 << 16),
    ] {
        assert_eq!(record_property(record, property), Some(value));
    }
    if let Some(property) = properties.hotspot_x {
        assert_eq!(
            record_property(record, property),
            Some(u64::from(binding.hot_x))
        );
    }
    if let Some(property) = properties.hotspot_y {
        assert_eq!(
            record_property(record, property),
            Some(u64::from(binding.hot_y))
        );
    }
}

fn establish_two_overlapping_cursors(
    fixture: &mut AtomicPathFixture,
    delivered: &mut HashSet<CompletionKey>,
) -> [crate::kms::render::platform::cursor_state::CursorBinding; 2] {
    let cursor = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [0x72, 0x31, 0x8a, 0xff],
        (0, 0),
    );
    define_root_cursor(&mut fixture.scene.events.backend, cursor);
    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        799,
        100,
    );
    damage_root(&mut fixture.scene.events.backend);

    for frame in 1..=12 {
        Backend::maybe_composite(&mut fixture.scene.events.backend)
            .expect("compose cursor on both CRTCs through Backend");
        for output_idx in 0..2 {
            if has_unretired_accepted_event(&fixture.scene.events, delivered, output_idx) {
                retire_pending_output(&mut fixture.scene.events, delivered, output_idx, frame);
            }
        }
        let bindings = [
            applied_cursor_binding(&fixture.scene.events.backend, 0),
            applied_cursor_binding(&fixture.scene.events.backend, 1),
        ];
        if let [Some(a), Some(b)] = bindings
            && !(0..2).any(|output_idx| {
                fixture
                    .scene
                    .events
                    .backend
                    .platform
                    .cursor_commit_in_flight_for_output(output_idx)
            })
        {
            assert_eq!(
                a.lease, b.lease,
                "overlapping outputs share the sprite lease"
            );
            return [a, b];
        }
    }
    panic!("cursor was not established on both overlapping CRTCs");
}

fn record_write_count(fixture: &AtomicPathFixture) -> usize {
    fixture.ledger.write_count()
}

fn state_for_output(
    backend: &KmsBackend,
    output_idx: usize,
) -> &crate::kms::render::platform::cursor_state::CursorCrtcState {
    let output = &backend.platform.outputs[output_idx];
    backend
        .platform
        .device_for_key(output.key.device_key)
        .expect("test output device")
        .cursor
        .crtc_states
        .get(&output.output.crtc)
        .expect("cursor CRTC state")
}

fn output_index_for_crtc(backend: &KmsBackend, crtc: ::drm::control::crtc::Handle) -> usize {
    backend
        .platform
        .outputs
        .iter()
        .position(|output| output.output.crtc == crtc)
        .expect("live output for CRTC")
}

#[derive(Clone)]
struct DisableObservation {
    /// `None` for a device-wide all-off commit.
    crtc: Option<::drm::control::crtc::Handle>,
    records: Vec<AtomicCommitRecord>,
}

struct RecordingOutputPowerExecutor {
    recorder_by_fd:
        HashMap<RawFd, Rc<crate::drm::atomic::tests_support::RecordingAtomicCommitExecutor>>,
    disabled: Rc<RefCell<Vec<DisableObservation>>>,
}

impl OutputPowerExecutor for RecordingOutputPowerExecutor {
    fn disable(&self, device: &crate::drm::Device, output: &Output) -> io::Result<()> {
        let fd = device.as_fd().as_raw_fd();
        self.disabled.borrow_mut().push(DisableObservation {
            crtc: Some(output.crtc),
            records: self
                .recorder_by_fd
                .get(&fd)
                .expect("recording atomic executor for output device")
                .records(),
        });
        Ok(())
    }

    fn disable_device(&self, device: &crate::drm::Device) -> io::Result<()> {
        let fd = device.as_fd().as_raw_fd();
        self.disabled.borrow_mut().push(DisableObservation {
            crtc: None,
            records: self
                .recorder_by_fd
                .get(&fd)
                .expect("recording atomic executor for device")
                .records(),
        });
        Ok(())
    }

    fn enable(
        &self,
        _device: &crate::drm::Device,
        _output: &Output,
        _framebuffer: ::drm::control::framebuffer::Handle,
    ) -> io::Result<()> {
        Ok(())
    }
}

fn install_recording_power_executor(
    fixture: &mut AtomicPathFixture,
) -> Rc<RefCell<Vec<DisableObservation>>> {
    let disabled = Rc::new(RefCell::new(Vec::new()));
    let executor = RecordingOutputPowerExecutor {
        recorder_by_fd: fixture.scene.events.recorders.clone(),
        disabled: Rc::clone(&disabled),
    };
    fixture.scene.events.backend.platform.output_power_executor = Rc::new(executor);
    disabled
}

fn finish_vt_release(fixture: &mut AtomicPathFixture) {
    Backend::begin_vt_release(&mut fixture.scene.events.backend);
    Backend::finish_vt_release(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        &yserver_core::core_loop::input_inventory::InputInventory::default(),
    );
}

fn finish_and_check(fixture: AtomicPathFixture, delivered: &mut HashSet<CompletionKey>) {
    let ledger = Rc::clone(&fixture.ledger);
    finish_atomic_fixture(fixture, delivered);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

fn finish_after_output_disable(fixture: AtomicPathFixture) {
    finish_after_output_disable_inner(fixture, false);
}

fn finish_after_output_disable_and_check(fixture: AtomicPathFixture) {
    finish_after_output_disable_inner(fixture, true);
}

fn finish_after_output_disable_inner(fixture: AtomicPathFixture, check_quiescence: bool) {
    let ledger = Rc::clone(&fixture.ledger);
    let mut scene = fixture.scene;
    let root = scene.events.backend.core.window_id;
    Backend::define_cursor(&mut scene.events.backend, None, root, 0x7f10)
        .expect("restore fixture cursor through Backend after output disable");
    let test_cursors: Vec<_> = scene
        .events
        .backend
        .cursor_records
        .keys()
        .copied()
        .filter(|cursor| *cursor != 0x7f10)
        .collect();
    for cursor in test_cursors {
        Backend::free_cursor(&mut scene.events.backend, None, cursor)
            .expect("free disabled test cursor through Backend");
    }
    for xid in [0x6a00, 0x6a01, 0x6a02, root, 0x6a10, 0x6a11, 0x6a12] {
        if scene.events.backend.store.lookup(xid).is_some() {
            Backend::free_pixmap(&mut scene.events.backend, None, xid)
                .expect("free disabled recording-scene drawable through Backend");
        }
    }
    for _ in 0..32 {
        Backend::before_block(&mut scene.events.backend);
    }
    if check_quiescence {
        scene.assert_quiescent();
        assert_no_cursor_resources(&scene.events.backend, None);
    }
    scene.finish();
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: skip the in-flight-prior detach, release the same old user twice,
// or release ownership before the blocking cursor commit is accepted.
fn p2_vt_suspend_detaches_even_with_a_replacement_in_flight() {
    let Some(mut fixture) = fixture(1) else {
        return;
    };
    let mut delivered = HashSet::new();
    let cursor_a = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [0x10, 0x20, 0x30, 0xff],
        (1, 1),
    );
    let first = super::support::establish_hardware_cursor(
        &mut fixture,
        &mut delivered,
        cursor_a,
        (120, 90),
    );
    let binding_a = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();

    let cursor_b = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [0x90, 0x80, 0x70, 0xff],
        (2, 1),
    );
    define_root_cursor(&mut fixture.scene.events.backend, cursor_b);
    damage_root(&mut fixture.scene.events.backend);
    let offset = record_count(&fixture, first.device_fd);
    let replacement = compose_until_cursor_commit_where(
        &mut fixture,
        &mut delivered,
        0,
        offset,
        binding_a.properties.fb_id,
        |record| record_property(record, binding_a.properties.fb_id).is_some_and(|fb| fb != 0),
    );
    let binding_b = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();
    assert_ne!(binding_a.lease, binding_b.lease);
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, binding_a.lease),
        Some(1)
    );
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, binding_b.lease),
        Some(1)
    );
    let replacement_id =
        crate::kms::render::backend::tests::commit_identity::support::accepted_event_id(
            &replacement,
        )
        .expect("replacement has an accepted ID");
    let snapshots = current_snapshots(&fixture.scene.events.backend);
    install_logical_topology(&mut fixture.scene.events.backend, snapshots);
    fixture
        .scene
        .events
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let disabled = install_recording_power_executor(&mut fixture);
    fixture.scene.events.queue_commit_completion(
        first.device_fd,
        first.crtc,
        replacement_id,
        59,
        Duration::from_millis(59),
    );
    let suspend_record_offset = record_count(&fixture, first.device_fd);

    finish_vt_release(&mut fixture);

    assert_eq!(
        applied_cursor_binding(&fixture.scene.events.backend, 0),
        None
    );
    assert!(
        fixture.scene.events.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprites()
            .all_users_zero(),
        "successful blocking detach releases both A and B exactly once",
    );
    let observation = disabled.borrow();
    assert_eq!(observation.len(), 1);
    let records_at_disable = &observation[0].records;
    assert!(
        records_at_disable
            .iter()
            .skip(suspend_record_offset)
            .any(|record| {
                record.kind == AtomicCommitKind::CursorOnly
                    && record.flags.is_empty()
                    && record.events.is_empty()
                    && record_property(record, binding_b.properties.fb_id) == Some(0)
                    && record_property(record, binding_b.properties.crtc_id) == Some(0)
            }),
        "blocking detach on B's plane precedes the all-off disable"
    );
    drop(observation);
    delivered.insert(CompletionKey {
        device_fd: first.device_fd,
        crtc: first.crtc,
        commit_id: replacement_id,
    });

    fixture.scene.events.queue_commit_completion(
        first.device_fd,
        first.crtc,
        replacement_id,
        60,
        Duration::from_millis(60),
    );
    Backend::on_page_flip_ready(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        first.device_fd,
    );
    assert!(
        fixture.scene.events.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprites()
            .all_users_zero()
    );
    Backend::set_cursor_hidden(&mut fixture.scene.events.backend, true);
    Backend::on_vt_acquire(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
    );
    finish_and_check(fixture, &mut delivered);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: clear `applied` after a failed detach, or let the discarded event
// release its still-live lease after every suspend detach attempt failed.
fn p2_failed_blocking_detach_keeps_ownership_and_invalidates_the_comparison() {
    let Some(mut fixture) = fixture(1) else {
        return;
    };
    let mut delivered = HashSet::new();
    let cursor = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (5, 5),
        [0x42, 0x51, 0x63, 0xff],
        (1, 2),
    );
    super::support::establish_hardware_cursor(&mut fixture, &mut delivered, cursor, (180, 120));
    let original = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();
    let fd = device_fd(&fixture.scene.events.backend, 0);
    // run_suspend submits three detach attempts: its explicit hide-all, DPMS-off,
    // and the scene drain after the all-off boundary. All three fail so the
    // accepted binding remains available for the resume full-state retry.
    for _ in 0..3 {
        fixture.scene.events.recorders[&fd]
            .script_result(Err(io::Error::from_raw_os_error(libc::EBUSY)));
    }
    let snapshots = current_snapshots(&fixture.scene.events.backend);
    install_logical_topology(&mut fixture.scene.events.backend, snapshots);
    fixture
        .scene
        .events
        .backend
        .platform
        .for_tests_accept_output_power_commits();

    finish_vt_release(&mut fixture);

    assert_eq!(
        applied_cursor_binding(&fixture.scene.events.backend, 0),
        Some(original)
    );
    assert!(!state_for_output(&fixture.scene.events.backend, 0).valid);
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, original.lease),
        Some(1)
    );
    assert_eq!(
        eventless_detach_records(&fixture, 0)
            .iter()
            .filter(|record| record.result != ScriptedResult::Ok)
            .count(),
        3,
        "all suspend detach attempts fail before the successful all-off",
    );

    let accepted_offset = accepted_live_records(&fixture.scene.events.recorders[&fd]).len();
    Backend::on_vt_acquire(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
    );

    let rebound = live_cursor_record_after(
        &fixture,
        0,
        accepted_offset,
        original.properties.fb_id,
        u64::from(original.framebuffer),
    );
    assert_full_attach(
        &rebound,
        original,
        fixture.scene.events.backend.platform.outputs[0].output.crtc,
    );
    let rebound_id =
        crate::kms::render::backend::tests::commit_identity::support::accepted_event_id(&rebound)
            .expect("resume rebind is accepted with a fresh event ID");
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, original.lease),
        Some(1)
    );
    assert_eq!(
        fixture.scene.events.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprites()
            .prepared_user_count(),
        0,
        "rebind accepted without a remaining reservation",
    );

    fixture.scene.events.queue_commit_completion(
        fd,
        fixture.scene.events.backend.platform.outputs[0].output.crtc,
        rebound_id,
        61,
        Duration::from_millis(61),
    );
    Backend::on_page_flip_ready(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        fd,
    );
    assert_eq!(
        applied_cursor_binding(&fixture.scene.events.backend, 0),
        Some(original)
    );
    assert!(state_for_output(&fixture.scene.events.backend, 0).valid);
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, original.lease),
        Some(1)
    );

    let hide = accept_cursor_detach(&mut fixture, &mut delivered, 0);
    retire_cursor_record(&mut fixture, &mut delivered, 0, &hide, 62);
    finish_and_check(fixture, &mut delivered);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: treat invalid applied state as valid and omit the full-state attach
// properties when rearming the existing sprite after resume.
fn p2_resume_sends_full_state_after_invalidation_without_a_new_reservation() {
    let Some(mut fixture) = fixture(1) else {
        return;
    };
    let mut delivered = HashSet::new();
    let cursor = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (5, 5),
        [0x24, 0x35, 0x46, 0xff],
        (2, 2),
    );
    let first =
        super::support::establish_hardware_cursor(&mut fixture, &mut delivered, cursor, (240, 140));
    let original = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();
    let first_write_count = record_write_count(&fixture);
    let fd = first.device_fd;
    for _ in 0..3 {
        fixture.scene.events.recorders[&fd]
            .script_result(Err(io::Error::from_raw_os_error(libc::EBUSY)));
    }
    let snapshots = current_snapshots(&fixture.scene.events.backend);
    install_logical_topology(&mut fixture.scene.events.backend, snapshots);
    fixture
        .scene
        .events
        .backend
        .platform
        .for_tests_accept_output_power_commits();

    finish_vt_release(&mut fixture);

    assert_eq!(eventless_detach_records(&fixture, 0).len(), 3);
    assert_eq!(
        applied_cursor_binding(&fixture.scene.events.backend, 0),
        Some(original)
    );
    assert!(!state_for_output(&fixture.scene.events.backend, 0).valid);
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, original.lease),
        Some(1)
    );
    assert_eq!(
        fixture.scene.events.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprites()
            .prepared_user_count(),
        0,
    );

    let accepted_offset = accepted_live_records(&fixture.scene.events.recorders[&fd]).len();
    Backend::on_vt_acquire(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
    );

    let rebound = live_cursor_record_after(
        &fixture,
        0,
        accepted_offset,
        original.properties.fb_id,
        u64::from(original.framebuffer),
    );
    assert_full_attach(
        &rebound,
        original,
        fixture.scene.events.backend.platform.outputs[0].output.crtc,
    );
    let rebound_id =
        crate::kms::render::backend::tests::commit_identity::support::accepted_event_id(&rebound)
            .expect("accepted rebind has a fresh event ID");
    assert_ne!(rebound_id, first.attach_commit_id);
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, original.lease),
        Some(1)
    );
    assert_eq!(
        fixture.scene.events.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprites()
            .prepared_user_count(),
        0,
        "an accepted rebind must not leave a reservation",
    );
    assert_eq!(record_write_count(&fixture), first_write_count);

    fixture.scene.events.queue_commit_completion(
        fd,
        first.crtc,
        rebound_id,
        63,
        Duration::from_millis(63),
    );
    Backend::on_page_flip_ready(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        fd,
    );
    assert_eq!(
        applied_cursor_binding(&fixture.scene.events.backend, 0),
        Some(original)
    );
    assert!(state_for_output(&fixture.scene.events.backend, 0).valid);
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, original.lease),
        Some(1)
    );
    assert_eq!(
        fixture.scene.events.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprites()
            .prepared_user_count(),
        0,
    );
    let hide = accept_cursor_detach(&mut fixture, &mut delivered, 0);
    retire_cursor_record(&mut fixture, &mut delivered, 0, &hide, 64);
    finish_and_check(fixture, &mut delivered);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: skip the blocking detach before DPMS-off; the off-state zero-user
// assertion must fail before the later full-state reattach.
fn p2_dpms_off_on_detaches_then_reattaches() {
    let Some(mut fixture) = fixture(1) else {
        return;
    };
    let mut delivered = HashSet::new();
    let cursor = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [0x15, 0x25, 0x35, 0xff],
        (1, 1),
    );
    let first =
        super::support::establish_hardware_cursor(&mut fixture, &mut delivered, cursor, (180, 130));
    let original = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();
    let write_count = record_write_count(&fixture);
    fixture
        .scene
        .events
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let disabled = install_recording_power_executor(&mut fixture);
    let dpms_record_offset = record_count(&fixture, first.device_fd);

    Backend::set_dpms_power(&mut fixture.scene.events.backend, 3)
        .expect("DPMS off through Backend");

    assert!(
        disabled.borrow().iter().any(|observation| {
            observation
                .records
                .iter()
                .skip(dpms_record_offset)
                .any(|record| {
                    record.kind == AtomicCommitKind::CursorOnly
                        && record.flags.is_empty()
                        && record.events.is_empty()
                        && record_property(record, original.properties.fb_id) == Some(0)
                        && record_property(record, original.properties.crtc_id) == Some(0)
                        && record.result == ScriptedResult::Ok
                })
        }),
        "DPMS all-off observes the successful blocking detach first"
    );
    assert!(
        eventless_detach_records(&fixture, 0)
            .iter()
            .any(|record| record.result == ScriptedResult::Ok)
    );
    assert_eq!(
        applied_cursor_binding(&fixture.scene.events.backend, 0),
        None
    );
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, original.lease),
        Some(0)
    );

    let accepted_offset =
        accepted_live_records(&fixture.scene.events.recorders[&first.device_fd]).len();
    Backend::set_dpms_power(&mut fixture.scene.events.backend, 0).expect("DPMS on through Backend");
    let rebound = live_cursor_record_after(
        &fixture,
        0,
        accepted_offset,
        original.properties.fb_id,
        u64::from(original.framebuffer),
    );
    assert_full_attach(
        &rebound,
        original,
        fixture.scene.events.backend.platform.outputs[0].output.crtc,
    );
    assert_eq!(record_write_count(&fixture), write_count);
    let rebound_id =
        crate::kms::render::backend::tests::commit_identity::support::accepted_event_id(&rebound)
            .expect("DPMS reattach has an ID");
    fixture.scene.events.queue_commit_completion(
        first.device_fd,
        first.crtc,
        rebound_id,
        65,
        Duration::from_millis(65),
    );
    Backend::on_page_flip_ready(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        first.device_fd,
    );
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, original.lease),
        Some(1)
    );
    let hide = accept_cursor_detach(&mut fixture, &mut delivered, 0);
    retire_cursor_record(&mut fixture, &mut delivered, 0, &hide, 66);
    finish_and_check(fixture, &mut delivered);
}

fn seed_randr_output_id(backend: &mut KmsBackend, output_idx: usize) -> u32 {
    let key = backend.platform.outputs[output_idx].key.clone();
    let ids = backend.pstate.randr_id_alloc.ids_for(&key);
    backend.pstate.output_key_by_id.insert(ids.output_id, key);
    ids.output_id
}

fn failed_detaches(fixture: &AtomicPathFixture, count: usize) {
    let fds: HashSet<_> = fixture
        .scene
        .events
        .backend
        .platform
        .outputs
        .iter()
        .map(|output| {
            device_fd(
                &fixture.scene.events.backend,
                output_index_for_crtc(&fixture.scene.events.backend, output.output.crtc),
            )
        })
        .collect();
    for fd in fds {
        for _ in 0..count {
            fixture.scene.events.recorders[&fd]
                .script_result(Err(io::Error::from_raw_os_error(libc::EBUSY)));
        }
    }
}

fn assert_removed_a_keeps_live_b(
    backend: &KmsBackend,
    crtc_a: ::drm::control::crtc::Handle,
    binding_a: crate::kms::render::platform::cursor_state::CursorBinding,
    binding_b: crate::kms::render::platform::cursor_state::CursorBinding,
) {
    let device = &backend.platform.devices[0];
    assert!(!device.cursor.crtc_states.contains_key(&crtc_a));
    assert!(
        !backend
            .platform
            .for_tests_atomic_cursor_commit_is_in_flight(
                backend.platform.outputs[0].key.device_key,
                crtc_a,
            ),
        "removed CRTC's in-flight cursor record is gone",
    );
    let b_index = output_index_for_crtc(backend, backend.platform.outputs[0].output.crtc);
    assert_eq!(applied_cursor_binding(backend, b_index), Some(binding_b));
    assert!(!state_for_output(backend, b_index).valid);
    assert_eq!(sprite_users(backend, b_index, binding_b.lease), Some(1));
    assert_eq!(
        device
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprites()
            .user_count(binding_a.lease),
        Some(1),
        "removed output's user is released while the shared live-B user remains",
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: omit removed-CRTC cleanup in remove_connector_at (A retains its
// lease), or release every CRTC on the device (B loses its user).
fn p2_randr_remove_after_all_off_releases_only_removed_crtc_ownership() {
    let Some(mut fixture) = fixture(2) else {
        return;
    };
    let mut delivered = HashSet::new();
    let [binding_a, binding_b] = establish_two_overlapping_cursors(&mut fixture, &mut delivered);
    let key_a = fixture.scene.events.backend.platform.outputs[0].key.clone();
    let crtc_a = fixture.scene.events.backend.platform.outputs[0].output.crtc;
    let crtc_b = fixture.scene.events.backend.platform.outputs[1].output.crtc;
    assert_ne!(crtc_a, crtc_b);
    failed_detaches(&fixture, 4);
    fixture
        .scene
        .events
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let snapshots = current_snapshots(&fixture.scene.events.backend);
    install_logical_topology(&mut fixture.scene.events.backend, snapshots);
    let output_id = seed_randr_output_id(&mut fixture.scene.events.backend, 0);

    assert!(
        Backend::apply_crtc_config(
            &mut fixture.scene.events.backend,
            output_id,
            &key_a.connector_name,
            None,
            0,
            0,
        )
        .expect("normal Backend RANDR disable path")
    );

    assert!(fixture.scene.events.backend.platform.outputs.len() == 1);
    assert_eq!(
        fixture.scene.events.backend.platform.outputs[0].output.crtc,
        crtc_b
    );
    assert_removed_a_keeps_live_b(&fixture.scene.events.backend, crtc_a, binding_a, binding_b);
    Backend::set_cursor_hidden(&mut fixture.scene.events.backend, true);
    damage_root(&mut fixture.scene.events.backend);
    let offset = record_count(&fixture, device_fd(&fixture.scene.events.backend, 0));
    let detach = compose_until_cursor_commit_where(
        &mut fixture,
        &mut delivered,
        0,
        offset,
        binding_b.properties.fb_id,
        |record| record_property(record, binding_b.properties.fb_id) == Some(0),
    );
    retire_cursor_record(&mut fixture, &mut delivered, 0, &detach, 67);
    finish_and_check(fixture, &mut delivered);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: omit removed-CRTC cleanup in the physical snapshot removal loop
// (A keeps a user), or release the live B CRTC with it. A physical disconnect
// keeps its route (#273); the remaining removal path is a connected connector
// that advertises no usable mode.
fn p2_hotplug_rescan_releases_only_removed_crtc_ownership() {
    let Some(mut fixture) = fixture(2) else {
        return;
    };
    let mut delivered = HashSet::new();
    let [binding_a, binding_b] = establish_two_overlapping_cursors(&mut fixture, &mut delivered);
    let crtc_a = fixture.scene.events.backend.platform.outputs[0].output.crtc;
    let crtc_b = fixture.scene.events.backend.platform.outputs[1].output.crtc;
    let mut snapshots = current_snapshots(&fixture.scene.events.backend);
    let key_a = fixture.scene.events.backend.platform.outputs[0].key.clone();
    snapshots
        .iter_mut()
        .find(|snapshot| snapshot.key == key_a)
        .expect("physical snapshot still lists A")
        .modes
        .clear();
    failed_detaches(&fixture, 4);
    fixture
        .scene
        .events
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    install_logical_topology(&mut fixture.scene.events.backend, snapshots);
    fixture.scene.events.backend.pstate.hotplug_rescan_deadline =
        Some(Instant::now() - Duration::from_millis(1));

    Backend::poll_deferred_input(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
    );

    assert_eq!(fixture.scene.events.backend.platform.outputs.len(), 1);
    assert_eq!(
        fixture.scene.events.backend.platform.outputs[0].output.crtc,
        crtc_b
    );
    assert_removed_a_keeps_live_b(&fixture.scene.events.backend, crtc_a, binding_a, binding_b);
    assert!(
        !fixture
            .scene
            .events
            .backend
            .platform
            .outputs
            .iter()
            .any(|output| { output.output.crtc == crtc_a })
    );
    Backend::set_cursor_hidden(&mut fixture.scene.events.backend, true);
    damage_root(&mut fixture.scene.events.backend);
    let offset = record_count(&fixture, device_fd(&fixture.scene.events.backend, 0));
    let detach = compose_until_cursor_commit_where(
        &mut fixture,
        &mut delivered,
        0,
        offset,
        binding_b.properties.fb_id,
        |record| record_property(record, binding_b.properties.fb_id) == Some(0),
    );
    retire_cursor_record(&mut fixture, &mut delivered, 0, &detach, 68);
    finish_and_check(fixture, &mut delivered);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: retire a physically disconnected route again (A's CRTC loses its
// cursor state and user), or detach its cursor on the rescan. Shutdown then
// releases both CRTCs through the device-wide clear.
fn p2_physical_disconnect_keeps_the_cursor_owned_by_the_kept_crtc() {
    let Some(mut fixture) = fixture(2) else {
        return;
    };
    let mut delivered = HashSet::new();
    let [binding_a, binding_b] = establish_two_overlapping_cursors(&mut fixture, &mut delivered);
    let crtc_a = fixture.scene.events.backend.platform.outputs[0].output.crtc;
    let key_b = fixture.scene.events.backend.platform.outputs[1].key.clone();
    let snapshot_b = current_snapshots(&fixture.scene.events.backend)
        .into_iter()
        .find(|snapshot| snapshot.key == key_b)
        .expect("physical snapshot retains B");
    fixture
        .scene
        .events
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    install_logical_topology(&mut fixture.scene.events.backend, vec![snapshot_b]);
    let fd = device_fd(&fixture.scene.events.backend, 0);
    let before = record_count(&fixture, fd);
    fixture.scene.events.backend.pstate.hotplug_rescan_deadline =
        Some(Instant::now() - Duration::from_millis(1));

    Backend::poll_deferred_input(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
    );

    let backend = &fixture.scene.events.backend;
    assert_eq!(
        backend.platform.outputs.len(),
        2,
        "the disconnect keeps A's route"
    );
    let a_index = output_index_for_crtc(backend, crtc_a);
    assert!(!backend.platform.outputs[a_index].connector_connected);
    assert_eq!(
        record_count(&fixture, fd),
        before,
        "a physical disconnect commits nothing, not even a cursor detach"
    );
    assert_eq!(applied_cursor_binding(backend, a_index), Some(binding_a));
    assert_eq!(
        applied_cursor_binding(backend, 1 - a_index),
        Some(binding_b)
    );
    assert_eq!(
        sprite_users(backend, a_index, binding_a.lease),
        Some(2),
        "the kept CRTC and B both still own the shared sprite"
    );

    failed_detaches(&fixture, 2);
    let disabled = install_recording_power_executor(&mut fixture);
    fixture
        .scene
        .events
        .backend
        .disable_output()
        .expect("shutdown through the recording output-power executor");
    assert_eq!(
        eventless_detach_records(&fixture, 0)
            .iter()
            .filter(|record| record.result != ScriptedResult::Ok)
            .count(),
        2,
        "both scene-drain detaches failed, leaving the device-wide clear as release owner"
    );
    assert!(
        disabled
            .borrow()
            .iter()
            .all(|observation| observation.crtc.is_none()),
        "the device-wide clear replaces the per-output disables"
    );
    assert!(
        fixture.scene.events.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprites()
            .all_users_zero(),
        "shutdown releases the disconnected CRTC's cursor too"
    );
    let _ = delivered;
    finish_after_output_disable(fixture);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: select only the applied binding and miss an unretired hide's prior
// plane, bypass the injected output-power executor during shutdown, or skip
// settling the CRTCs of a device the device-wide clear turned off.
fn p2_shutdown_settles_an_unretired_hide_and_observes_successful_disable() {
    let Some(mut fixture) = fixture(1) else {
        return;
    };
    let mut delivered = HashSet::new();
    let cursor = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [0xa1, 0xb2, 0xc3, 0xff],
        (1, 1),
    );
    let hardware =
        super::support::establish_hardware_cursor(&mut fixture, &mut delivered, cursor, (300, 200));
    let original = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();
    let before = record_count(&fixture, hardware.device_fd);
    Backend::set_cursor_hidden(&mut fixture.scene.events.backend, true);
    damage_root(&mut fixture.scene.events.backend);
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("submit scene frame that retires HideOnRetire");
    let scene_frame = accepted_live_records(&fixture.scene.events.recorders[&hardware.device_fd])
        .into_iter()
        .skip(before)
        .find(|record| {
            matches!(
                record.kind,
                AtomicCommitKind::Scene | AtomicCommitKind::SceneWithCursor
            ) && record
                .events
                .iter()
                .any(|event| event.crtc == hardware.crtc)
        })
        .expect("accepted scene frame before cursor hide");
    let scene_id = crate::kms::render::backend::tests::commit_identity::support::accepted_event_id(
        &scene_frame,
    )
    .expect("accepted scene frame has an ID");
    fixture.scene.events.queue_commit_completion(
        hardware.device_fd,
        hardware.crtc,
        scene_id,
        69,
        Duration::from_millis(69),
    );
    Backend::on_page_flip_ready(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        hardware.device_fd,
    );
    delivered.insert(CompletionKey {
        device_fd: hardware.device_fd,
        crtc: hardware.crtc,
        commit_id: scene_id,
    });

    let hide = live_cursor_record(&fixture, 0, original.properties.fb_id, 0);
    let hide_id =
        crate::kms::render::backend::tests::commit_identity::support::accepted_event_id(&hide)
            .expect("accepted hide has an event ID");
    assert!(!delivered.contains(&CompletionKey {
        device_fd: hardware.device_fd,
        crtc: hardware.crtc,
        commit_id: hide_id,
    }));
    assert_eq!(
        applied_cursor_binding(&fixture.scene.events.backend, 0),
        None
    );
    let cursor_record = fixture
        .scene
        .events
        .backend
        .platform
        .for_tests_atomic_cursor_prior_lease(
            fixture.scene.events.backend.platform.outputs[0]
                .key
                .device_key,
            hardware.crtc,
        );
    assert_eq!(
        cursor_record,
        Some(original.lease),
        "unretired hide retains its prior lease"
    );
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, original.lease),
        Some(1)
    );

    fixture.scene.events.recorders[&hardware.device_fd]
        .script_result(Err(io::Error::from_raw_os_error(libc::EBUSY)));
    let disabled = install_recording_power_executor(&mut fixture);
    fixture
        .scene
        .events
        .backend
        .disable_output()
        .expect("shutdown succeeds through the recording output-power executor");

    let detaches = eventless_detach_records(&fixture, 0);
    assert_eq!(
        detaches.len(),
        1,
        "only the scene-drain detach; the device-wide clear removes the plane"
    );
    assert_ne!(detaches[0].result, ScriptedResult::Ok);
    let observation = disabled.borrow();
    assert_eq!(
        observation.len(),
        1,
        "one device-wide clear and no per-output disable"
    );
    assert_eq!(
        observation[0].crtc, None,
        "the device-wide clear is the release owner"
    );
    assert!(
        observation[0]
            .records
            .iter()
            .any(|record| record.kind == AtomicCommitKind::CursorOnly
                && record.result != ScriptedResult::Ok),
        "the device-wide clear follows the failed scene-drain detach"
    );
    drop(observation);
    assert!(
        !fixture
            .scene
            .events
            .backend
            .platform
            .for_tests_atomic_cursor_commit_is_in_flight(
                fixture.scene.events.backend.platform.outputs[0]
                    .key
                    .device_key,
                hardware.crtc,
            )
    );
    assert!(
        fixture.scene.events.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprites()
            .all_users_zero()
    );
    finish_after_output_disable(fixture);
    let _ = hide;
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: discard without releasing the prior, or release it twice; the
// shared live CRTC user must remain at one after the discard.
fn p2_discard_releases_the_prior_of_a_discarded_in_flight_commit() {
    let Some(mut fixture) = fixture(2) else {
        return;
    };
    let mut delivered = HashSet::new();
    let [binding_a, live_binding] = establish_two_overlapping_cursors(&mut fixture, &mut delivered);
    assert_eq!(binding_a.lease, live_binding.lease);
    assert_eq!(
        fixture.scene.events.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprites()
            .user_count(binding_a.lease),
        Some(2),
        "both CRTCs retain the shared prior sprite before replacement",
    );
    let binding_a = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();
    let cursor_b = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [0x91, 0x82, 0x73, 0xff],
        (2, 2),
    );
    define_root_cursor(&mut fixture.scene.events.backend, cursor_b);
    damage_root(&mut fixture.scene.events.backend);
    let fd = device_fd(&fixture.scene.events.backend, 0);
    let offset = record_count(&fixture, fd);
    let replacement = compose_until_cursor_commit_where(
        &mut fixture,
        &mut delivered,
        0,
        offset,
        binding_a.properties.fb_id,
        |record| record_property(record, binding_a.properties.fb_id).is_some_and(|fb| fb != 0),
    );
    let replacement_id =
        crate::kms::render::backend::tests::commit_identity::support::accepted_event_id(
            &replacement,
        )
        .expect("replacement has an accepted ID");
    let binding_b = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, binding_a.lease),
        Some(2),
        "replacement keeps both the displaced and live CRTC owners until discard",
    );
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, binding_b.lease),
        Some(1)
    );
    // DPMS all-off discards the unretired replacement. The second CRTC keeps
    // A live, making a duplicate release of the discarded prior observable.
    failed_detaches(&fixture, 4);
    fixture
        .scene
        .events
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let crtc_a = fixture.scene.events.backend.platform.outputs[0].output.crtc;
    fixture.scene.events.queue_commit_completion(
        fd,
        crtc_a,
        replacement_id,
        69,
        Duration::from_millis(69),
    );
    let live_crtc = fixture.scene.events.backend.platform.outputs[1].output.crtc;
    let live_event_id = accepted_live_records(&fixture.scene.events.recorders[&fd])
        .into_iter()
        .find_map(|record| {
            let id =
                crate::kms::render::backend::tests::commit_identity::support::accepted_event_id(
                    &record,
                )?;
            (record.events.iter().any(|event| event.crtc == live_crtc)
                && !delivered.contains(&CompletionKey {
                    device_fd: fd,
                    crtc: live_crtc,
                    commit_id: id,
                }))
            .then_some(id)
        })
        .expect("live CRTC has its accepted scene event queued for the all-off drain");
    fixture.scene.events.queue_commit_completion(
        fd,
        live_crtc,
        live_event_id,
        70,
        Duration::from_millis(70),
    );
    Backend::set_dpms_power(&mut fixture.scene.events.backend, 3)
        .expect("all-off DPMS boundary succeeds after the failed detaches");
    delivered.insert(CompletionKey {
        device_fd: fd,
        crtc: crtc_a,
        commit_id: replacement_id,
    });

    assert_eq!(
        applied_cursor_binding(&fixture.scene.events.backend, 0),
        Some(binding_b)
    );
    assert_eq!(
        applied_cursor_binding(&fixture.scene.events.backend, 1),
        Some(live_binding)
    );
    assert_eq!(
        fixture.scene.events.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprites()
            .user_count(binding_a.lease),
        Some(1),
        "discard releases the prior once and preserves the shared live CRTC user",
    );
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, binding_b.lease),
        Some(1),
        "discard preserves the replacement's applied user",
    );
    assert!(
        !fixture
            .scene
            .events
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );

    fixture
        .scene
        .events
        .backend
        .disable_output()
        .expect("shutdown detaches the retained bindings after DPMS discard");
    finish_after_output_disable_and_check(fixture);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: retain a second stale owner slot which a later shutdown detach
// overwrites, causing the last release to be missed or applied twice.
fn p2_double_detach_releases_each_lease_once() {
    let Some(mut fixture) = fixture(1) else {
        return;
    };
    let mut delivered = HashSet::new();
    let cursor = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [0x60, 0x70, 0x80, 0xff],
        (1, 2),
    );
    super::support::establish_hardware_cursor(&mut fixture, &mut delivered, cursor, (260, 190));
    let binding = applied_cursor_binding(&fixture.scene.events.backend, 0).unwrap();
    fixture
        .scene
        .events
        .backend
        .platform
        .for_tests_accept_output_power_commits();

    Backend::set_dpms_power(&mut fixture.scene.events.backend, 3)
        .expect("DPMS off detaches the cursor");
    assert_eq!(
        applied_cursor_binding(&fixture.scene.events.backend, 0),
        None
    );
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, binding.lease),
        Some(0)
    );
    assert!(
        fixture.scene.events.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprites()
            .all_users_zero()
    );
    let detach_count_after_dpms = eventless_detach_records(&fixture, 0).len();

    fixture
        .scene
        .events
        .backend
        .disable_output()
        .expect("shutdown after DPMS off");
    assert!(
        fixture.scene.events.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprites()
            .all_users_zero()
    );
    assert_eq!(
        eventless_detach_records(&fixture, 0).len(),
        detach_count_after_dpms,
        "shutdown does not overwrite or release a stale second owner slot",
    );
    finish_after_output_disable(fixture);
}
