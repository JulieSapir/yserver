use crate::{
    drm::atomic::tests_support::{
        AtomicCommitRecord, RecordingAtomicCommitExecutor, ScriptedResult,
    },
    kms::{
        cursor_plane::{
            CursorHideExecutor, CursorPlaneBuffer, CursorShowExecutor,
            ops::{
                CursorBufferIo,
                recording::{CursorBufferLedger, RecordingCursorBufferIo},
            },
            sprites::SpriteBufferLease,
            topology::{
                CursorPlaneProperties, CursorPlaneTestPlane, CursorPlaneTestSnapshot,
                CursorStatePath,
            },
        },
        render::{backend::KmsBackend, platform::cursor_state::CursorBinding},
    },
};
use std::{
    cell::Cell,
    collections::HashSet,
    io,
    os::fd::{AsFd as _, AsRawFd as _, RawFd},
    rc::Rc,
    time::Duration,
};
use yserver_core::backend::{AnyHandle, Backend, CursorHandle};

use super::super::commit_identity::{
    recording_scene::{DirectPresentSetup, P1SceneRoute, RecordingSceneFixture},
    support::{AtomicEventFixture, accepted_event_id},
};

pub(crate) fn create_argb_cursor(
    backend: &mut KmsBackend,
    extent: (u16, u16),
    pixel: [u8; 4],
    hotspot: (u16, u16),
) -> CursorHandle {
    let (width, height) = extent;
    let source = Backend::create_pixmap(backend, None, 32, width, height)
        .expect("create ARGB cursor pixmap");
    Backend::put_image(
        backend,
        None,
        source.as_raw(),
        32,
        width,
        height,
        0,
        0,
        &pixel.repeat(usize::from(width) * usize::from(height)),
    )
    .expect("write ARGB cursor pixels");
    let picture =
        Backend::render_create_picture(backend, None, AnyHandle::Pixmap(source), 0, 0, &[])
            .expect("create ARGB cursor picture")
            .expect("picture handle");
    let cursor = Backend::render_create_cursor(backend, None, picture, hotspot.0, hotspot.1)
        .expect("create ARGB cursor")
        .expect("cursor handle");
    Backend::render_free_picture(backend, None, picture.as_raw())
        .expect("free ARGB cursor picture after cursor creation");
    Backend::free_pixmap(backend, None, source.as_raw())
        .expect("free ARGB cursor source pixmap after cursor creation");
    cursor
}

pub(crate) fn create_bitmap_cursor(backend: &mut KmsBackend, visible: bool) -> CursorHandle {
    let source = Backend::create_pixmap(backend, None, 1, 1, 1).expect("create source bitmap");
    let source_pixel = if visible { [1, 0, 0, 0] } else { [0; 4] };
    Backend::put_image(backend, None, source.as_raw(), 1, 1, 1, 0, 0, &source_pixel)
        .expect("write source bitmap bit");
    let mask = if visible {
        None
    } else {
        let mask = Backend::create_pixmap(backend, None, 1, 1, 1).expect("create mask bitmap");
        Backend::put_image(backend, None, mask.as_raw(), 1, 1, 1, 0, 0, &[0; 4])
            .expect("write transparent mask bit");
        Some(mask)
    };
    let cursor = Backend::create_cursor(
        backend,
        None,
        source,
        mask,
        (u16::MAX, u16::MAX, u16::MAX),
        (0, 0, 0),
        0,
        0,
    )
    .expect("create bitmap cursor");
    Backend::free_pixmap(backend, None, source.as_raw())
        .expect("free bitmap cursor source pixmap after cursor creation");
    if let Some(mask) = mask {
        Backend::free_pixmap(backend, None, mask.as_raw())
            .expect("free bitmap cursor mask pixmap after cursor creation");
    }
    cursor
}

pub(crate) fn define_root_cursor(backend: &mut KmsBackend, cursor: CursorHandle) {
    Backend::define_cursor(backend, None, backend.core.window_id, cursor.as_raw())
        .expect("define root cursor");
}

pub(crate) fn damage_root(fixture: &mut AtomicPathFixture) {
    let backend = &mut fixture.scene.events.backend;
    let root = backend.core.window_id;
    let width = backend.platform.fb_w;
    let height = backend.platform.fb_h;
    Backend::clear_area(backend, None, root, 0, None, 0, 0, width, height, (0, 0))
        .expect("damage root through Backend");
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PathKind {
    Physical,
    Paravirtualized,
}

#[derive(Default)]
pub(crate) struct RecordingLegacyCursor {
    pub(crate) show_calls: Cell<usize>,
    pub(crate) move_calls: Cell<usize>,
    pub(crate) hide_calls: Cell<usize>,
}

impl CursorShowExecutor for RecordingLegacyCursor {
    fn show(
        &self,
        _device: &crate::drm::Device,
        _buffer: &CursorPlaneBuffer,
        _crtc: ::drm::control::crtc::Handle,
        _hotspot: (i32, i32),
        _img_x: i32,
        _img_y: i32,
    ) -> io::Result<()> {
        self.show_calls.set(self.show_calls.get() + 1);
        Ok(())
    }

    fn move_to(
        &self,
        _device: &crate::drm::Device,
        _crtc: ::drm::control::crtc::Handle,
        _img_x: i32,
        _img_y: i32,
    ) -> io::Result<()> {
        self.move_calls.set(self.move_calls.get() + 1);
        Ok(())
    }
}

impl CursorHideExecutor for RecordingLegacyCursor {
    fn hide(
        &self,
        _device: &crate::drm::Device,
        _crtc: ::drm::control::crtc::Handle,
    ) -> io::Result<()> {
        self.hide_calls.set(self.hide_calls.get() + 1);
        Ok(())
    }
}

pub(crate) struct AtomicPathFixture {
    pub(crate) scene: RecordingSceneFixture,
    pub(crate) ledger: Rc<CursorBufferLedger>,
    pub(crate) legacy: Rc<RecordingLegacyCursor>,
}

pub(crate) struct LegacyPathFixture {
    pub(crate) scene: RecordingSceneFixture,
    pub(crate) ledger: Rc<CursorBufferLedger>,
    pub(crate) legacy: Rc<RecordingLegacyCursor>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct HardwareCursor {
    pub(crate) output_idx: usize,
    pub(crate) device_fd: RawFd,
    pub(crate) crtc: ::drm::control::crtc::Handle,
    pub(crate) attach_commit_id: u64,
}

impl AtomicPathFixture {
    pub(crate) fn new(kind: PathKind) -> Result<Option<Self>, String> {
        Self::with_output_count(kind, 1)
    }

    pub(crate) fn with_output_count(
        kind: PathKind,
        output_count: usize,
    ) -> Result<Option<Self>, String> {
        Self::with_scene_route(kind, output_count, P1SceneRoute::Shared)
    }

    pub(crate) fn with_scene_route(
        kind: PathKind,
        output_count: usize,
        route: P1SceneRoute,
    ) -> Result<Option<Self>, String> {
        let Some(mut scene) = RecordingSceneFixture::with_route(output_count, route)? else {
            return Ok(None);
        };
        if route != P1SceneRoute::Shared {
            scene
                .readiness
                .as_ref()
                .expect("copied fixture has a render-completion observer")
                .hold_ready_jobs();
        }
        let (io, ledger) = RecordingCursorBufferIo::new(256);
        let buffer_io: Rc<dyn CursorBufferIo> = Rc::new(io);
        let legacy = Rc::new(RecordingLegacyCursor::default());
        let property_names = [
            "FB_ID", "CRTC_ID", "CRTC_X", "CRTC_Y", "CRTC_W", "CRTC_H", "SRC_X", "SRC_Y", "SRC_W",
            "SRC_H",
        ];
        let mut property_names: Vec<_> = property_names.into_iter().map(str::to_owned).collect();
        if kind == PathKind::Paravirtualized {
            property_names.extend(["HOTSPOT_X".to_owned(), "HOTSPOT_Y".to_owned()]);
        }
        let (driver, hotspot_cap_accepted) = match kind {
            PathKind::Physical => ("amdgpu", false),
            PathKind::Paravirtualized => ("virtio_gpu", true),
        };
        for device_idx in 0..scene.events.backend.platform.devices.len() {
            let key = scene.events.backend.platform.devices[device_idx].key;
            let crtcs: Vec<_> = scene
                .events
                .backend
                .platform
                .outputs
                .iter()
                .filter(|output| output.key.device_key == key)
                .map(|output| output.output.crtc)
                .collect();
            if crtcs.is_empty() {
                continue;
            }
            let snapshot = CursorPlaneTestSnapshot {
                crtcs: crtcs.iter().map(|crtc| u32::from(*crtc)).collect(),
                planes: crtcs
                    .iter()
                    .enumerate()
                    .map(|(index, crtc)| CursorPlaneTestPlane {
                        handle: u32::try_from(index + 21).expect("cursor plane handle fits u32"),
                        possible_crtcs: vec![u32::from(*crtc)],
                        property_names: property_names.clone(),
                    })
                    .collect(),
                cursor_caps: Some(crate::drm::CursorDeviceCaps::new(
                    hotspot_cap_accepted,
                    Some(driver.to_owned()),
                )),
                has_buffer: true,
            };
            scene
                .events
                .backend
                .platform
                .for_tests_install_cursor_plane_from_snapshot(
                    device_idx,
                    snapshot,
                    Rc::clone(&buffer_io),
                    64,
                    64,
                );
            let device = &mut scene.events.backend.platform.devices[device_idx];
            assert_eq!(device.cursor.state_path(), CursorStatePath::Atomic);
            let plane = device
                .cursor
                .plane
                .as_mut()
                .expect("installed cursor plane");
            plane.for_tests_set_show_executor(legacy.clone());
            plane.for_tests_set_hide_executor(legacy.clone());
        }
        Ok(Some(Self {
            scene,
            ledger,
            legacy,
        }))
    }
}

impl LegacyPathFixture {
    pub(crate) fn new() -> Result<Option<Self>, String> {
        let Some(mut scene) = RecordingSceneFixture::new(1)? else {
            return Ok(None);
        };
        let (io, ledger) = RecordingCursorBufferIo::new(256);
        let buffer_io: Rc<dyn CursorBufferIo> = Rc::new(io);
        let legacy = Rc::new(RecordingLegacyCursor::default());
        let property_names: Vec<String> = [
            "FB_ID", "CRTC_ID", "CRTC_X", "CRTC_Y", "CRTC_W", "CRTC_H", "SRC_X", "SRC_Y", "SRC_W",
            "SRC_H",
        ]
        .into_iter()
        .filter(|name| *name != "SRC_H")
        .map(str::to_owned)
        .collect();
        for device_idx in 0..scene.events.backend.platform.devices.len() {
            let key = scene.events.backend.platform.devices[device_idx].key;
            let crtcs: Vec<_> = scene
                .events
                .backend
                .platform
                .outputs
                .iter()
                .filter(|output| output.key.device_key == key)
                .map(|output| output.output.crtc)
                .collect();
            let snapshot = CursorPlaneTestSnapshot {
                crtcs: crtcs.iter().map(|crtc| u32::from(*crtc)).collect(),
                planes: crtcs
                    .iter()
                    .enumerate()
                    .map(|(index, crtc)| CursorPlaneTestPlane {
                        handle: u32::try_from(index + 21).expect("cursor plane handle fits u32"),
                        possible_crtcs: vec![u32::from(*crtc)],
                        property_names: property_names.clone(),
                    })
                    .collect(),
                cursor_caps: Some(crate::drm::CursorDeviceCaps::new(
                    false,
                    Some("amdgpu".to_owned()),
                )),
                has_buffer: true,
            };
            scene
                .events
                .backend
                .platform
                .for_tests_install_cursor_plane_from_snapshot(
                    device_idx,
                    snapshot,
                    Rc::clone(&buffer_io),
                    64,
                    64,
                );
            let device = &mut scene.events.backend.platform.devices[device_idx];
            assert_eq!(device.cursor.state_path(), CursorStatePath::Legacy);
            let plane = device
                .cursor
                .plane
                .as_mut()
                .expect("installed cursor plane");
            plane.for_tests_set_show_executor(legacy.clone());
            plane.for_tests_set_hide_executor(legacy.clone());
        }
        Ok(Some(Self {
            scene,
            ledger,
            legacy,
        }))
    }
}

pub(crate) fn establish_hardware_cursor(
    fixture: &mut AtomicPathFixture,
    delivered: &mut HashSet<CompletionKey>,
    cursor: CursorHandle,
    pointer: (i32, i32),
) -> HardwareCursor {
    let output_idx = 0;
    {
        let backend = &mut fixture.scene.events.backend;
        define_root_cursor(backend, cursor);
    }
    Backend::warp_pointer_root(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        pointer.0,
        pointer.1,
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
    .expect("damage root for cursor transition through Backend");

    let layout = &fixture.scene.events.backend.platform.outputs[output_idx];
    let crtc = layout.output.crtc;
    let device_fd = fixture.scene.kms_fd(output_idx);
    let properties = cursor_properties_for_output(&fixture.scene.events.backend, output_idx);
    let recorder = &fixture.scene.events.recorders[&device_fd];
    let start = accepted_live_records(recorder).len();
    for frame in 1..=4 {
        Backend::maybe_composite(&mut fixture.scene.events.backend)
            .expect("compose cursor transition through Backend");
        if has_unretired_accepted_event(&fixture.scene.events, delivered, output_idx) {
            retire_pending_output(&mut fixture.scene.events, delivered, output_idx, frame);
        }
        let records = accepted_live_records(&fixture.scene.events.recorders[&device_fd]);
        let sequence: Vec<_> = records
            .iter()
            .skip(start)
            .filter(|record| record.events.iter().any(|event| event.crtc == crtc))
            .map(|record| record.kind)
            .collect();
        let expected_sequence = [
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
        ];
        assert_eq!(
            sequence, expected_sequence,
            "Atomic cursor event sequence changed at frame {frame}"
        );
        if let Some(attach) = records.iter().find(|record| {
            record.kind == crate::drm::atomic::AtomicCommitKind::CursorOnly
                && record.events.iter().any(|event| event.crtc == crtc)
                && record_property(record, properties.fb_id).is_some_and(|fb| fb != 0)
        }) {
            let applied = applied_cursor_binding(&fixture.scene.events.backend, output_idx);
            let no_cursor_commit = !fixture
                .scene
                .events
                .backend
                .platform
                .cursor_commit_in_flight_for_output(output_idx);
            if applied.is_some() && no_cursor_commit {
                let attach_commit_id = accepted_event_id(attach)
                    .expect("accepted cursor attach has a nonzero commit ID");
                return HardwareCursor {
                    output_idx,
                    device_fd,
                    crtc,
                    attach_commit_id,
                };
            }
        }
    }
    panic!("hardware cursor not established");
}

pub(crate) fn deliver_render_completion(fixture: &mut AtomicPathFixture, job_id: u64) {
    fixture.scene.events.wait_for_submitted_gpu_work();
    fixture
        .scene
        .readiness
        .as_ref()
        .expect("copied fixture has a render-completion observer")
        .release_ready_job(job_id);
    Backend::on_scanout_render_completion(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
    );
    fixture.scene.events.wait_for_submitted_gpu_work();
    assert!(
        !fixture.scene.pending_render_job_ids().contains(&job_id),
        "released render completion was delivered through Backend"
    );
}

pub(crate) fn finish_atomic_fixture(
    mut fixture: AtomicPathFixture,
    delivered: &mut HashSet<CompletionKey>,
) {
    let root = fixture.scene.events.backend.core.window_id;
    Backend::define_cursor(&mut fixture.scene.events.backend, None, root, 0x7f10)
        .expect("restore the fixture's sticky root cursor through Backend");
    if fixture.scene.readiness.is_some() {
        // Mutation: free copied-route cursor sprites before the submitted GPU
        // work settles. The bounded wait covers source and copied-sink queues;
        // Backend completion polling retires any source-job bookkeeping first.
        fixture.scene.events.wait_for_submitted_gpu_work();
        Backend::on_scanout_render_completion(
            &mut fixture.scene.events.backend,
            &mut fixture.scene.events.state,
        );
        assert!(fixture.scene.pending_render_job_ids().is_empty());
        fixture.scene.events.wait_for_submitted_gpu_work();
    }
    let test_cursors: Vec<_> = fixture
        .scene
        .events
        .backend
        .cursor_records
        .keys()
        .copied()
        .filter(|cursor| *cursor != 0x7f10)
        .collect();
    for cursor in test_cursors {
        Backend::free_cursor(&mut fixture.scene.events.backend, None, cursor)
            .expect("free test cursor through Backend after its plane was detached");
    }
    assert!(
        fixture
            .scene
            .events
            .backend
            .cursor_records
            .keys()
            .all(|cursor| *cursor == 0x7f10),
        "fixture cursor cleanup left a test cursor record",
    );
    assert!(
        fixture
            .scene
            .events
            .backend
            .cursor_pixmaps
            .keys()
            .all(|cursor| *cursor == 0x7f10),
        "fixture cursor cleanup left a test sprite pixmap",
    );
    finish_fixture(&mut fixture.scene.events, delivered);
    let _ = Backend::drain_completed_present_events(&mut fixture.scene.events.backend);
    let _ = Backend::drain_retired_present_idle_events(&mut fixture.scene.events.backend);
    let ledger = Rc::clone(&fixture.ledger);
    fixture.scene.finish_after_normal_drain();
    assert_eq!(ledger.end_state(), (0, 0, 0));
    let records = ledger.records();
    if records.iter().any(|record| {
        matches!(
            record,
            crate::kms::cursor_plane::ops::recording::CursorBufferRecord::AddFb { .. }
        )
    }) {
        assert!(
            records.iter().any(|record| matches!(
                record,
                crate::kms::cursor_plane::ops::recording::CursorBufferRecord::RemoveFb { .. }
            )),
            "free cursor sprites are removed after the plane is dropped"
        );
    }
}

pub(crate) fn record_count(fixture: &AtomicPathFixture, device_fd: RawFd) -> usize {
    fixture.scene.events.recorders[&device_fd].records().len()
}

pub(crate) fn compose_until_cursor_commit(
    fixture: &mut AtomicPathFixture,
    delivered: &mut HashSet<CompletionKey>,
    output_idx: usize,
    record_offset: usize,
    property: ::drm::control::property::Handle,
    value: u64,
) -> AtomicCommitRecord {
    compose_until_cursor_commit_where(
        fixture,
        delivered,
        output_idx,
        record_offset,
        property,
        |record| record_property(record, property) == Some(value),
    )
}

pub(crate) fn compose_until_cursor_commit_where<F>(
    fixture: &mut AtomicPathFixture,
    delivered: &mut HashSet<CompletionKey>,
    output_idx: usize,
    record_offset: usize,
    property: ::drm::control::property::Handle,
    mut matches: F,
) -> AtomicCommitRecord
where
    F: FnMut(&AtomicCommitRecord) -> bool,
{
    let device_fd = fixture.scene.kms_fd(output_idx);
    let crtc = fixture.scene.events.backend.platform.outputs[output_idx]
        .output
        .crtc;
    for frame in 1..=4 {
        Backend::maybe_composite(&mut fixture.scene.events.backend)
            .expect("compose cursor transition through Backend");
        let matching = fixture.scene.events.recorders[&device_fd]
            .records()
            .into_iter()
            .skip(record_offset)
            .find(|record| {
                record.device_fd == device_fd
                    && record.result == ScriptedResult::Ok
                    && !record
                        .flags
                        .contains(::drm::control::AtomicCommitFlags::TEST_ONLY)
                    && record.kind == crate::drm::atomic::AtomicCommitKind::CursorOnly
                    && accepted_event_id(record).is_some()
                    && record.events.iter().any(|event| event.crtc == crtc)
                    && record_property(record, property).is_some()
                    && matches(record)
            });
        if let Some(record) = matching {
            return record;
        }
        if has_unretired_accepted_event(&fixture.scene.events, delivered, output_idx) {
            retire_pending_output(&mut fixture.scene.events, delivered, output_idx, frame);
        }
    }
    panic!("cursor-only commit with property {property:?} not accepted");
}

pub(crate) fn accept_cursor_detach(
    fixture: &mut AtomicPathFixture,
    delivered: &mut HashSet<CompletionKey>,
    output_idx: usize,
) -> AtomicCommitRecord {
    drain_unretired_output_events(fixture, delivered, output_idx);
    Backend::set_cursor_hidden(&mut fixture.scene.events.backend, true);
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
    .expect("damage root for cursor hide through Backend");
    let offset = record_count(fixture, fixture.scene.kms_fd(output_idx));
    let properties = cursor_properties_for_output(&fixture.scene.events.backend, output_idx);
    compose_until_cursor_commit(fixture, delivered, output_idx, offset, properties.fb_id, 0)
}

pub(crate) fn drain_unretired_output_events(
    fixture: &mut AtomicPathFixture,
    delivered: &mut HashSet<CompletionKey>,
    output_idx: usize,
) {
    for frame in 20..28 {
        if !has_unretired_accepted_event(&fixture.scene.events, delivered, output_idx) {
            return;
        }
        retire_pending_output(&mut fixture.scene.events, delivered, output_idx, frame);
    }
    assert!(
        !has_unretired_accepted_event(&fixture.scene.events, delivered, output_idx),
        "accepted output events did not drain"
    );
}

pub(crate) fn retire_cursor_record(
    fixture: &mut AtomicPathFixture,
    delivered: &mut HashSet<CompletionKey>,
    output_idx: usize,
    record: &AtomicCommitRecord,
    frame: u32,
) {
    let id = accepted_event_id(record).expect("accepted cursor record has an ID");
    assert_eq!(
        retire_pending_output(&mut fixture.scene.events, delivered, output_idx, frame),
        id,
        "Backend must retire the matching cursor commit ID"
    );
}

pub(crate) fn enter_held_direct(
    fixture: &mut AtomicPathFixture,
    delivered: &mut HashSet<CompletionKey>,
    present_id: u64,
) -> Result<DirectPresentSetup, String> {
    let direct = fixture.scene.install_direct_present(present_id, 0);
    // install_direct_present hides the scene cursor. Restore it before direct
    // admission: the Atomic path must keep the already-applied plane visible.
    Backend::set_cursor_hidden(&mut fixture.scene.events.backend, false);
    assert!(!fixture.scene.events.backend.effective_cursor_hidden());
    let applied = applied_cursor_binding(&fixture.scene.events.backend, 0)
        .expect("cursor A remains applied after direct-present installation");
    assert_eq!(
        sprite_users(&fixture.scene.events.backend, 0, applied.lease),
        Some(1)
    );
    let device_fd = fixture.scene.kms_fd(0);

    // Restore the scene's retired cursor mode before testing direct admission.
    // The hide/unhide around direct Present can leave the scene in a transition
    // until a damaged scene frame retires, even though the plane still has A.
    let crtc = fixture.scene.events.backend.platform.outputs[0].output.crtc;
    let mut scene_mode = fixture.scene.events.backend.scene.cursor_mode();
    for frame in 1..=4 {
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
        .expect("damage root before direct admission");
        let record_offset = record_count(fixture, device_fd);
        Backend::maybe_composite(&mut fixture.scene.events.backend)
            .expect("compose scene frame before direct admission");
        let scene_frame = fixture.scene.events.recorders[&device_fd]
            .records()
            .into_iter()
            .skip(record_offset)
            .find(|record| {
                record.device_fd == device_fd
                    && record.result == ScriptedResult::Ok
                    && !record
                        .flags
                        .contains(::drm::control::AtomicCommitFlags::TEST_ONLY)
                    && record
                        .flags
                        .contains(::drm::control::AtomicCommitFlags::PAGE_FLIP_EVENT)
                    && matches!(
                        record.kind,
                        crate::drm::atomic::AtomicCommitKind::Scene
                            | crate::drm::atomic::AtomicCommitKind::SceneWithCursor
                    )
                    && record.events.iter().any(|event| event.crtc == crtc)
                    && accepted_event_id(record).is_some()
            })
            .ok_or_else(|| {
                let backend = &fixture.scene.events.backend;
                let accepted_kinds: Vec<_> = accepted_live_records(
                    &fixture.scene.events.recorders[&device_fd],
                )
                .into_iter()
                .map(|record| record.kind)
                .collect();
                format!(
                    "direct admission transition frame {frame} was not accepted (cursor_mode={scene_mode:?}, scene_wants_scene={}, structure_dirty={}, output_ready={}, scene_flip_pending={}, cursor_transition_pending={}, cursor_commit_in_flight={}, direct_active={}, accepted_kinds={accepted_kinds:?})",
                    backend.for_tests_scene_wants_scene(),
                    backend.scene.scene_structure_dirty,
                    backend.scene.output_ready_for_submit(0, &backend.platform),
                    backend.scene.has_pending_page_flip(0),
                    backend.scene.cursor_transition_pending(),
                    backend.platform.cursor_commit_in_flight_for_output(0),
                    backend.pstate.scanout_m2.active(),
                )
            })?;
        let id = accepted_event_id(&scene_frame).expect("accepted scene frame has an ID");
        fixture.scene.events.queue_commit_completion(
            device_fd,
            crtc,
            id,
            frame,
            Duration::from_millis(10),
        );
        Backend::on_page_flip_ready(
            &mut fixture.scene.events.backend,
            &mut fixture.scene.events.state,
            device_fd,
        );
        delivered.insert(CompletionKey {
            device_fd,
            crtc,
            commit_id: id,
        });
        scene_mode = fixture.scene.events.backend.scene.cursor_mode();
        if scene_mode == crate::kms::render::scene::CursorPlaneMode::Hw {
            break;
        }
    }
    if scene_mode != crate::kms::render::scene::CursorPlaneMode::Hw {
        return Err(format!(
            "direct admission cursor transition did not reach Hw (cursor_mode={scene_mode:?})"
        ));
    }

    let record_offset = record_count(fixture, device_fd);
    for attempt in 0..8 {
        let accepted = Backend::try_present_direct(
            &mut fixture.scene.events.backend,
            direct.candidate,
            direct.event.clone(),
        )
        .expect("direct candidate through Backend");
        if accepted != (attempt == 7) {
            return Err(format!(
                "direct admission has seven probation rejections (attempt={attempt}, streak={}, hidden={}, visible={}, cursor_eligible={}, pending_cursor={})",
                fixture
                    .scene
                    .events
                    .backend
                    .pstate
                    .scanout_m2
                    .eligible_root_streak,
                fixture.scene.events.backend.effective_cursor_hidden(),
                fixture
                    .scene
                    .events
                    .backend
                    .platform
                    .any_cursor_plane_visible(),
                fixture.scene.events.backend.direct_cursor_eligible(),
                fixture
                    .scene
                    .events
                    .backend
                    .platform
                    .any_cursor_commit_in_flight(),
            ));
        }
    }
    let direct_record = fixture.scene.events.recorders[&device_fd]
        .records()
        .into_iter()
        .skip(record_offset)
        .find(|record| {
            record.device_fd == device_fd
                && record.result == ScriptedResult::Ok
                && accepted_event_id(record).is_some()
                && record.kind != crate::drm::atomic::AtomicCommitKind::CursorOnly
        })
        .expect("accepted direct frame record");
    let id = accepted_event_id(&direct_record).expect("direct frame has an accepted ID");
    assert_eq!(direct_record.events.len(), 1);
    fixture.scene.events.queue_commit_completion(
        device_fd,
        direct_record.events[0].crtc,
        id,
        1,
        Duration::from_millis(10),
    );
    Backend::on_page_flip_ready(
        &mut fixture.scene.events.backend,
        &mut fixture.scene.events.state,
        device_fd,
    );
    delivered.insert(CompletionKey {
        device_fd,
        crtc: direct_record.events[0].crtc,
        commit_id: id,
    });
    assert!(fixture.scene.events.backend.pstate.scanout_m2.active());
    Ok(direct)
}

pub(crate) fn cursor_properties_for_output(
    backend: &KmsBackend,
    output_idx: usize,
) -> CursorPlaneProperties {
    let layout = &backend.platform.outputs[output_idx];
    let crtc = layout.output.crtc;
    let device_idx = backend
        .platform
        .devices
        .iter()
        .position(|device| device.key == layout.key.device_key)
        .expect("cursor output device");
    backend.platform.devices[device_idx]
        .cursor
        .plane
        .as_ref()
        .and_then(|plane| plane.properties_for(crtc))
        .expect("complete cursor properties")
}

pub(crate) fn applied_cursor_binding(
    backend: &KmsBackend,
    output_idx: usize,
) -> Option<CursorBinding> {
    let layout = &backend.platform.outputs[output_idx];
    let crtc = layout.output.crtc;
    let device_idx = backend
        .platform
        .devices
        .iter()
        .position(|device| device.key == layout.key.device_key)
        .expect("cursor output device");
    backend.platform.devices[device_idx]
        .cursor
        .crtc_states
        .get(&crtc)
        .and_then(|state| state.applied)
}

pub(crate) fn sprite_users(
    backend: &KmsBackend,
    output_idx: usize,
    lease: SpriteBufferLease,
) -> Option<usize> {
    let layout = &backend.platform.outputs[output_idx];
    let device_idx = backend
        .platform
        .devices
        .iter()
        .position(|device| device.key == layout.key.device_key)
        .expect("cursor output device");
    backend.platform.devices[device_idx]
        .cursor
        .plane
        .as_ref()
        .and_then(|plane| plane.sprites().user_count(lease))
}

pub(crate) fn sprite_bytes(
    backend: &KmsBackend,
    output_idx: usize,
    lease: SpriteBufferLease,
) -> Option<Vec<u8>> {
    let layout = &backend.platform.outputs[output_idx];
    let device_idx = backend
        .platform
        .devices
        .iter()
        .position(|device| device.key == layout.key.device_key)
        .expect("cursor output device");
    backend.platform.devices[device_idx]
        .cursor
        .plane
        .as_ref()
        .and_then(|plane| plane.sprites().for_tests_sprite_bytes(lease))
}

pub(crate) fn accepted_live_records(
    recorder: &RecordingAtomicCommitExecutor,
) -> Vec<AtomicCommitRecord> {
    recorder
        .records()
        .into_iter()
        .filter(|record| {
            record.result == ScriptedResult::Ok
                && !record
                    .flags
                    .contains(::drm::control::AtomicCommitFlags::TEST_ONLY)
        })
        .collect()
}

pub(crate) fn record_property(
    record: &AtomicCommitRecord,
    property: ::drm::control::property::Handle,
) -> Option<u64> {
    record
        .properties
        .iter()
        .find(|update| update.property == property)
        .map(|update| update.value)
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct CompletionKey {
    pub(crate) device_fd: RawFd,
    pub(crate) crtc: ::drm::control::crtc::Handle,
    pub(crate) commit_id: u64,
}

pub(crate) fn retire_pending_output(
    events: &mut AtomicEventFixture,
    delivered: &mut HashSet<CompletionKey>,
    output_idx: usize,
    frame: u32,
) -> u64 {
    let layout = &events.backend.platform.outputs[output_idx];
    let crtc = layout.output.crtc;
    let device_key = layout.key.device_key;
    let device = events
        .backend
        .platform
        .device_for_key(device_key)
        .expect("test output has an owning device");
    let device_fd = device.device.as_fd().as_raw_fd();
    let recorder = events
        .recorders
        .get(&device_fd)
        .expect("recording executor exists for output device");
    let matching: Vec<_> = accepted_live_records(recorder)
        .into_iter()
        .filter_map(|record| {
            let id = accepted_event_id(&record)?;
            (record.device_fd == device_fd
                && record.events.iter().any(|event| event.crtc == crtc)
                && !delivered.contains(&CompletionKey {
                    device_fd,
                    crtc,
                    commit_id: id,
                }))
            .then_some(id)
        })
        .collect();
    let id = match matching.as_slice() {
        [] => panic!("no accepted unretired commit for output"),
        [id] => *id,
        _ => panic!("more than one accepted unretired commit for output"),
    };

    events.queue_commit_completion(
        device_fd,
        crtc,
        id,
        frame,
        Duration::from_micros(u64::from(frame)),
    );
    Backend::on_page_flip_ready(&mut events.backend, &mut events.state, device_fd);
    delivered.insert(CompletionKey {
        device_fd,
        crtc,
        commit_id: id,
    });
    id
}

pub(crate) fn assert_no_cursor_resources(
    backend: &KmsBackend,
    ledger: Option<&CursorBufferLedger>,
) {
    assert!(
        !backend.platform.any_cursor_commit_in_flight(),
        "cursor commit still in flight",
    );
    for (output_idx, output) in backend.platform.outputs.iter().enumerate() {
        assert!(
            backend
                .platform
                .for_tests_atomic_cursor_pending_position(
                    output.key.device_key,
                    output.output.crtc,
                )
                .is_none(),
            "cursor position still pending for output {output_idx}",
        );
        assert!(
            !backend.platform.cursor_retry_waits_for_flip(output_idx),
            "cursor retry still waiting for output {output_idx}",
        );
    }
    for device in &backend.platform.devices {
        if let Some(plane) = &device.cursor.plane {
            assert!(
                plane.sprites().all_users_zero(),
                "cursor sprite pool still has users or reservations for device {}",
                device.key,
            );
        }
    }
    if let Some(ledger) = ledger {
        assert_eq!(
            ledger.end_state(),
            (0, 0, 0),
            "cursor buffer ledger is not empty"
        );
    }
}

pub(crate) fn has_unretired_accepted_event(
    events: &AtomicEventFixture,
    delivered: &HashSet<CompletionKey>,
    output_idx: usize,
) -> bool {
    let layout = &events.backend.platform.outputs[output_idx];
    let crtc = layout.output.crtc;
    let device_key = layout.key.device_key;
    let device = events
        .backend
        .platform
        .device_for_key(device_key)
        .expect("test output has an owning device");
    let device_fd = device.device.as_fd().as_raw_fd();
    events
        .recorders
        .get(&device_fd)
        .expect("recording executor exists for output device")
        .records()
        .into_iter()
        .any(|record| {
            accepted_event_id(&record).is_some_and(|id| {
                record.device_fd == device_fd
                    && record.events.iter().any(|event| event.crtc == crtc)
                    && !delivered.contains(&CompletionKey {
                        device_fd,
                        crtc,
                        commit_id: id,
                    })
            })
        })
}

pub(crate) fn finish_fixture(
    events: &mut AtomicEventFixture,
    delivered: &mut HashSet<CompletionKey>,
) {
    for output_idx in 0..events.backend.platform.outputs.len() {
        for iteration in 0..8 {
            if !has_unretired_accepted_event(events, delivered, output_idx) {
                break;
            }
            retire_pending_output(
                events,
                delivered,
                output_idx,
                u32::try_from(iteration + 1).expect("fixture frame fits u32"),
            );
        }
        assert!(
            !has_unretired_accepted_event(events, delivered, output_idx),
            "fixture did not settle",
        );
    }
    for _ in 0..4 {
        Backend::before_block(&mut events.backend);
    }
    events.assert_quiescent();
    assert_no_cursor_resources(&events.backend, None);
}

#[cfg(test)]
mod tests {
    use super::super::super::commit_identity::{
        recording_scene::RecordingSceneFixture,
        support::{AtomicEventFixture, accepted_event_id},
    };
    use crate::drm::atomic::{
        AtomicCommitKind, AtomicCommitRequest, AtomicCommitSink,
        tests_support::{RecordingAtomicCommitExecutor, ScriptedResult},
    };
    use std::{
        collections::HashSet,
        os::fd::{AsFd as _, AsRawFd as _, RawFd},
        rc::Rc,
    };
    use yserver_core::backend::Backend;

    fn raw<H: ::drm::control::ResourceHandle>(value: u32) -> H {
        ::drm::control::from_u32(value).expect("nonzero test handle")
    }

    fn output_fd(events: &AtomicEventFixture, output_idx: usize) -> RawFd {
        let layout = &events.backend.platform.outputs[output_idx];
        events
            .backend
            .platform
            .device_for_key(layout.key.device_key)
            .expect("test output has an owning device")
            .device
            .as_fd()
            .as_raw_fd()
    }

    #[test]
    // Mutation: define the cursor on a child window instead of the root, or
    // ignore `extent` and always construct a 4x4 ARGB cursor.
    #[ignore = "needs live Vulkan ICD (lavapipe)"]
    fn p2_fixture_cursor_helpers_control_the_effective_cursor() {
        let Some(mut fixture) = RecordingSceneFixture::new(1)
            .expect("cursor-helper recording scene fixture construction")
        else {
            return;
        };
        let backend = &mut fixture.events.backend;

        let visible = super::create_bitmap_cursor(backend, true);
        super::define_root_cursor(backend, visible);
        assert!(
            !backend.effective_cursor_hidden(),
            "visible cursor record: {:?}",
            backend.cursor_records[&visible.as_raw()],
        );
        assert_eq!(backend.effective_cursor_xid, Some(visible.as_raw()));

        let invisible = super::create_bitmap_cursor(backend, false);
        super::define_root_cursor(backend, invisible);
        assert!(backend.effective_cursor_hidden());
        assert_eq!(backend.effective_cursor_xid, Some(invisible.as_raw()));

        let argb = super::create_argb_cursor(backend, (6, 3), [1, 2, 3, 4], (1, 2));
        let record = backend
            .cursor_records
            .get(&argb.as_raw())
            .expect("ARGB cursor record");
        assert_eq!(record.width, 6);
        assert_eq!(record.height, 3);
        assert_eq!(record.hot_x, 1);
        assert_eq!(record.hot_y, 2);

        let root = backend.core.window_id;
        Backend::define_cursor(backend, None, root, 0x7f10)
            .expect("restore the fixture's sticky root cursor");
        let test_cursors: Vec<_> = backend
            .cursor_records
            .keys()
            .copied()
            .filter(|cursor| *cursor != 0x7f10)
            .collect();
        for cursor in test_cursors {
            Backend::free_cursor(backend, None, cursor).expect("free test cursor");
        }
        let mut delivered = HashSet::new();
        super::finish_fixture(&mut fixture.events, &mut delivered);
        fixture.finish_after_normal_drain();
    }

    #[test]
    // Mutation: deliver `id + 1` instead of the accepted event identity.
    fn p2_fixture_retire_pending_output_delivers_exactly_the_accepted_id() {
        let mut events = AtomicEventFixture::new(1);
        let output = &events.backend.platform.outputs[0];
        let device_key = output.key.device_key;
        let crtc = output.output.crtc;
        events
            .backend
            .platform
            .for_tests_seed_atomic_cursor_position(0, raw(51), raw(61), raw(62), 100, 100, 0, 0);
        Backend::warp_pointer_root(&mut events.backend, &mut events.state, 120, 110);
        Backend::maybe_composite(&mut events.backend).expect("submit cursor-only move");

        let fd = output_fd(&events, 0);
        let recorder = &events.recorders[&fd];
        let accepted = super::accepted_live_records(recorder);
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].kind, AtomicCommitKind::CursorOnly);
        let accepted_id = accepted_event_id(&accepted[0]).expect("accepted cursor event id");

        let mut delivered = HashSet::new();
        assert_eq!(
            super::retire_pending_output(&mut events, &mut delivered, 0, 1),
            accepted_id,
        );
        assert!(!events.backend.platform.any_cursor_commit_in_flight());
        assert_eq!(
            events.backend.platform.devices[0]
                .atomic_commit_sink
                .for_tests_pending_page_flip_entry_count(),
            0,
        );
        let second = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            super::retire_pending_output(&mut events, &mut delivered, 0, 2)
        }))
        .expect_err("a completion cannot be delivered twice");
        let message = second
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| second.downcast_ref::<&str>().copied());
        assert_eq!(message, Some("no accepted unretired commit for output"));
        assert!(delivered.contains(&super::CompletionKey {
            device_fd: fd,
            crtc,
            commit_id: accepted_id,
        }));
        super::finish_fixture(&mut events, &mut delivered);
        assert!(
            events
                .backend
                .platform
                .for_tests_atomic_cursor_pending_position(device_key, crtc)
                .is_none()
        );
    }

    #[test]
    // Mutation: key delivered completions by numeric ID alone, dropping the
    // device identity even though each device starts its own ID sequence.
    fn p2_fixture_retire_pending_output_qualifies_equal_ids_by_device() {
        let mut events = AtomicEventFixture::new(2);
        for (idx, plane, crtc_x, crtc_y) in [(0, 51, 61, 62), (1, 52, 63, 64)] {
            events
                .backend
                .platform
                .for_tests_seed_atomic_cursor_position(
                    idx,
                    raw(plane),
                    raw(crtc_x),
                    raw(crtc_y),
                    100,
                    100,
                    0,
                    0,
                );
        }
        Backend::warp_pointer_root(&mut events.backend, &mut events.state, 120, 110);
        Backend::maybe_composite(&mut events.backend).expect("submit cursor-only moves");

        let fd0 = output_fd(&events, 0);
        let fd1 = output_fd(&events, 1);
        let records0 = super::accepted_live_records(&events.recorders[&fd0]);
        let records1 = super::accepted_live_records(&events.recorders[&fd1]);
        assert_eq!(records0.len(), 1);
        assert_eq!(records1.len(), 1);
        assert_eq!(records0[0].kind, AtomicCommitKind::CursorOnly);
        assert_eq!(records1[0].kind, AtomicCommitKind::CursorOnly);
        assert_eq!(accepted_event_id(&records0[0]), Some(1));
        assert_eq!(accepted_event_id(&records1[0]), Some(1));

        let crtc0 = events.backend.platform.outputs[0].output.crtc;
        let crtc1 = events.backend.platform.outputs[1].output.crtc;
        assert_eq!(crtc0, crtc1);
        let mut delivered = HashSet::new();
        assert_eq!(
            super::retire_pending_output(&mut events, &mut delivered, 0, 1),
            1,
        );
        assert_eq!(
            super::retire_pending_output(&mut events, &mut delivered, 1, 2),
            1,
        );
        assert!(!events.backend.platform.any_cursor_commit_in_flight());
        assert!(delivered.contains(&super::CompletionKey {
            device_fd: fd0,
            crtc: crtc0,
            commit_id: 1,
        }));
        assert!(delivered.contains(&super::CompletionKey {
            device_fd: fd1,
            crtc: crtc1,
            commit_id: 1,
        }));
        super::finish_fixture(&mut events, &mut delivered);
    }

    #[test]
    #[ignore = "requires the lavapipe GPU scene fixture"]
    // Mutation: key grouped deliveries by `(fd, id)`, suppressing the second
    // target CRTC that shares the accepted cookie.
    fn p2_fixture_retire_pending_output_accepts_a_group_cookie_per_crtc() {
        let Some(mut fixture) =
            RecordingSceneFixture::new(2).expect("two-output recording scene fixture construction")
        else {
            return;
        };
        let direct = fixture.install_direct_present(1, 0);
        for attempt in 0..8 {
            let accepted = Backend::try_present_direct(
                &mut fixture.events.backend,
                direct.candidate,
                direct.event.clone(),
            )
            .expect("direct candidate through Backend");
            assert_eq!(accepted, attempt == 7, "drive the existing probation");
        }

        let fd = fixture.kms_fd(0);
        let record = fixture.events.recorders[&fd]
            .records()
            .into_iter()
            .find(|record| {
                accepted_event_id(record).is_some()
                    && record.events.len() == 2
                    && record.events[0].crtc != record.events[1].crtc
            })
            .expect("accepted grouped record targets both CRTCs");
        let id = accepted_event_id(&record).expect("grouped record has an accepted ID");
        let crtc0 = fixture.events.backend.platform.outputs[0].output.crtc;
        let crtc1 = fixture.events.backend.platform.outputs[1].output.crtc;
        assert!(record.events.iter().any(|event| event.crtc == crtc0));
        assert!(record.events.iter().any(|event| event.crtc == crtc1));

        let mut delivered = HashSet::new();
        assert_eq!(
            super::retire_pending_output(&mut fixture.events, &mut delivered, 0, 1),
            id,
        );
        assert_eq!(
            super::retire_pending_output(&mut fixture.events, &mut delivered, 1, 2),
            id,
        );
        assert!(delivered.contains(&super::CompletionKey {
            device_fd: fd,
            crtc: crtc0,
            commit_id: id,
        }));
        assert!(delivered.contains(&super::CompletionKey {
            device_fd: fd,
            crtc: crtc1,
            commit_id: id,
        }));

        let completed = Backend::drain_completed_present_events(&mut fixture.events.backend);
        assert_eq!(
            completed
                .iter()
                .map(|event| event.present_id)
                .collect::<Vec<_>>(),
            [direct.candidate.present_id],
        );
        assert!(Backend::drain_retired_present_idle_events(&mut fixture.events.backend).is_empty());

        let mut successor = direct.candidate;
        successor.present_id += 1;
        successor.valid_region_xid = 0x55;
        let mut successor_event = direct.event.clone();
        successor_event.present_id = successor.present_id;
        successor_event.serial = successor_event.serial.wrapping_add(1);
        assert!(
            !Backend::try_present_direct(&mut fixture.events.backend, successor, successor_event,)
                .expect("ineligible successor requests a composed unflip"),
            "the successor must request a composed unflip",
        );
        assert!(fixture.events.backend.pstate.scanout_m2.unflip_requested);

        let unflip_offset = fixture.events.recorders[&fd].records().len();
        Backend::maybe_composite(&mut fixture.events.backend).expect("submit grouped unflip");
        let unflip_record = fixture.events.recorders[&fd]
            .records()
            .into_iter()
            .skip(unflip_offset)
            .find(|record| record.events.len() == 2 && accepted_event_id(record).is_some())
            .expect("accepted grouped unflip record");
        let unflip_id = accepted_event_id(&unflip_record).expect("unflip has an accepted ID");
        assert_ne!(unflip_id, id);
        assert_eq!(
            super::retire_pending_output(&mut fixture.events, &mut delivered, 0, 3),
            unflip_id,
        );
        assert_eq!(
            super::retire_pending_output(&mut fixture.events, &mut delivered, 1, 4),
            unflip_id,
        );
        assert!(fixture.events.backend.pstate.scanout_m2.current.is_none());
        assert!(fixture.events.backend.present_source_pins.is_empty());
        assert!(Backend::drain_completed_present_events(&mut fixture.events.backend).is_empty());
        let idled = Backend::drain_retired_present_idle_events(&mut fixture.events.backend);
        assert_eq!(
            idled
                .iter()
                .map(|event| event.present_id)
                .collect::<Vec<_>>(),
            [direct.candidate.present_id],
        );

        super::finish_fixture(&mut fixture.events, &mut delivered);
        fixture.finish_after_normal_drain();
    }

    #[test]
    // Mutation: include TEST_ONLY records in accepted live records, or match
    // `record_property` by object handle instead of property handle.
    fn p2_fixture_accepted_live_records_exclude_test_only_and_failed_commits() {
        let mut events = AtomicEventFixture::new(2);
        let recorder = Rc::new(RecordingAtomicCommitExecutor::default());
        let fds: Vec<_> = events
            .backend
            .platform
            .devices
            .iter()
            .map(|device| device.device.as_fd().as_raw_fd())
            .collect();
        for device in &mut events.backend.platform.devices {
            let executor: Rc<dyn crate::drm::atomic::AtomicCommitExecutor> = recorder.clone();
            device.atomic_commit_sink = Rc::new(AtomicCommitSink::new(executor));
        }
        events.recorders.clear();
        for fd in &fds {
            events.recorders.insert(*fd, Rc::clone(&recorder));
        }
        for (idx, plane, crtc_x, crtc_y) in [(0, 51, 61, 62), (1, 52, 63, 64)] {
            events
                .backend
                .platform
                .for_tests_seed_atomic_cursor_position(
                    idx,
                    raw(plane),
                    raw(crtc_x),
                    raw(crtc_y),
                    100,
                    100,
                    0,
                    0,
                );
        }
        recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
        Backend::warp_pointer_root(&mut events.backend, &mut events.state, 120, 110);
        Backend::maybe_composite(&mut events.backend).expect("submit cursor-only moves");

        let test_only_device = &events.backend.platform.devices[0];
        test_only_device
            .atomic_commit_sink
            .test_only(
                &test_only_device.device,
                AtomicCommitKind::CursorOnly,
                AtomicCommitRequest::new(),
            )
            .expect("inject a TEST_ONLY record");

        let records = recorder.records();
        assert!(records.iter().any(|record| {
            record.kind == AtomicCommitKind::CursorOnly
                && record.result
                    == ScriptedResult::Error {
                        kind: std::io::ErrorKind::InvalidInput,
                        raw_os_error: Some(libc::EINVAL),
                    }
        }));
        assert!(records.iter().any(|record| {
            record
                .flags
                .contains(::drm::control::AtomicCommitFlags::TEST_ONLY)
        }));
        let accepted = super::accepted_live_records(&recorder);
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].kind, AtomicCommitKind::CursorOnly);
        assert_eq!(accepted[0].result, ScriptedResult::Ok);
        let crtc_x = raw(63);
        let x_value = accepted[0]
            .properties
            .iter()
            .find(|update| update.property == crtc_x)
            .expect("accepted record carries CRTC_X")
            .value;
        assert_eq!(super::record_property(&accepted[0], crtc_x), Some(x_value));
        assert_eq!(
            super::record_property(&accepted[0], raw(52)),
            None,
            "plane object handle is not a property handle",
        );

        let mut delivered = HashSet::new();
        super::finish_fixture(&mut events, &mut delivered);
    }

    #[test]
    #[should_panic(expected = "cursor commit still in flight")]
    // Mutation: skip the in-flight assertion in `assert_no_cursor_resources`.
    fn p2_fixture_finish_rejects_an_unretired_cursor_commit() {
        let mut events = AtomicEventFixture::new(1);
        events
            .backend
            .platform
            .for_tests_seed_atomic_cursor_position(0, raw(51), raw(61), raw(62), 100, 100, 0, 0);
        Backend::warp_pointer_root(&mut events.backend, &mut events.state, 120, 110);
        Backend::maybe_composite(&mut events.backend).expect("submit cursor-only move");
        let assertion = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            super::assert_no_cursor_resources(&events.backend, None);
        }));
        let mut delivered = HashSet::new();
        super::finish_fixture(&mut events, &mut delivered);
        std::panic::resume_unwind(
            assertion.expect_err("an unretired cursor commit must be rejected"),
        );
    }
}
