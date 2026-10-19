use crate::{
    drm::atomic::{
        AtomicCommitSink,
        tests_support::{AtomicCommitRecord, RecordingAtomicCommitExecutor, ScriptedResult},
    },
    kms::{
        backend::ActiveOutput,
        render::{
            backend::KmsBackend,
            platform::{RenderDeviceId, flip_events::RecordingPageFlipEventSource},
        },
        scanout_route::{RenderKmsRelationship, ScanoutRoute},
    },
};
use std::{
    collections::HashMap,
    os::fd::{AsFd as _, AsRawFd as _, RawFd},
    rc::Rc,
    time::Duration,
};
use yserver_core::server::ServerState;

pub(crate) struct AtomicEventFixture {
    pub(crate) backend: KmsBackend,
    pub(crate) state: ServerState,
    pub(crate) recorders: HashMap<RawFd, Rc<RecordingAtomicCommitExecutor>>,
    pub(crate) event_sources: HashMap<RawFd, Rc<RecordingPageFlipEventSource>>,
}

impl AtomicEventFixture {
    /// Build headless device/output pairs. Each device deliberately exposes
    /// CRTC handle 1 so tests cannot accidentally rely on globally unique
    /// object handles.
    pub(crate) fn new(output_count: usize) -> Self {
        assert!(
            output_count > 0,
            "an event fixture needs at least one output"
        );
        let mut backend = KmsBackend::for_tests();
        for minor in 1..output_count {
            let minor = u32::try_from(minor).expect("test device index fits");
            let device_key = super::super::test_device_key(minor);
            super::super::push_test_device(&mut backend, device_key);
            let connector_name = format!("test-{minor}");
            let output_key = super::super::test_output_key(minor, &connector_name);
            let route = ScanoutRoute::new(
                RenderDeviceId::UnverifiedFallback,
                device_key,
                RenderKmsRelationship::Unknown,
            );
            backend.platform.outputs.push(ActiveOutput::new(
                route,
                test_output(connector_name),
                crate::drm::Swapchain::empty_for_tests(),
                i32::try_from(minor).expect("test output position fits") * 800,
                0,
            ));
            backend.pstate.crtc_key_by_id.insert(1, output_key);
        }
        backend.platform.fb_w =
            u16::try_from(output_count * 800).expect("test virtual screen width fits u16");
        backend
            .platform
            .scanout_pools
            .resize_with(output_count, || None);
        backend
            .platform
            .bo_generations
            .resize_with(output_count, Vec::new);
        backend
            .platform
            .first_pageflip_logged
            .resize(output_count, false);
        backend.refresh_present_crtc_clock_epochs();
        Self::from_backend(backend)
    }

    pub(crate) fn from_backend(mut backend: KmsBackend) -> Self {
        let mut recorders = HashMap::new();
        let mut event_sources = HashMap::new();
        for device in &mut backend.platform.devices {
            let fd = device.device.as_fd().as_raw_fd();
            let recorder = Rc::new(RecordingAtomicCommitExecutor::default());
            device.atomic_commit_sink = Rc::new(AtomicCommitSink::new(recorder.clone()));
            let event_source = Rc::new(RecordingPageFlipEventSource::default());
            device.page_flip_event_source = event_source.clone();
            device.test_page_flip_event_source = Some(event_source.clone());
            recorders.insert(fd, recorder);
            event_sources.insert(fd, event_source);
        }
        Self {
            backend,
            state: ServerState::new(),
            recorders,
            event_sources,
        }
    }

    /// Queue exactly the supplied completion cookie on the selected device's
    /// event source. This is an injected external observation, not a submit or
    /// retirement helper.
    pub(crate) fn queue_commit_completion(
        &self,
        device_fd: RawFd,
        crtc: ::drm::control::crtc::Handle,
        commit_id: u64,
        frame: u32,
        ust: Duration,
    ) {
        self.event_sources
            .get(&device_fd)
            .expect("completion device fd belongs to this fixture")
            .queue_flip_with_cookie(crtc, frame, commit_id, ust);
    }

    /// Check observations without clearing state or manufacturing completion.
    pub(crate) fn assert_quiescent(&self) {
        for device in &self.backend.platform.devices {
            assert_eq!(
                device
                    .atomic_commit_sink
                    .for_tests_pending_page_flip_entry_count(),
                0,
                "device {} has pending flip associations",
                device.key
            );
            for output in self
                .backend
                .platform
                .outputs
                .iter()
                .filter(|output| output.key.device_key == device.key)
            {
                assert!(
                    device
                        .atomic_commit_sink
                        .for_tests_pending_page_flip_commit_ids(&device.device, output.output.crtc,)
                        .is_empty(),
                    "device {} has pending identity for CRTC {:?}",
                    device.key,
                    output.output.crtc
                );
            }
        }
        for source in self.event_sources.values() {
            assert_eq!(source.queued_flip_count(), 0, "unconsumed injected flip");
        }
    }
}

pub(crate) fn accepted_event_id(record: &AtomicCommitRecord) -> Option<u64> {
    (record.result == ScriptedResult::Ok
        && !record
            .flags
            .contains(::drm::control::AtomicCommitFlags::TEST_ONLY)
        && record
            .flags
            .contains(::drm::control::AtomicCommitFlags::PAGE_FLIP_EVENT)
        && record.user_data != 0)
        .then_some(record.user_data)
}

fn raw<H: ::drm::control::ResourceHandle>(value: u32) -> H {
    ::drm::control::from_u32(value).expect("nonzero test handle")
}

fn test_output(connector_name: String) -> crate::platform::drm::Output {
    crate::platform::drm::Output {
        connector: raw(1),
        connector_name,
        encoder: raw(1),
        crtc: raw(1),
        plane: raw(1),
        // SAFETY: synthetic fixture mode is never passed to DRM.
        mode: unsafe { std::mem::zeroed() },
        picked: crate::platform::drm::Mode {
            name: "800x600".to_owned(),
            width: 800,
            height: 600,
            vrefresh: 60,
            preferred: true,
            ..Default::default()
        },
        plane_fb_id_prop: raw(1),
        plane_crtc_id_prop: raw(1),
        plane_src_x_prop: raw(1),
        plane_src_y_prop: raw(1),
        plane_src_w_prop: raw(1),
        plane_src_h_prop: raw(1),
        plane_crtc_x_prop: raw(1),
        plane_crtc_y_prop: raw(1),
        plane_crtc_w_prop: raw(1),
        plane_crtc_h_prop: raw(1),
        plane_in_fence_fd_prop: None,
        crtc_out_fence_ptr_prop: None,
        scanout_modifiers: Vec::new(),
        mm_width: 0,
        mm_height: 0,
        edid: Vec::new(),
        connector_type: "unknown".to_owned(),
        modes: vec![crate::platform::drm::Mode {
            name: "800x600".to_owned(),
            width: 800,
            height: 600,
            vrefresh: 60,
            preferred: true,
            ..Default::default()
        }],
    }
}
