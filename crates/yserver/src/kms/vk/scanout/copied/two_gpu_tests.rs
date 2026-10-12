use super::*;

use std::{ffi::CStr, sync::Arc};

use crate::kms::vk::device::VulkanDeviceSelector;

const TWO_GPU_FENCE_TIMEOUT_NS: u64 = 5_000_000_000;
const NVIDIA_VENDOR_ID: u32 = 0x10de;
const AMD_VENDOR_ID: u32 = 0x1002;

#[derive(Clone, Debug)]
struct GpuInfo {
    selector: VulkanDeviceSelector,
    vendor_id: u32,
    driver_id: vk::DriverId,
    device_type: vk::PhysicalDeviceType,
    name: String,
}

fn enumerate_test_gpus() -> io::Result<Vec<GpuInfo>> {
    let entry = unsafe { ash::Entry::load() }
        .map_err(|error| io::Error::other(format!("load Vulkan loader: {error}")))?;
    let app_info = vk::ApplicationInfo::default()
        .application_name(c"yserver copied PRIME two-GPU test")
        .application_version(vk::make_api_version(0, 0, 1, 0))
        .engine_name(c"yserver test harness")
        .api_version(vk::API_VERSION_1_2);
    let instance = unsafe {
        entry.create_instance(
            &vk::InstanceCreateInfo::default().application_info(&app_info),
            None,
        )
    }
    .map_err(|error| io::Error::other(format!("create Vulkan inventory instance: {error:?}")))?;

    let result = (|| {
        let physical_devices = unsafe { instance.enumerate_physical_devices() }
            .map_err(|error| io::Error::other(format!("enumerate Vulkan devices: {error:?}")))?;
        let mut gpus = Vec::new();
        for physical_device in physical_devices {
            let queue_families =
                unsafe { instance.get_physical_device_queue_family_properties(physical_device) };
            if !queue_families.iter().any(|family| {
                family.queue_count > 0 && family.queue_flags.contains(vk::QueueFlags::GRAPHICS)
            }) {
                continue;
            }

            let mut id = vk::PhysicalDeviceIDProperties::default();
            let mut driver = vk::PhysicalDeviceDriverProperties::default();
            let (vendor_id, device_type, name) = {
                let mut properties = vk::PhysicalDeviceProperties2::default()
                    .push_next(&mut id)
                    .push_next(&mut driver);
                unsafe {
                    instance.get_physical_device_properties2(physical_device, &mut properties);
                }
                let name = unsafe { CStr::from_ptr(properties.properties.device_name.as_ptr()) }
                    .to_string_lossy()
                    .into_owned();
                (
                    properties.properties.vendor_id,
                    properties.properties.device_type,
                    name,
                )
            };
            let selector = VulkanDeviceSelector::from_uuid_pair(id.device_uuid, id.driver_uuid);
            let driver_id = driver.driver_id;
            gpus.push(GpuInfo {
                selector,
                vendor_id,
                driver_id,
                device_type,
                name,
            });
        }
        Ok(gpus)
    })();
    unsafe { instance.destroy_instance(None) };
    result
}

fn two_gpu_contexts(
    render_vendor: u32,
) -> io::Result<(GpuInfo, GpuInfo, Arc<VkContext>, Arc<VkContext>)> {
    let gpus = enumerate_test_gpus()?;
    let nvidia = gpus
        .iter()
        .find(|gpu| gpu.vendor_id == NVIDIA_VENDOR_ID)
        .cloned()
        .ok_or_else(|| {
            io::Error::other(format!(
                "two-GPU copied PRIME test requires an NVIDIA Vulkan device; found {gpus:?}"
            ))
        })?;
    let amd = gpus
        .iter()
        .find(|gpu| gpu.vendor_id == AMD_VENDOR_ID && gpu.driver_id == vk::DriverId::MESA_RADV)
        .cloned()
        .ok_or_else(|| {
            io::Error::other(format!(
                "two-GPU copied PRIME test requires an AMD RADV Vulkan device; found {gpus:?}"
            ))
        })?;
    let (renderer, sink) = if render_vendor == AMD_VENDOR_ID {
        (amd, nvidia)
    } else {
        (nvidia, amd)
    };
    let render_vk = VkContext::new_disposable_for_test(renderer.selector)
        .map_err(|error| io::Error::other(format!("create renderer {}: {error}", renderer.name)))?;
    let sink_vk = VkContext::new_disposable_transfer_for_device(sink.selector)
        .map_err(|error| io::Error::other(format!("create sink {}: {error}", sink.name)))?;
    Ok((renderer, sink, render_vk, sink_vk))
}

struct TwoGpuAttempt {
    render_vk: Arc<VkContext>,
    sink_vk: Arc<VkContext>,
    source: Option<CopiedRenderSource>,
    sink_destination: Option<DrawableImage>,
    pattern: Option<CopiedProbePatternPipeline>,
    render_digest: Option<ProbeDigestPipeline>,
    sink_digest: Option<ProbeDigestPipeline>,
    sink_command_pool: vk::CommandPool,
    sink_command_buffer: vk::CommandBuffer,
    width: u32,
    height: u32,
}

impl TwoGpuAttempt {
    fn new(render_vk: Arc<VkContext>, sink_vk: Arc<VkContext>, width: u32, height: u32) -> Self {
        Self {
            render_vk,
            sink_vk,
            source: None,
            sink_destination: None,
            pattern: None,
            render_digest: None,
            sink_digest: None,
            sink_command_pool: vk::CommandPool::null(),
            sink_command_buffer: vk::CommandBuffer::null(),
            width,
            height,
        }
    }

    fn prepare(&mut self, renderer: GpuInfo, sink: GpuInfo) -> Result<(), DisposableProbeError> {
        self.source = Some(
            CopiedRenderSource::allocate_exact(
                Arc::clone(&self.render_vk),
                Arc::clone(&self.sink_vk),
                self.width,
                self.height,
                CopiedSourcePlan::DrmModifier(crate::kms::vk::dri3::DRM_FORMAT_MOD_LINEAR),
            )
            .map_err(DisposableProbeError::from)?,
        );
        self.sink_destination = Some(
            DrawableImage::new_server_owned_window(
                Arc::clone(&self.sink_vk),
                self.width,
                self.height,
            )
            .map_err(|error| DisposableProbeError::from(io::Error::other(error.to_string())))?,
        );
        self.pattern = Some(
            CopiedProbePatternPipeline::new(Arc::clone(&self.render_vk)).map_err(|error| {
                DisposableProbeError::from(scanout_vk_error("create test pattern", error))
            })?,
        );
        self.render_digest = Some(
            ProbeDigestPipeline::new(Arc::clone(&self.render_vk), self.width, self.height)
                .map_err(|error| {
                    DisposableProbeError::from(scanout_vk_error("create renderer digest", error))
                })?,
        );
        self.sink_digest = Some(
            ProbeDigestPipeline::new(Arc::clone(&self.sink_vk), self.width, self.height).map_err(
                |error| DisposableProbeError::from(scanout_vk_error("create sink digest", error)),
            )?,
        );
        self.allocate_sink_commands()?;
        let source = self
            .source
            .as_ref()
            .ok_or_else(|| DisposableProbeError::from(io::Error::other("copied source missing")))?;
        let transport = source.transport_on_renderer.as_ref().ok_or_else(|| {
            DisposableProbeError::from(io::Error::other("copied transport missing"))
        })?;
        let requirements = unsafe {
            self.render_vk
                .device
                .get_image_memory_requirements(transport.image)
        };
        let properties = unsafe {
            self.render_vk
                .instance
                .get_physical_device_memory_properties(self.render_vk.physical_device)
        };
        let memory_type_index = super::super::super::target::pick_export_memory_type(
            &properties,
            requirements.memory_type_bits,
            super::super::super::target::ExportMemoryPlacement::CrossDevice,
        )
        .ok_or_else(|| {
            DisposableProbeError::from(io::Error::other("no CrossDevice memory type"))
        })?;
        let flags = properties.memory_types[memory_type_index as usize].property_flags;
        eprintln!(
            "two_gpu copied transport CrossDevice preference: renderer={} ({:?}, {:?}), sink={} ({:?}, {:?}), extent={}x{}, memory_type={} flags={flags:?}",
            renderer.name,
            renderer.driver_id,
            renderer.device_type,
            sink.name,
            sink.driver_id,
            sink.device_type,
            self.width,
            self.height,
            memory_type_index,
        );
        Ok(())
    }

    fn allocate_sink_commands(&mut self) -> Result<(), DisposableProbeError> {
        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(self.sink_vk.graphics_queue_family)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        self.sink_command_pool =
            unsafe { self.sink_vk.device.create_command_pool(&pool_info, None) }.map_err(
                |error| {
                    DisposableProbeError::from(scanout_vk_error(
                        "create sink test command pool",
                        error,
                    ))
                },
            )?;
        let allocate_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.sink_command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        self.sink_command_buffer =
            unsafe { self.sink_vk.device.allocate_command_buffers(&allocate_info) }.map_err(
                |error| {
                    DisposableProbeError::from(scanout_vk_error(
                        "allocate sink test command buffer",
                        error,
                    ))
                },
            )?[0];
        Ok(())
    }

    fn execute(&mut self) -> Result<(), DisposableProbeError> {
        let render_fence_handle =
            create_probe_fence(&self.render_vk).map_err(DisposableProbeError::from)?;
        let mut render_fence = ProbeFence::new(&self.render_vk.device, render_fence_handle);
        let sink_fence_handle = match create_probe_fence(&self.sink_vk) {
            Ok(handle) => handle,
            Err(error) => {
                render_fence.destroy_known_idle();
                return Err(DisposableProbeError::from(error));
            }
        };
        let mut sink_fence = ProbeFence::new(&self.sink_vk.device, sink_fence_handle);

        let completion = match (
            self.source.as_mut(),
            self.pattern.as_ref(),
            self.render_digest.as_ref(),
        ) {
            (Some(source), Some(pattern), Some(digest)) => submit_copied_source_probe(
                source,
                pattern,
                CopiedProbeReadback::GpuDigest(digest),
                0,
                render_fence.handle(),
            ),
            _ => Err(DisposableProbeError::from(io::Error::other(
                "two-GPU attempt was executed before all renderer resources were prepared",
            ))),
        };
        let completion = match completion {
            Ok(completion) => completion,
            Err(error) => {
                let pending = if error.requires_quarantine() {
                    PendingProbeSubmissions::Render
                } else {
                    PendingProbeSubmissions::None
                };
                return Err(finish_pending_probe_failure(
                    pending,
                    error,
                    &mut render_fence,
                    &mut sink_fence,
                ));
            }
        };

        let sink_wait =
            match crate::kms::vk::sync::import_optional_sync_file(&self.sink_vk, completion) {
                Ok(semaphore) => semaphore,
                Err(error) => {
                    return Err(finish_pending_probe_failure(
                        PendingProbeSubmissions::Render,
                        DisposableProbeError::from(scanout_vk_error(
                            "import test renderer completion on sink",
                            error,
                        )),
                        &mut render_fence,
                        &mut sink_fence,
                    ));
                }
            };
        if let Some(source) = self.source.as_mut() {
            source.sink_wait_semaphore = Some(sink_wait);
        }

        let sink_recording = self.record_sink_copy();
        if let Err(error) = sink_recording {
            return Err(finish_pending_probe_failure(
                PendingProbeSubmissions::Render,
                DisposableProbeError::from(error),
                &mut render_fence,
                &mut sink_fence,
            ));
        }
        let wait_infos = [vk::SemaphoreSubmitInfo::default()
            .semaphore(sink_wait)
            .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
        let command_infos =
            [vk::CommandBufferSubmitInfo::default().command_buffer(self.sink_command_buffer)];
        let submits = [vk::SubmitInfo2::default()
            .wait_semaphore_infos(&wait_infos)
            .command_buffer_infos(&command_infos)];
        if let Err(error) = unsafe {
            self.sink_vk.device.queue_submit2(
                self.sink_vk.graphics_queue,
                &submits,
                sink_fence.handle(),
            )
        } {
            return Err(finish_pending_probe_failure(
                PendingProbeSubmissions::RenderAndSink,
                DisposableProbeError::quarantined(scanout_vk_error("submit test sink copy", error)),
                &mut render_fence,
                &mut sink_fence,
            ));
        }
        if let Some(source) = self.source.as_mut() {
            source.note_sink_submit_succeeded();
        }

        wait_copied_probe_fence_pair(&mut render_fence, &mut sink_fence, TWO_GPU_FENCE_TIMEOUT_NS)?;

        let render_summary = self
            .render_digest
            .as_ref()
            .ok_or_else(|| DisposableProbeError::from(io::Error::other("renderer digest missing")))?
            .read_summary()
            .map_err(|error| {
                if error == vk::Result::ERROR_DEVICE_LOST {
                    DisposableProbeError::quarantined(scanout_vk_error(
                        "read test renderer digest",
                        error,
                    ))
                } else {
                    copied_probe_digest_readback_error("read test renderer digest", error)
                }
            })?;
        let sink_summary = self
            .sink_digest
            .as_ref()
            .ok_or_else(|| DisposableProbeError::from(io::Error::other("sink digest missing")))?
            .read_summary()
            .map_err(|error| {
                if error == vk::Result::ERROR_DEVICE_LOST {
                    DisposableProbeError::quarantined(scanout_vk_error(
                        "read test sink digest",
                        error,
                    ))
                } else {
                    copied_probe_digest_readback_error("read test sink digest", error)
                }
            })?;
        completed_probe_validation(validate_copied_probe_digest_fiducials(
            &render_summary,
            0,
            0,
            0,
        ))?;
        completed_probe_validation(verify_copied_probe_digests(
            &render_summary,
            &sink_summary,
            self.render_digest
                .as_ref()
                .map_or(0, ProbeDigestPipeline::grid_width),
            self.render_digest
                .as_ref()
                .map_or(0, ProbeDigestPipeline::grid_height),
            0,
            0,
            0,
        ))?;
        Ok(())
    }

    fn record_sink_copy(&self) -> io::Result<()> {
        let source = self
            .source
            .as_ref()
            .ok_or_else(|| io::Error::other("copied source missing"))?;
        let destination = self
            .sink_destination
            .as_ref()
            .ok_or_else(|| io::Error::other("sink destination missing"))?;
        let command_buffer = self.sink_command_buffer;
        unsafe {
            self.sink_vk
                .device
                .reset_command_buffer(command_buffer, vk::CommandBufferResetFlags::empty())
                .map_err(|error| scanout_vk_error("reset sink test command buffer", error))?;
            self.sink_vk
                .device
                .begin_command_buffer(
                    command_buffer,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(|error| scanout_vk_error("begin sink test command buffer", error))?;

            let acquire = [
                vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                    .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
                    .dst_stage_mask(vk::PipelineStageFlags2::COPY)
                    .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
                    .src_queue_family_index(vk::QUEUE_FAMILY_FOREIGN_EXT)
                    .dst_queue_family_index(self.sink_vk.graphics_queue_family)
                    .old_layout(vk::ImageLayout::GENERAL)
                    .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .image(source.imported_sink_image())
                    .subresource_range(color_subresource_range()),
                vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::TOP_OF_PIPE)
                    .src_access_mask(vk::AccessFlags2::empty())
                    .dst_stage_mask(vk::PipelineStageFlags2::COPY)
                    .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .old_layout(vk::ImageLayout::UNDEFINED)
                    .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .image(destination.vk_image)
                    .subresource_range(color_subresource_range()),
            ];
            self.sink_vk.device.cmd_pipeline_barrier2(
                command_buffer,
                &vk::DependencyInfo::default().image_memory_barriers(&acquire),
            );
            let region = [vk::ImageCopy::default()
                .src_subresource(color_subresource_layers())
                .dst_subresource(color_subresource_layers())
                .extent(vk::Extent3D {
                    width: self.width,
                    height: self.height,
                    depth: 1,
                })];
            self.sink_vk.device.cmd_copy_image(
                command_buffer,
                source.imported_sink_image(),
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                destination.vk_image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &region,
            );
            let post_copy = [
                vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::COPY)
                    .src_access_mask(vk::AccessFlags2::TRANSFER_READ)
                    .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                    .dst_access_mask(vk::AccessFlags2::MEMORY_READ)
                    .src_queue_family_index(self.sink_vk.graphics_queue_family)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_FOREIGN_EXT)
                    .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .new_layout(vk::ImageLayout::GENERAL)
                    .image(source.imported_sink_image())
                    .subresource_range(color_subresource_range()),
                vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::COPY)
                    .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                    .dst_stage_mask(vk::PipelineStageFlags2::COPY)
                    .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .image(destination.vk_image)
                    .subresource_range(color_subresource_range()),
            ];
            self.sink_vk.device.cmd_pipeline_barrier2(
                command_buffer,
                &vk::DependencyInfo::default().image_memory_barriers(&post_copy),
            );
            let digest = self
                .sink_digest
                .as_ref()
                .ok_or_else(|| io::Error::other("sink digest missing"))?;
            let readback = [vk::BufferImageCopy::default()
                .buffer_offset(0)
                .buffer_row_length(0)
                .buffer_image_height(0)
                .image_subresource(color_subresource_layers())
                .image_offset(vk::Offset3D::default())
                .image_extent(vk::Extent3D {
                    width: self.width,
                    height: self.height,
                    depth: 1,
                })];
            self.sink_vk.device.cmd_copy_image_to_buffer(
                command_buffer,
                destination.vk_image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                digest.input_buffer(),
                &readback,
            );
            digest.record_after_transfer(command_buffer);
            self.sink_vk
                .device
                .end_command_buffer(command_buffer)
                .map_err(|error| scanout_vk_error("end sink test command buffer", error))?;
        }
        Ok(())
    }
}

impl Drop for TwoGpuAttempt {
    fn drop(&mut self) {
        if self.sink_command_pool != vk::CommandPool::null() {
            unsafe {
                self.sink_vk
                    .device
                    .destroy_command_pool(self.sink_command_pool, None);
            }
            self.sink_command_pool = vk::CommandPool::null();
        }
    }
}

impl DisposableProbeAttempt for TwoGpuAttempt {
    fn mark_known_quiescent(&self) {
        self.render_vk.mark_disposable_probe_quiescent();
        self.sink_vk.mark_disposable_probe_quiescent();
    }

    fn release_strict_drm_resources(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The `--ignored` suite also runs on GPU-less CI with lavapipe as the only
/// ICD; like the other hardware tests, these skip when their devices are absent.
/// Once both are present, every failure is a test failure.
fn gpu_pair_or_skip() -> bool {
    let gpus = match enumerate_test_gpus() {
        Ok(gpus) => gpus,
        Err(error) => {
            eprintln!("skipping: no Vulkan devices: {error}");
            return false;
        }
    };
    let has_nvidia = gpus.iter().any(|gpu| gpu.vendor_id == NVIDIA_VENDOR_ID);
    let has_radv = gpus
        .iter()
        .any(|gpu| gpu.vendor_id == AMD_VENDOR_ID && gpu.driver_id == vk::DriverId::MESA_RADV);
    if !(has_nvidia && has_radv) {
        eprintln!("skipping: needs an NVIDIA and an AMD RADV Vulkan device; found {gpus:?}");
        return false;
    }
    true
}

fn run_two_gpu_transfer(render_vendor: u32, width: u32, height: u32) -> Result<(), String> {
    let (renderer, sink, render_vk, sink_vk) =
        two_gpu_contexts(render_vendor).map_err(|error| error.to_string())?;
    let mut attempt = TwoGpuAttempt::new(render_vk, sink_vk, width, height);
    let result = attempt
        .prepare(renderer, sink)
        .and_then(|()| attempt.execute());
    finish_disposable_probe_attempt(attempt, result).map_err(|error| error.to_string())
}

#[test]
#[ignore = "needs the NVIDIA and AMD Vulkan devices"]
fn copied_source_transfers_from_igpu_to_dgpu_two_gpu() {
    if !gpu_pair_or_skip() {
        return;
    }
    for (width, height) in [(1920, 1080), (1600, 1200)] {
        run_two_gpu_transfer(AMD_VENDOR_ID, width, height).unwrap_or_else(|error| {
            panic!("AMD RADV renderer -> NVIDIA sink {width}x{height} failed: {error}")
        });
    }
}

#[test]
#[ignore = "needs the NVIDIA and AMD Vulkan devices"]
fn copied_source_transfers_across_gpus_two_gpu() {
    if !gpu_pair_or_skip() {
        return;
    }
    run_two_gpu_transfer(NVIDIA_VENDOR_ID, 1920, 1080).unwrap_or_else(|error| {
        panic!("NVIDIA renderer -> AMD RADV sink 1920x1080 failed: {error}")
    });
}

#[test]
#[ignore = "needs the NVIDIA and AMD Vulkan devices"]
fn copied_source_imports_on_the_sink_at_unaligned_widths_two_gpu() {
    if !gpu_pair_or_skip() {
        return;
    }
    let mut failures = Vec::new();
    for (width, height) in [
        (1366, 768),
        (1440, 900),
        (1680, 1050),
        (1920, 1080),
        (2560, 1440),
    ] {
        match run_two_gpu_transfer(NVIDIA_VENDOR_ID, width, height) {
            Ok(()) => eprintln!("two_gpu sink import and logical digest {width}x{height}: passed"),
            Err(error) => {
                eprintln!(
                    "two_gpu sink import and logical digest {width}x{height}: failed: {error}"
                );
                failures.push(format!("{width}x{height}: {error}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "two-GPU copied import failures: {failures:#?}"
    );
}
