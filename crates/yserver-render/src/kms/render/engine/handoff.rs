//! Asynchronous readback of a drawable into host memory (rootless CPU frames).

use crate::kms::{
    render::{
        engine::{
            RenderEngine, RenderError, RenderFlushReason, StagingBuffer, SubmittedOp,
            fill_copy::clamp_rect_to,
            frame::{begin_op_cb, end_and_submit_op},
        },
        frame_builder::CloseReason,
        gpu::{FenceTicket, GpuCore},
        store::{DrawableId, DrawableStore},
        submit_group::FlushReason,
    },
    vk::device::VkContext,
};
use ash::vk;
use std::sync::Arc;

/// A submitted copy of rects of a drawable into host memory. Never waits.
pub struct PendingReadback {
    /// `None`: every rect was empty, nothing was submitted.
    ticket: Option<FenceTicket>,
    staging: Option<Arc<StagingBuffer>>,
    /// (rect in drawable pixels, byte offset in `staging`), tightly packed BGRA rows.
    regions: Vec<(vk::Rect2D, u64)>,
}

impl PendingReadback {
    /// Whether the copy finished. A poll, never a wait.
    ///
    /// # Errors
    ///
    /// The fence status query failed (device lost).
    pub fn poll(&self, vk: &VkContext) -> Result<bool, vk::Result> {
        self.ticket
            .as_ref()
            .map_or(Ok(true), |t| t.poll_signaled_result(vk))
    }

    /// Copy every region into `dst` (stride `dst_stride`, BGRA). Only after
    /// [`Self::poll`] returned true. Regions outside `dst` are skipped.
    ///
    /// # Errors
    ///
    /// The cache invalidation of the staging memory failed.
    pub fn write_into(&self, dst: &mut [u8], dst_stride: usize) -> Result<(), vk::Result> {
        let Some(staging) = &self.staging else {
            return Ok(());
        };
        staging.invalidate_for_read()?;
        // SAFETY: mapped for `size` bytes; the fence signalled (caller polled)
        // and the invalidate above made the GPU's writes visible.
        let raw: &[u8] =
            unsafe { std::slice::from_raw_parts(staging.mapped.as_ptr(), staging.size as usize) };
        for &(rect, offset) in &self.regions {
            let row = rect.extent.width as usize * 4;
            let x = rect.offset.x as usize * 4;
            let y = rect.offset.y as usize;
            for r in 0..rect.extent.height as usize {
                let src = offset as usize + r * row;
                let at = (y + r) * dst_stride + x;
                let (Some(from), Some(to)) = (raw.get(src..src + row), dst.get_mut(at..at + row))
                else {
                    debug_assert!(false, "readback region outside the destination");
                    continue;
                };
                to.copy_from_slice(from);
            }
        }
        Ok(())
    }

    pub fn regions(&self) -> impl Iterator<Item = vk::Rect2D> + '_ {
        self.regions.iter().map(|(r, _)| *r)
    }
}

impl RenderEngine {
    /// `get_image`'s flush + transition + copy + submit for several rects of a
    /// depth-24/32 drawable, without the wait. Rects are clamped to the
    /// extent; empty rects are dropped. Touches the drawable's render fence,
    /// so its storage outlives the copy.
    ///
    /// # Errors
    ///
    /// `UnknownDrawable`, `UnsupportedDepth` (not 24/32), `Vk` for
    /// allocation, recording or submit failures.
    pub fn begin_readback(
        &mut self,
        store: &mut DrawableStore,
        gpu: &mut GpuCore,
        src: DrawableId,
        rects: &[vk::Rect2D],
    ) -> Result<PendingReadback, RenderError> {
        self.flush_render_batch(store, gpu, RenderFlushReason::Readback)?;
        self.close_open_frame(store, gpu, CloseReason::NonPortedPaintOp)?;
        self.flush_submit_group(store, gpu, FlushReason::SyncBoundary)
            .map_err(RenderError::Vk)?;
        let Some(inner) = self.inner.as_mut() else {
            return Err(RenderError::NoVk);
        };
        if gpu.renderer_failed {
            return Err(RenderError::RendererFailed);
        }
        let Some(drawable) = store.get_mut(src) else {
            return Err(RenderError::UnknownDrawable(src));
        };
        if !matches!(drawable.depth, 24 | 32) {
            return Err(RenderError::UnsupportedDepth(drawable.depth));
        }
        let bounds = vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent: drawable.storage.extent,
        };
        let mut regions = Vec::with_capacity(rects.len());
        let mut size = 0u64;
        for r in rects {
            let c = clamp_rect_to(*r, bounds);
            if c.extent.width == 0 || c.extent.height == 0 {
                continue;
            }
            regions.push((c, size));
            size += u64::from(c.extent.width) * u64::from(c.extent.height) * 4;
        }
        if regions.is_empty() {
            return Ok(PendingReadback {
                ticket: None,
                staging: None,
                regions,
            });
        }
        let staging = Arc::new(StagingBuffer::new_for_readback(inner.vk.clone(), size)?);
        let (cb, ticket) = begin_op_cb(inner, gpu)?;
        drawable.record_layout_transition(
            &inner.vk,
            cb,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            vk::PipelineStageFlags2::ALL_COMMANDS,
            vk::AccessFlags2::SHADER_SAMPLED_READ
                | vk::AccessFlags2::TRANSFER_WRITE
                | vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
            vk::PipelineStageFlags2::COPY,
            vk::AccessFlags2::TRANSFER_READ,
        );
        let copies: Vec<_> = regions
            .iter()
            .map(|&(r, offset)| {
                vk::BufferImageCopy::default()
                    .buffer_offset(offset)
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .layer_count(1),
                    )
                    .image_offset(vk::Offset3D {
                        x: r.offset.x,
                        y: r.offset.y,
                        z: 0,
                    })
                    .image_extent(vk::Extent3D {
                        width: r.extent.width,
                        height: r.extent.height,
                        depth: 1,
                    })
            })
            .collect();
        unsafe {
            inner.vk.device.cmd_copy_image_to_buffer(
                cb,
                drawable.storage.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                staging.buffer,
                &copies,
            );
        }
        drawable.record_layout_transition(
            &inner.vk,
            cb,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            vk::PipelineStageFlags2::COPY,
            vk::AccessFlags2::TRANSFER_READ,
            vk::PipelineStageFlags2::FRAGMENT_SHADER,
            vk::AccessFlags2::SHADER_SAMPLED_READ,
        );
        end_and_submit_op(inner, gpu, cb, &ticket)?;
        store.touch_render_fence(src, ticket.clone());
        inner.acquire_generation += 1;
        let generation = inner.acquire_generation;
        inner.pending_group_ops.push(SubmittedOp {
            cb,
            ticket: ticket.clone(),
            staging: Some(staging.clone()),
            scratch: Vec::new(),
            sampled_scratch: Vec::new(),
            atlas_ticket: None,
            generation,
            retired_resources: Vec::new(),
        });
        self.flush_submit_group(store, gpu, FlushReason::SyncBoundary)
            .map_err(RenderError::Vk)?;
        Ok(PendingReadback {
            ticket: Some(ticket),
            staging: Some(staging),
            regions,
        })
    }
}
