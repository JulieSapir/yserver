//! Lavapipe checks for the `VkContextOptions` gates.

use std::sync::Arc;

use ash::vk;

use super::{
    device::{VkContext, VkContextOptions},
    target::{EXPORT_FORMAT_BGRA8, allocate_exportable},
};

const HEADLESS_PRESENT: VkContextOptions = VkContextOptions {
    wants_present: true,
    surface_extension: Some(ash::ext::headless_surface::NAME),
    wants_kms_interop: false,
};

fn vk_or_skip(options: VkContextOptions) -> Option<Arc<VkContext>> {
    match VkContext::new_with_options(options) {
        Ok(vk) => Some(vk),
        Err(e) => {
            eprintln!("skipping: no Vk: {e:?}");
            None
        }
    }
}

#[test]
#[ignore = "needs live Vulkan ICD"]
fn headless_present_context_can_present_from_its_graphics_queue() {
    let Some(vk) = vk_or_skip(HEADLESS_PRESENT) else {
        return;
    };
    assert!(vk.swapchain, "wants_present enables VK_KHR_swapchain");
    let headless = ash::ext::headless_surface::Instance::new(&vk.entry, &vk.instance);
    let surface_fns = ash::khr::surface::Instance::new(&vk.entry, &vk.instance);
    let surface = unsafe {
        headless.create_headless_surface(&vk::HeadlessSurfaceCreateInfoEXT::default(), None)
    }
    .expect("vkCreateHeadlessSurfaceEXT");
    let supported = unsafe {
        surface_fns.get_physical_device_surface_support(
            vk.physical_device,
            vk.graphics_queue_family,
            surface,
        )
    };
    unsafe { surface_fns.destroy_surface(surface, None) };
    assert_eq!(
        supported,
        Ok(true),
        "graphics queue presents to the surface"
    );
}

#[test]
#[ignore = "needs live Vulkan ICD"]
fn surface_context_picks_a_device_presenting_to_the_probed_surface() {
    let mut probes = 0;
    let mut probe = |entry: &ash::Entry, instance: &ash::Instance| {
        probes += 1;
        let headless = ash::ext::headless_surface::Instance::new(entry, instance);
        unsafe {
            headless.create_headless_surface(&vk::HeadlessSurfaceCreateInfoEXT::default(), None)
        }
    };
    let vk = match VkContext::new_with_surface(HEADLESS_PRESENT, &mut probe) {
        Ok(vk) => vk,
        Err(e) => {
            eprintln!("skipping: no Vk: {e:?}");
            return;
        }
    };
    assert_eq!(probes, 1, "the device pick probed the surface once");
    assert!(vk.swapchain);

    let mut failing = |_: &ash::Entry, _: &ash::Instance| Err(vk::Result::ERROR_SURFACE_LOST_KHR);
    assert!(
        VkContext::new_with_surface(HEADLESS_PRESENT, &mut failing).is_err(),
        "no surface, no presenting context"
    );
}

#[test]
#[ignore = "needs live Vulkan ICD"]
fn kms_context_keeps_dmabuf_export_and_no_swapchain() {
    let Some(vk) = vk_or_skip(VkContextOptions::KMS) else {
        return;
    };
    assert!(!vk.swapchain);
    // Drivers on macOS (KosmicKrisp) have no dma-buf at all.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    assert!(vk.dmabuf_export, "lavapipe exposes dma-buf export");
}

#[test]
#[ignore = "needs live Vulkan ICD"]
fn surface_context_refuses_dmabuf_export() {
    let Some(vk) = vk_or_skip(HEADLESS_PRESENT) else {
        return;
    };
    assert!(!vk.dmabuf_export);
    assert!(!vk.supports_sync_fd());
    assert_eq!(
        allocate_exportable(&vk, 16, 16, EXPORT_FORMAT_BGRA8).err(),
        Some(vk::Result::ERROR_FORMAT_NOT_SUPPORTED),
    );
    assert!(vk.tfp_tiling_strategy.get().is_none(), "no strategy probed");
}

/// Plan RL1 Task 7a: a presenting context enables swapchain maintenance1
/// exactly when the device offers it (lavapipe does, measured 2026-10-19).
#[test]
#[ignore = "needs live Vulkan ICD"]
fn presenting_context_enables_swapchain_maintenance1_when_offered() {
    let Some(vk) = vk_or_skip(HEADLESS_PRESENT) else {
        return;
    };
    let offered = unsafe {
        vk.instance
            .enumerate_device_extension_properties(vk.physical_device)
    }
    .expect("device extensions")
    .iter()
    .any(|p| {
        p.extension_name_as_c_str().is_ok_and(|n| {
            n == ash::ext::swapchain_maintenance1::NAME || n == c"VK_KHR_swapchain_maintenance1"
        })
    });
    assert_eq!(vk.swapchain_maintenance1, offered);
    if vk.is_software_rasterizer() {
        assert!(vk.swapchain_maintenance1, "lavapipe offers it");
    }
    let plain = vk_or_skip(VkContextOptions {
        wants_present: false,
        surface_extension: None,
        wants_kms_interop: false,
    })
    .expect("plain context");
    assert!(!plain.swapchain_maintenance1, "only a presenting context");
}

/// Validation errors are counted per context, so parallel tests do not see
/// each other's (VUID-VkImageCreateInfo-extent-00944: a zero extent).
#[test]
#[ignore = "needs live Vulkan ICD"]
fn validation_errors_are_counted_per_context() {
    let options = VkContextOptions {
        wants_present: false,
        surface_extension: None,
        wants_kms_interop: false,
    };
    let (Some(a), Some(b)) = (vk_or_skip(options), vk_or_skip(options)) else {
        return;
    };
    if !a.validation_enabled() {
        eprintln!("validation layer off: nothing to count");
        return;
    }
    assert_eq!(a.validation_errors(), 0);
    let info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(vk::Format::B8G8R8A8_UNORM)
        .extent(vk::Extent3D {
            width: 0,
            height: 0,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::TRANSFER_DST);
    if let Ok(image) = unsafe { a.device.create_image(&info, None) } {
        unsafe { a.device.destroy_image(image, None) };
    }
    assert!(a.validation_errors() >= 1, "the invalid call was counted");
    assert_eq!(b.validation_errors(), 0, "another context's count");
}
