use std::{
    io,
    os::fd::{AsFd as _, AsRawFd as _},
};

use crate::drm::Device;

/// Client capability requests needed while initializing a KMS device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DeviceClientCapability {
    UniversalPlanes,
    Atomic,
    CursorPlaneHotspot,
}

/// Cached driver facts that affect atomic cursor-plane eligibility.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CursorDeviceCaps {
    hotspot_cap_accepted: bool,
    driver_name: Option<String>,
}

impl CursorDeviceCaps {
    pub(crate) fn new(hotspot_cap_accepted: bool, driver_name: Option<String>) -> Self {
        Self {
            hotspot_cap_accepted,
            driver_name,
        }
    }

    /// Whether this fd accepted `DRM_CLIENT_CAP_CURSOR_PLANE_HOTSPOT`.
    pub const fn cursor_plane_hotspot_cap_accepted(&self) -> bool {
        self.hotspot_cap_accepted
    }

    /// Kernel DRM driver name, or `None` when the driver query failed.
    pub fn driver_name(&self) -> Option<&str> {
        self.driver_name.as_deref()
    }

    /// Whether the driver is one of the exact names that needs hotspot support.
    pub fn is_paravirtualized(&self) -> bool {
        matches!(
            self.driver_name(),
            Some("virtio_gpu" | "vmwgfx" | "qxl" | "vboxvideo")
        )
    }

    /// Whether the cursor plane must expose hotspot properties for atomic use.
    pub fn requires_hotspot_support(&self) -> bool {
        self.is_paravirtualized() || self.driver_name.is_none()
    }
}

/// Operations performed during primary KMS-device initialization.
pub(crate) trait DeviceControl: Send + Sync {
    fn set_client_capability(
        &self,
        device: &Device,
        capability: DeviceClientCapability,
    ) -> io::Result<()>;

    fn driver_name(&self, device: &Device) -> io::Result<String>;

    fn crtc_in_vblank_event(&self, device: &Device) -> io::Result<u64>;
}

/// Production implementation of KMS capability and driver queries.
pub(crate) struct DrmDeviceControl;

impl DeviceControl for DrmDeviceControl {
    fn set_client_capability(
        &self,
        device: &Device,
        capability: DeviceClientCapability,
    ) -> io::Result<()> {
        match capability {
            DeviceClientCapability::UniversalPlanes => {
                use ::drm::Device as _;
                device.set_client_capability(::drm::ClientCapability::UniversalPlanes, true)
            }
            DeviceClientCapability::Atomic => {
                use ::drm::Device as _;
                device.set_client_capability(::drm::ClientCapability::Atomic, true)
            }
            DeviceClientCapability::CursorPlaneHotspot => set_cursor_plane_hotspot_cap(device),
        }
    }

    fn driver_name(&self, device: &Device) -> io::Result<String> {
        use ::drm::Device as _;
        Ok(device.get_driver()?.name().to_string_lossy().into_owned())
    }

    fn crtc_in_vblank_event(&self, device: &Device) -> io::Result<u64> {
        use ::drm::Device as _;
        device.get_driver_capability(::drm::DriverCapability::CRTCInVBlankEvent)
    }
}

#[repr(C)]
struct DrmSetClientCap {
    capability: u64,
    value: u64,
}

// _IOW('d', 0x0d, struct drm_set_client_cap), a 16-byte payload.
const DRM_IOCTL_SET_CLIENT_CAP: nix::sys::ioctl::ioctl_num_type =
    nix::request_code_write!(b'd', 0x0d, std::mem::size_of::<DrmSetClientCap>());
const DRM_CLIENT_CAP_CURSOR_PLANE_HOTSPOT: u64 = 6;

fn set_cursor_plane_hotspot_cap(device: &Device) -> io::Result<()> {
    let mut request = DrmSetClientCap {
        capability: DRM_CLIENT_CAP_CURSOR_PLANE_HOTSPOT,
        value: 1,
    };
    // Keep the request's unsigned bit pattern while letting libc select each target's type.
    let result = unsafe {
        libc::ioctl(
            device.as_fd().as_raw_fd(),
            DRM_IOCTL_SET_CLIENT_CAP as _,
            &mut request,
        )
    };
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Negotiate cursor-specific capabilities and cache the driver name.
pub(crate) fn negotiate_cursor_caps(
    device: &Device,
    control: &dyn DeviceControl,
) -> CursorDeviceCaps {
    let hotspot_cap_accepted =
        match control.set_client_capability(device, DeviceClientCapability::CursorPlaneHotspot) {
            Ok(()) => true,
            Err(err) => {
                log::debug!("DRM_CLIENT_CAP_CURSOR_PLANE_HOTSPOT rejected ({err})");
                false
            }
        };
    let driver_name = match control.driver_name(device) {
        Ok(name) => Some(name),
        Err(err) => {
            log::warn!("failed to query DRM driver name: {err}");
            None
        }
    };
    CursorDeviceCaps::new(hotspot_cap_accepted, driver_name)
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DeviceControlCall {
    SetClientCapability(DeviceClientCapability),
    DriverName,
    CrtcInVBlankEvent,
}

#[cfg(test)]
pub(crate) struct RecordingDeviceControl {
    calls: std::sync::Mutex<Vec<DeviceControlCall>>,
    capability_results: std::sync::Mutex<std::collections::VecDeque<Result<(), i32>>>,
    driver_name_result: std::sync::Mutex<Option<Result<String, i32>>>,
    crtc_event_result: std::sync::Mutex<Option<Result<u64, i32>>>,
}

#[cfg(test)]
impl RecordingDeviceControl {
    pub(crate) fn new(
        capability_results: impl IntoIterator<Item = Result<(), i32>>,
        driver_name_result: Result<String, i32>,
        crtc_event_result: Result<u64, i32>,
    ) -> Self {
        Self {
            calls: std::sync::Mutex::new(Vec::new()),
            capability_results: std::sync::Mutex::new(capability_results.into_iter().collect()),
            driver_name_result: std::sync::Mutex::new(Some(driver_name_result)),
            crtc_event_result: std::sync::Mutex::new(Some(crtc_event_result)),
        }
    }

    pub(crate) fn calls(&self) -> Vec<DeviceControlCall> {
        self.calls.lock().expect("control call mutex").clone()
    }
}

#[cfg(test)]
impl DeviceControl for RecordingDeviceControl {
    fn set_client_capability(
        &self,
        _device: &Device,
        capability: DeviceClientCapability,
    ) -> io::Result<()> {
        self.calls
            .lock()
            .expect("control call mutex")
            .push(DeviceControlCall::SetClientCapability(capability));
        match self
            .capability_results
            .lock()
            .expect("capability result mutex")
            .pop_front()
        {
            Some(Ok(())) => Ok(()),
            Some(Err(errno)) => Err(io::Error::from_raw_os_error(errno)),
            None => Err(io::Error::from_raw_os_error(libc::EIO)),
        }
    }

    fn driver_name(&self, _device: &Device) -> io::Result<String> {
        self.calls
            .lock()
            .expect("control call mutex")
            .push(DeviceControlCall::DriverName);
        match self
            .driver_name_result
            .lock()
            .expect("driver result mutex")
            .take()
        {
            Some(Ok(name)) => Ok(name),
            Some(Err(errno)) => Err(io::Error::from_raw_os_error(errno)),
            None => Err(io::Error::from_raw_os_error(libc::EIO)),
        }
    }

    fn crtc_in_vblank_event(&self, _device: &Device) -> io::Result<u64> {
        self.calls
            .lock()
            .expect("control call mutex")
            .push(DeviceControlCall::CrtcInVBlankEvent);
        match self
            .crtc_event_result
            .lock()
            .expect("CRTC event result mutex")
            .take()
        {
            Some(Ok(value)) => Ok(value),
            Some(Err(errno)) => Err(io::Error::from_raw_os_error(errno)),
            None => Err(io::Error::from_raw_os_error(libc::EIO)),
        }
    }
}

#[cfg(test)]
pub(crate) fn recording_control(driver_name: &str) -> std::sync::Arc<RecordingDeviceControl> {
    std::sync::Arc::new(RecordingDeviceControl::new(
        [Ok(()), Ok(()), Ok(())],
        Ok(driver_name.to_owned()),
        Ok(1),
    ))
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io, sync::Arc};

    use crate::drm::{
        Device,
        client_caps::{
            CursorDeviceCaps, DeviceClientCapability, DeviceControlCall, RecordingDeviceControl,
            negotiate_cursor_caps,
        },
    };

    #[test]
    // Mutation: duplicate or reorder the cursor step, or keep an extra Atomic capability request.
    fn open_sequence_requests_universal_atomic_hotspot_name_then_checks_crtc_events_once() {
        let control = super::recording_control("amdgpu");
        let device = Device::for_tests_with_control(control.clone()).expect("configured device");

        assert_eq!(
            control.calls(),
            vec![
                DeviceControlCall::SetClientCapability(DeviceClientCapability::UniversalPlanes),
                DeviceControlCall::SetClientCapability(DeviceClientCapability::Atomic),
                DeviceControlCall::SetClientCapability(DeviceClientCapability::CursorPlaneHotspot),
                DeviceControlCall::DriverName,
                DeviceControlCall::CrtcInVBlankEvent,
            ]
        );
        assert_eq!(
            device.cursor_caps(),
            Some(&CursorDeviceCaps::new(true, Some("amdgpu".to_owned())))
        );

        let query_error = Arc::new(RecordingDeviceControl::new(
            [Ok(()), Ok(()), Ok(())],
            Ok("amdgpu".to_owned()),
            Err(libc::EIO),
        ));
        let error = Device::for_tests_with_control(query_error)
            .err()
            .expect("CRTC event query errors must reject KMS setup");
        assert!(error.to_string().contains("/dev/null"));
        assert!(error.to_string().contains("DRM_CAP_CRTC_IN_VBLANK_EVENT"));

        let zero_value = Arc::new(RecordingDeviceControl::new(
            [Ok(()), Ok(()), Ok(())],
            Ok("amdgpu".to_owned()),
            Ok(0),
        ));
        let error = Device::for_tests_with_control(zero_value)
            .err()
            .expect("zero CRTC event capability must reject KMS setup");
        assert!(error.to_string().contains("/dev/null"));
        assert!(error.to_string().contains("DRM_CAP_CRTC_IN_VBLANK_EVENT"));
    }

    #[test]
    // Mutation: query the driver name before requesting cursor hotspot support.
    fn negotiate_cursor_caps_orders_hotspot_then_driver_name() {
        let device = Device::for_tests().expect("stub device");
        let control = RecordingDeviceControl::new([Ok(())], Ok("virtio_gpu".to_owned()), Ok(1));

        let caps = negotiate_cursor_caps(&device, &control);

        assert_eq!(
            control.calls(),
            vec![
                DeviceControlCall::SetClientCapability(DeviceClientCapability::CursorPlaneHotspot),
                DeviceControlCall::DriverName,
            ]
        );
        assert!(caps.cursor_plane_hotspot_cap_accepted());
        assert_eq!(caps.driver_name(), Some("virtio_gpu"));
    }

    #[test]
    // Mutation: make hotspot rejection fatal or cache it as accepted.
    fn physical_driver_hotspot_rejection_is_non_fatal_and_cached_false() {
        let device = Device::for_tests().expect("stub device");
        let control =
            RecordingDeviceControl::new([Err(libc::EOPNOTSUPP)], Ok("amdgpu".to_owned()), Ok(1));

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            negotiate_cursor_caps(&device, &control)
        }));
        assert!(
            result.is_ok(),
            "hotspot rejection must not make negotiation fatal"
        );
        let caps = result.expect("checked non-fatal hotspot rejection");

        assert!(!caps.cursor_plane_hotspot_cap_accepted());
        assert!(!caps.requires_hotspot_support());
    }

    #[test]
    // Mutation: fail negotiation on a driver-name query error or treat the unknown name as physical.
    fn unknown_name_query_failure_requires_hotspot_support() {
        let device = Device::for_tests().expect("stub device");
        let control = RecordingDeviceControl::new([Ok(())], Err(libc::EIO), Ok(1));

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            negotiate_cursor_caps(&device, &control)
        }));
        assert!(
            result.is_ok(),
            "driver-name query failure must remain non-fatal"
        );
        let caps = result.expect("checked non-fatal driver-name failure");

        assert_eq!(caps.driver_name(), None);
        assert!(caps.requires_hotspot_support());
        assert!(!caps.is_paravirtualized());
    }

    #[test]
    // Mutation: use prefix/case-insensitive matching or classify every unknown name as paravirtualized.
    fn paravirtualized_names_are_exact_and_unknown_needs_hotspot() {
        for name in ["virtio_gpu", "vmwgfx", "qxl", "vboxvideo"] {
            let caps = CursorDeviceCaps::new(true, Some(name.to_owned()));
            assert!(
                caps.is_paravirtualized(),
                "{name} must be recognized exactly"
            );
            assert!(
                caps.requires_hotspot_support(),
                "{name} requires hotspot support"
            );
        }

        for name in [
            "msm",
            "xe",
            "i915",
            "amdgpu",
            "nouveau",
            "nvidia-drm",
            "apple",
            "Virtio_gpu",
            "virtio_gpu2",
            "qxl-drm",
        ] {
            let caps = CursorDeviceCaps::new(false, Some(name.to_owned()));
            assert!(!caps.is_paravirtualized(), "{name} is not an exact match");
            assert!(
                !caps.requires_hotspot_support(),
                "{name} is known physical/other"
            );
        }

        let unknown = CursorDeviceCaps::new(false, None);
        assert!(!unknown.is_paravirtualized());
        assert!(unknown.requires_hotspot_support());
    }

    #[test]
    // Mutation: negotiate cursor capabilities from a render-node or inherited-fd constructor.
    fn render_node_and_inherited_fd_devices_have_no_caps() -> io::Result<()> {
        let public_render = Device::open_render_node("/dev/null")?;
        assert!(public_render.cursor_caps().is_none());

        let render_control = Arc::new(RecordingDeviceControl::new(
            [Ok(())],
            Ok("amdgpu".to_owned()),
            Ok(1),
        ));
        let render = Device::open_render_node_with_control("/dev/null", render_control.clone())?;
        assert!(render.cursor_caps().is_none());
        assert!(render_control.calls().is_empty());

        let public_inherited =
            Device::from_inherited_kms_fd(File::open("/dev/null")?.into(), "/dev/null");
        assert!(public_inherited.cursor_caps().is_none());

        let inherited_control = Arc::new(RecordingDeviceControl::new(
            [Ok(())],
            Ok("amdgpu".to_owned()),
            Ok(1),
        ));
        let inherited = Device::from_inherited_kms_fd_with_control(
            File::open("/dev/null")?.into(),
            "/dev/null",
            inherited_control.clone(),
        );
        assert!(inherited.cursor_caps().is_none());
        assert!(inherited_control.calls().is_empty());
        Ok(())
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[test]
    // Mutation: change the target's _IOW direction or payload-size encoding.
    fn set_client_cap_ioctl_matches_target_abi() {
        #[cfg(all(
            target_os = "linux",
            not(any(
                target_arch = "mips",
                target_arch = "mips32r6",
                target_arch = "mips64",
                target_arch = "mips64r6",
                target_arch = "powerpc",
                target_arch = "powerpc64",
                target_arch = "sparc64"
            ))
        ))]
        let expected = 0x4010_640d_u64;

        #[cfg(any(
            target_os = "freebsd",
            all(
                target_os = "linux",
                any(
                    target_arch = "mips",
                    target_arch = "mips32r6",
                    target_arch = "mips64",
                    target_arch = "mips64r6",
                    target_arch = "powerpc",
                    target_arch = "powerpc64",
                    target_arch = "sparc64"
                )
            )
        ))]
        let expected = 0x8010_640d_u64;

        assert_eq!(
            format!("{:#x}", super::DRM_IOCTL_SET_CLIENT_CAP),
            format!("{expected:#x}"),
        );
    }
}
