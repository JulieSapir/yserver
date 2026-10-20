//! The swapchain core a rooted output and a rootless window share: one
//! swapchain on a surface the caller owns, its per-image present semaphores,
//! creation (replacing an old one), a non-blocking acquire and the present.
//! Retiring or destroying an old swapchain is the caller's choice.

use std::sync::Arc;

use ash::vk;

use crate::kms::vk::device::VkContext;

/// How the window system blends the images.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum AlphaMode {
    /// Rooted outputs and depth-24 windows.
    Opaque,
    /// Depth-32 windows: X ARGB visuals are premultiplied (spec §4.5).
    Premultiplied,
}

/// The composite alpha for `mode` among `supported`.
pub(crate) fn pick_alpha(
    supported: vk::CompositeAlphaFlagsKHR,
    mode: AlphaMode,
) -> vk::CompositeAlphaFlagsKHR {
    use vk::CompositeAlphaFlagsKHR as A;
    // Rooted order (OPAQUE first) as before the split.
    let order: &[A] = match mode {
        AlphaMode::Opaque => &[A::OPAQUE, A::INHERIT, A::PRE_MULTIPLIED, A::POST_MULTIPLIED],
        AlphaMode::Premultiplied => &[A::PRE_MULTIPLIED, A::OPAQUE, A::INHERIT, A::POST_MULTIPLIED],
    };
    order
        .iter()
        .copied()
        .find(|flag| supported.contains(*flag))
        .unwrap_or(A::OPAQUE)
}

/// A swapchain and its per-image present semaphores.
pub(crate) struct Swapchain {
    pub(crate) handle: vk::SwapchainKHR,
    pub(crate) images: Vec<vk::Image>,
    pub(crate) extent: vk::Extent2D,
    /// Signalled by the copy into image `i`, waited by its present.
    pub(crate) ready: Vec<vk::Semaphore>,
}

/// A non-blocking acquire's outcome.
pub(crate) enum Acquire {
    Image {
        index: u32,
        suboptimal: bool,
    },
    /// No image free yet (`NOT_READY`/`TIMEOUT`).
    NotReady,
    OutOfDate,
}

/// A present's outcome; its semaphore wait executes either way.
pub(crate) enum Present {
    Queued { suboptimal: bool },
    OutOfDate,
}

pub(crate) struct Chain {
    vk: Arc<VkContext>,
    surface_fns: ash::khr::surface::Instance,
    swapchain_fns: ash::khr::swapchain::Device,
    surface: vk::SurfaceKHR,
    current: Option<Swapchain>,
    /// Usage beyond `TRANSFER_DST` for the next swapchain (test reads).
    #[cfg(test)]
    pub(crate) extra_usage: vk::ImageUsageFlags,
}

impl Chain {
    /// Swapchains on `surface`, which the caller owns and destroys after
    /// the last of them.
    pub(crate) fn new(vk: &Arc<VkContext>, surface: vk::SurfaceKHR) -> Self {
        Self {
            vk: Arc::clone(vk),
            surface_fns: ash::khr::surface::Instance::new(&vk.entry, &vk.instance),
            swapchain_fns: ash::khr::swapchain::Device::new(&vk.instance, &vk.device),
            surface,
            current: None,
            #[cfg(test)]
            extra_usage: vk::ImageUsageFlags::empty(),
        }
    }

    pub(crate) fn surface_fns(&self) -> &ash::khr::surface::Instance {
        &self.surface_fns
    }

    pub(crate) fn current(&self) -> Option<&Swapchain> {
        self.current.as_ref()
    }

    /// Take the current swapchain out; the caller destroys or retires it.
    pub(crate) fn take(&mut self) -> Option<Swapchain> {
        self.current.take()
    }

    pub(crate) fn capabilities(&self) -> Result<vk::SurfaceCapabilitiesKHR, vk::Result> {
        // SAFETY: the surface belongs to this instance.
        unsafe {
            self.surface_fns
                .get_physical_device_surface_capabilities(self.vk.physical_device, self.surface)
        }
    }

    pub(crate) fn formats(&self) -> Result<Vec<vk::SurfaceFormatKHR>, vk::Result> {
        // SAFETY: the surface belongs to this instance.
        unsafe {
            self.surface_fns
                .get_physical_device_surface_formats(self.vk.physical_device, self.surface)
        }
    }

    /// Create the current swapchain at `extent`, replacing `old` (passed as
    /// `oldSwapchain`, which the caller still owns).
    pub(crate) fn create(
        &mut self,
        caps: &vk::SurfaceCapabilitiesKHR,
        format: vk::SurfaceFormatKHR,
        extent: vk::Extent2D,
        alpha: AlphaMode,
        old: Option<&Swapchain>,
    ) -> Result<(), vk::Result> {
        #[cfg(test)]
        let usage = vk::ImageUsageFlags::TRANSFER_DST | self.extra_usage;
        #[cfg(not(test))]
        let usage = vk::ImageUsageFlags::TRANSFER_DST;
        let mut image_count = caps.min_image_count.max(3);
        if caps.max_image_count != 0 {
            image_count = image_count.min(caps.max_image_count);
        }
        let info = vk::SwapchainCreateInfoKHR::default()
            .surface(self.surface)
            .min_image_count(image_count)
            .image_format(format.format)
            .image_color_space(format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(usage)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(caps.current_transform)
            .composite_alpha(pick_alpha(caps.supported_composite_alpha, alpha))
            .present_mode(vk::PresentModeKHR::FIFO)
            .clipped(true)
            .old_swapchain(old.map_or(vk::SwapchainKHR::null(), |s| s.handle));
        // SAFETY: valid surface and (optional) retired old swapchain.
        let handle = unsafe { self.swapchain_fns.create_swapchain(&info, None) }?;
        let images = match unsafe { self.swapchain_fns.get_swapchain_images(handle) } {
            Ok(images) => images,
            Err(e) => {
                unsafe { self.swapchain_fns.destroy_swapchain(handle, None) };
                return Err(e);
            }
        };
        let mut ready = Vec::with_capacity(images.len());
        for _ in &images {
            match unsafe {
                self.vk
                    .device
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
            } {
                Ok(semaphore) => ready.push(semaphore),
                Err(e) => {
                    self.destroy(Some(Swapchain {
                        handle,
                        images,
                        extent,
                        ready,
                    }));
                    return Err(e);
                }
            }
        }
        log::info!(
            "surface: swapchain {}x{} with {} images ({:?})",
            extent.width,
            extent.height,
            images.len(),
            format.format
        );
        self.current = Some(Swapchain {
            handle,
            images,
            extent,
            ready,
        });
        Ok(())
    }

    /// Acquire the next image without waiting, signalling `semaphore`.
    pub(crate) fn acquire(&self, semaphore: vk::Semaphore) -> Result<Acquire, vk::Result> {
        let Some(swapchain) = self.current.as_ref() else {
            return Ok(Acquire::NotReady);
        };
        // SAFETY: the swapchain and an unsignalled semaphore of this device.
        match unsafe {
            self.swapchain_fns
                .acquire_next_image(swapchain.handle, 0, semaphore, vk::Fence::null())
        } {
            Ok((index, suboptimal)) => Ok(Acquire::Image { index, suboptimal }),
            Err(vk::Result::NOT_READY | vk::Result::TIMEOUT) => Ok(Acquire::NotReady),
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => Ok(Acquire::OutOfDate),
            Err(e) => Err(e),
        }
    }

    /// Present image `index` of the current swapchain after its ready
    /// semaphore. With `fence` (swapchain maintenance1) the presentation
    /// engine signals it once it is done with the present's semaphore and
    /// image.
    pub(crate) fn present_with_fence(
        &self,
        index: u32,
        fence: Option<vk::Fence>,
    ) -> Result<Present, vk::Result> {
        let Some(swapchain) = self.current.as_ref() else {
            return Err(vk::Result::ERROR_OUT_OF_DATE_KHR);
        };
        let swapchains = [swapchain.handle];
        let indices = [index];
        let waits = [swapchain.ready[index as usize]];
        let fences = [fence.unwrap_or_default()];
        let mut fence_info = vk::SwapchainPresentFenceInfoEXT::default().fences(&fences);
        let mut present = vk::PresentInfoKHR::default()
            .wait_semaphores(&waits)
            .swapchains(&swapchains)
            .image_indices(&indices);
        if fence.is_some() {
            present = present.push_next(&mut fence_info);
        }
        // SAFETY: the image was acquired and its copy submitted.
        match unsafe {
            self.swapchain_fns
                .queue_present(self.vk.graphics_queue, &present)
        } {
            Ok(suboptimal) => Ok(Present::Queued { suboptimal }),
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => Ok(Present::OutOfDate),
            Err(e) => Err(e),
        }
    }

    /// Destroy a swapchain whose presents and copies have completed.
    pub(crate) fn destroy(&self, swapchain: Option<Swapchain>) {
        let Some(swapchain) = swapchain else {
            return;
        };
        unsafe {
            for semaphore in swapchain.ready {
                self.vk.device.destroy_semaphore(semaphore, None);
            }
            self.swapchain_fns.destroy_swapchain(swapchain.handle, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use ash::vk::CompositeAlphaFlagsKHR as A;

    use super::{AlphaMode, pick_alpha};

    #[test]
    fn opaque_mode_keeps_todays_order() {
        assert_eq!(
            pick_alpha(A::INHERIT | A::OPAQUE, AlphaMode::Opaque),
            A::OPAQUE
        );
        assert_eq!(
            pick_alpha(A::POST_MULTIPLIED | A::INHERIT, AlphaMode::Opaque),
            A::INHERIT
        );
        assert_eq!(
            pick_alpha(A::POST_MULTIPLIED | A::PRE_MULTIPLIED, AlphaMode::Opaque),
            A::PRE_MULTIPLIED
        );
    }

    /// Spec §4.5: X ARGB visuals are premultiplied.
    #[test]
    fn premultiplied_mode_prefers_premultiplied() {
        assert_eq!(
            pick_alpha(A::OPAQUE | A::PRE_MULTIPLIED, AlphaMode::Premultiplied),
            A::PRE_MULTIPLIED
        );
        assert_eq!(pick_alpha(A::OPAQUE, AlphaMode::Premultiplied), A::OPAQUE);
    }
}
