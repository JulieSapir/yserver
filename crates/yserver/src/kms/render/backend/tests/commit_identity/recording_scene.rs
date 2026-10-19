use super::support::AtomicEventFixture;
use crate::{
    drm::atomic::tests_support::ScriptedResult,
    kms::{
        render::{backend::KmsBackend, scene::SceneCompositor},
        vk::scanout::{BoPhase, OutputScanout, ScanoutBo, ScanoutBoPool},
    },
    platform::drm::Output,
};
use std::{
    collections::HashMap,
    io,
    os::fd::RawFd,
    sync::Arc,
    time::{Duration, Instant},
};
use yserver_core::backend::Backend;

pub(crate) struct RecordingSceneFixture {
    pub(crate) events: AtomicEventFixture,
}

impl RecordingSceneFixture {
    pub(crate) fn new(output_count: usize) -> Result<Option<Self>, String> {
        let backend = build_recording_scene_backend(output_count, false)?;
        Ok(backend.map(|backend| Self {
            events: AtomicEventFixture::from_backend(backend),
        }))
    }

    pub(crate) fn assert_quiescent(&self) {
        self.events.assert_quiescent();
        assert_eq!(
            self.events
                .backend
                .scene
                .pending_pool_release_count_for_tests(),
            0,
            "recording scene has deferred descriptor-pool releases"
        );
        assert!(
            !self.events.backend.scene.has_pending_page_flips(),
            "recording scene has pending page-flip acknowledgements"
        );
        assert!(
            self.events
                .backend
                .platform
                .scanout_pools
                .iter()
                .filter_map(Option::as_ref)
                .flat_map(|scanout| scanout.display_pool().bos.iter())
                .all(|bo| bo.state.phase != BoPhase::Pending),
            "recording scene has a Pending scanout BO"
        );
    }

    pub(crate) fn drain_deferred_pool_releases(&mut self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        let mut record_offsets: HashMap<RawFd, usize> = self
            .events
            .recorders
            .iter()
            .map(|(&fd, recorder)| (fd, recorder.records().len()))
            .collect();
        let mut submitted_frame = false;
        loop {
            if submitted_frame
                && self
                    .events
                    .backend
                    .scene
                    .pending_pool_release_count_for_tests()
                    == 0
            {
                break;
            }
            if Instant::now() >= deadline {
                break;
            }

            Backend::before_block(&mut self.events.backend);
            let root = self.events.backend.core.window_id;
            let width = self.events.backend.platform.fb_w;
            let height = self.events.backend.platform.fb_h;
            Backend::clear_area(
                &mut self.events.backend,
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
            .expect("damage root through Backend while draining scene pool releases");
            Backend::maybe_composite(&mut self.events.backend)
                .expect("drain deferred scene pool releases through a damaged Backend tick");
            // A fresh damaged tick can submit a frame while it polls the prior
            // frame's deferred pool release. The descriptor-pool ring can
            // return NoPool while those releases are still pending, so keep
            // polling through these Backend entries without requiring a
            // submission on every pass. Retire each resulting event through
            // the normal backend entry before checking quiescence.
            let record_batches: Vec<_> = self
                .events
                .recorders
                .iter()
                .map(|(&fd, recorder)| (fd, recorder.records()))
                .collect();
            let mut completion_fds = Vec::new();
            let mut queued_events = 0;
            for (fd, records) in record_batches {
                let offset = record_offsets
                    .get_mut(&fd)
                    .expect("record offset exists for fixture device");
                let new_records = records.iter().skip(*offset);
                *offset = records.len();
                for record in new_records {
                    if record.result != ScriptedResult::Ok
                        || record
                            .flags
                            .contains(::drm::control::AtomicCommitFlags::TEST_ONLY)
                        || !record
                            .flags
                            .contains(::drm::control::AtomicCommitFlags::PAGE_FLIP_EVENT)
                    {
                        continue;
                    }
                    for event in &record.events {
                        self.events.queue_commit_completion(
                            fd,
                            event.crtc,
                            record.user_data,
                            52,
                            Duration::from_millis(1600),
                        );
                        queued_events += 1;
                    }
                    if !record.events.is_empty() {
                        completion_fds.push(fd);
                    }
                }
            }
            submitted_frame |= queued_events > 0;
            completion_fds.sort_unstable();
            completion_fds.dedup();
            for fd in completion_fds {
                Backend::on_page_flip_ready(&mut self.events.backend, &mut self.events.state, fd);
            }
            Backend::maybe_composite(&mut self.events.backend)
                .expect("finish normal scene completion work through Backend");
        }
        // Mutation: suppress root damage; this eventual-submission assertion
        // must fail because maybe_composite then skips scene.tick.
        assert!(
            submitted_frame,
            "a damaged Backend tick must eventually submit a frame for explicit completion"
        );
        assert_eq!(
            self.events
                .backend
                .scene
                .pending_pool_release_count_for_tests(),
            0,
            "deferred scene pool releases did not drain at a damaged Backend scene tick before the deadline"
        );
    }

    pub(crate) fn finish(self) {
        let super::support::AtomicEventFixture {
            mut backend,
            state: _,
            recorders: _,
            event_sources: _,
        } = self.events;
        // XBackend drops PlatformBackend before SceneCompositor. Hold the
        // Vulkan owner until the scene's child handles have dropped too.
        let vk_keepalive = backend.platform.gpu.vk.as_ref().map(Arc::clone);
        let scene = std::mem::replace(&mut backend.scene, SceneCompositor::stub());
        drop(backend);
        drop(scene);
        drop(vk_keepalive);
    }
}

pub(in crate::kms::render::backend::tests) fn init_tearing_test_logger() {
    let _ = env_logger::Builder::new()
        .is_test(true)
        .filter_level(log::LevelFilter::Error)
        .filter_module("yserver", log::LevelFilter::Warn)
        .filter_module("yserver_render", log::LevelFilter::Warn)
        .try_init();
}

pub(in crate::kms::render::backend::tests) fn supports_vulkan_fed_scanout(
    supports_sync_fd: bool,
    supports_drm_format_modifier: bool,
) -> bool {
    supports_sync_fd && supports_drm_format_modifier
}

pub(in crate::kms::render::backend::tests) fn skip_unless_vulkan_fed_scanout(
    backend: &KmsBackend,
) -> bool {
    let supported = backend.platform.gpu.vk.as_ref().is_some_and(|vk| {
        supports_vulkan_fed_scanout(vk.supports_sync_fd(), vk.image_drm_format_modifier)
    });
    if !supported {
        eprintln!(
            "SKIP recording_scene: Vulkan device lacks SYNC_FD export or DRM format modifier support"
        );
    }
    supported
}

pub(in crate::kms::render::backend::tests) fn raw<H: ::drm::control::ResourceHandle>(
    value: u32,
) -> H {
    ::drm::control::from_u32(value).expect("nonzero test handle")
}

pub(in crate::kms::render::backend::tests) fn set_scene_fence_properties(output: &mut Output) {
    output.plane_in_fence_fd_prop = Some(raw(81));
    output.crtc_out_fence_ptr_prop = Some(raw(82));
}

pub(in crate::kms::render::backend::tests) fn set_unique_direct_plane_properties(
    output: &mut Output,
) {
    output.plane_fb_id_prop = raw(71);
    output.plane_crtc_id_prop = raw(72);
    output.plane_src_x_prop = raw(73);
    output.plane_src_y_prop = raw(74);
    output.plane_src_w_prop = raw(75);
    output.plane_src_h_prop = raw(76);
    output.plane_crtc_x_prop = raw(77);
    output.plane_crtc_y_prop = raw(78);
    output.plane_crtc_w_prop = raw(79);
    output.plane_crtc_h_prop = raw(80);
}

pub(in crate::kms::render::backend::tests) fn build_recording_scene_backend(
    output_count: usize,
    dead_zone: bool,
) -> Result<Option<KmsBackend>, String> {
    init_tearing_test_logger();
    let screen_width = match (output_count, dead_zone) {
        (2, true) => 1700,
        (2, false) => 1600,
        (1, false) => 800,
        _ => return Err("unsupported recording scene fixture topology".to_owned()),
    };
    let mut backend = match KmsBackend::for_tests_with_vk_dimensions(screen_width, 600) {
        Ok(backend) => backend,
        Err(error) if is_expected_vulkan_absence(&error) => {
            eprintln!("SKIP recording_scene: Vulkan ICD unavailable ({error})");
            return Ok(None);
        }
        Err(error) => return Err(format!("Vulkan fixture unavailable: {error}")),
    };
    if !skip_unless_vulkan_fed_scanout(&backend) {
        return Ok(None);
    }
    if output_count == 2 {
        super::super::push_test_output(&mut backend, 201);
        if dead_zone {
            backend.platform.outputs[1].x = 900;
            backend.platform.outputs[1].height = 400;
            backend.platform.outputs[1].output.picked.height = 400;
            for mode in &mut backend.platform.outputs[1].output.modes {
                mode.height = 400;
            }
        } else {
            backend.platform.outputs[1].x = 800;
        }
    }
    let layouts = backend.platform.scene_output_layouts();
    backend.scene = SceneCompositor::new(&backend.platform, &layouts)
        .map_err(|error| format!("live SceneCompositor unavailable: {error:?}"))?;
    backend
        .platform
        .scanout_pools
        .resize_with(output_count, || None);
    backend
        .platform
        .bo_generations
        .resize_with(output_count, Vec::new);

    let vk = Arc::clone(
        backend
            .platform
            .gpu
            .vk
            .as_ref()
            .expect("Vk-backed test fixture has a context"),
    );
    for output_idx in 0..output_count {
        let width = u32::from(backend.platform.outputs[output_idx].width);
        let height = u32::from(backend.platform.outputs[output_idx].height);
        let route = backend.platform.outputs[output_idx].scanout_route;
        let key = backend.platform.outputs[output_idx].key.clone();
        let drm = std::rc::Rc::clone(
            &backend
                .platform
                .device_for_key(key.device_key)
                .expect("test output has a KMS device")
                .device,
        );
        let output = &mut backend.platform.outputs[output_idx].output;
        set_scene_fence_properties(output);
        set_unique_direct_plane_properties(output);

        let mut bos = Vec::with_capacity(2);
        for bo_idx in 0..2 {
            let xid = 0x6a00
                + u32::try_from(output_idx).expect("output index fits") * 0x10
                + u32::try_from(bo_idx).expect("BO index fits");
            let storage = backend
                .platform
                .gpu
                .allocate_drawable_storage(
                    u16::try_from(width).expect("test width fits"),
                    u16::try_from(height).expect("test height fits"),
                    32,
                )
                .map_err(|error| format!("test target image allocation failed: {error:?}"))?;
            let image = storage.image;
            let image_view = storage.image_view;
            backend
                .store
                .allocate(
                    xid,
                    crate::kms::render::store::DrawableKind::Pixmap,
                    32,
                    false,
                    storage,
                )
                .map_err(|error| format!("test target storage registration failed: {error:?}"))?;
            let mut bo = ScanoutBo::for_tests_compose_target(
                Arc::clone(&vk),
                std::rc::Rc::clone(&drm),
                raw(0x5200 + u32::try_from(output_idx * 2 + bo_idx).expect("test BO handle fits")),
                width,
                height,
                image,
                image_view,
            )
            .map_err(|error| format!("test compose BO allocation failed: {error}"))?;
            if bo_idx == 0 {
                bo.state.phase = BoPhase::OnScreen;
            }
            bos.push(bo);
        }
        let pool = ScanoutBoPool::for_tests_recording_pool(route, width, height, bos);
        backend.platform.bo_generations[output_idx] =
            vec![crate::kms::render::platform::BoGenerationEntry::default(); 2];
        backend.platform.scanout_pools[output_idx] = Some(OutputScanout::Shared(pool));
    }
    for output in &backend.platform.outputs {
        backend
            .pstate
            .crtc_key_by_id
            .insert(u32::from(output.output.crtc), output.key.clone());
    }
    backend.refresh_present_crtc_clock_epochs();
    Ok(Some(backend))
}

fn is_expected_vulkan_absence(error: &io::Error) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("loader")
        || message.contains("no suitable physical device")
        || message.contains("nosuitabledevice")
        || message.contains("error_incompatible_driver")
}
