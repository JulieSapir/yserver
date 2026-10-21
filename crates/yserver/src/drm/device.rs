use std::{
    error::Error,
    fmt,
    fs::{File, OpenOptions},
    io,
    os::unix::io::{AsFd, BorrowedFd, OwnedFd},
    sync::Arc,
};

use super::client_caps::{
    CursorDeviceCaps, DeviceClientCapability, DeviceControl, DrmDeviceControl,
    negotiate_cursor_caps,
};
use drm::Device as DrmDevice;

pub struct Device {
    file: File,
    path: String,
    master_ownership: MasterOwnership,
    device_control: Arc<dyn DeviceControl + Send + Sync>,
    cursor_caps: Option<CursorDeviceCaps>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MasterOwnership {
    None,
    AcquiredHere,
    InheritedDuplicate,
}

impl AsFd for Device {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.file.as_fd()
    }
}

impl DrmDevice for Device {}
impl drm::control::Device for Device {}

impl Device {
    /// Construct a stub `Device` backed by `/dev/null` for tests.
    ///
    /// The returned device is unable to issue real ioctls; callers
    /// that exercise actual DRM control paths must use `open`.
    /// Hidden from rustdoc — for use by test fixtures only.
    #[doc(hidden)]
    pub fn for_tests() -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/null")?;
        Ok(Self {
            file,
            path: "/dev/null".to_string(),
            master_ownership: MasterOwnership::None,
            device_control: Arc::new(DrmDeviceControl),
            cursor_caps: None,
        })
    }

    /// Construct a `/dev/null` device and run the post-open KMS setup with an injected control.
    #[doc(hidden)]
    #[cfg(test)]
    pub(crate) fn for_tests_with_control(
        control: Arc<dyn DeviceControl + Send + Sync>,
    ) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/null")?;
        let mut device = Self {
            file,
            path: "/dev/null".to_string(),
            master_ownership: MasterOwnership::None,
            device_control: control.clone(),
            cursor_caps: None,
        };
        device.configure_kms_device(control.as_ref())?;
        Ok(device)
    }

    /// Open a render node without taking DRM master.
    ///
    /// `open` is the KMS-master constructor: it calls `acquire_master_lock`
    /// and configures KMS capabilities, both of which a render node rejects
    /// (`DRM_IOCTL_SET_MASTER` → `EACCES` from a render client). Render nodes
    /// still serve the `DRM_RENDER_ALLOW` ioctls, which is everything the
    /// syncobj paths need.
    pub fn open_render_node(path: &str) -> io::Result<Self> {
        Self::open_render_node_with_control(path, Arc::new(DrmDeviceControl))
    }

    pub(crate) fn open_render_node_with_control(
        path: &str,
        control: Arc<dyn DeviceControl + Send + Sync>,
    ) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|err| open_error(path, &err))?;
        Ok(Self {
            file,
            path: path.to_string(),
            master_ownership: MasterOwnership::None,
            device_control: control,
            cursor_caps: None,
        })
    }

    /// Wrap a duplicated KMS fd inherited by a helper process.
    ///
    /// The fd must refer to the same DRM file description as a parent-owned
    /// KMS device. This `Device` owns and closes its duplicate, but deliberately
    /// never issues `DRM_IOCTL_DROP_MASTER`: master ownership remains with the
    /// parent, and closing one duplicate does not release it while the parent's
    /// reference remains open. Per-file client capabilities are inherited with
    /// the duplicated file description, and the parent completed the
    /// `DRM_CAP_CRTC_IN_VBLANK_EVENT` setup check. This constructor therefore
    /// does not repeat capability negotiation or the CRTC-event query.
    #[doc(hidden)]
    pub fn from_inherited_kms_fd(fd: OwnedFd, path: impl Into<String>) -> Self {
        Self::from_inherited_kms_fd_with_control(fd, path, Arc::new(DrmDeviceControl))
    }

    pub(crate) fn from_inherited_kms_fd_with_control(
        fd: OwnedFd,
        path: impl Into<String>,
        control: Arc<dyn DeviceControl + Send + Sync>,
    ) -> Self {
        Self {
            file: File::from(fd),
            path: path.into(),
            master_ownership: MasterOwnership::InheritedDuplicate,
            device_control: control,
            cursor_caps: None,
        }
    }

    pub fn open(path: &str) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|err| open_error(path, &err))?;
        let mut device = Self {
            file,
            path: path.to_string(),
            master_ownership: MasterOwnership::AcquiredHere,
            device_control: Arc::new(DrmDeviceControl),
            cursor_caps: None,
        };
        device.acquire_master_lock().map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("failed to acquire DRM master on {path}: {err}"),
            )
        })?;
        let control = Arc::clone(&device.device_control);
        device.configure_kms_device(control.as_ref())?;
        Ok(device)
    }

    fn configure_kms_device(&mut self, control: &dyn DeviceControl) -> io::Result<()> {
        // Both UniversalPlanes and Atomic are *opt-ins to fd visibility*
        // — drivers that have inherently-universal-only planes (e.g.
        // Asahi's apple_drm) reject the cap-set with EOPNOTSUPP even
        // though atomic state is still honoured at the ioctl level.
        // Warn but continue on either; an actually-non-atomic driver
        // will fail downstream at the first atomic_commit, which is
        // where we'd want the diagnostic anyway.
        if let Err(err) =
            control.set_client_capability(self, DeviceClientCapability::UniversalPlanes)
        {
            log::warn!(
                "DRM_CLIENT_CAP_UNIVERSAL_PLANES rejected ({err}); driver is presumably \
                 universal-only — continuing"
            );
        }
        if let Err(err) = control.set_client_capability(self, DeviceClientCapability::Atomic) {
            log::warn!(
                "DRM_CLIENT_CAP_ATOMIC rejected ({err}); the driver may still honour \
                 atomic_commit ioctls without the explicit opt-in (Asahi apple_drm) — \
                 continuing. If subsequent atomic_commit calls fail, the driver is \
                 genuinely non-atomic and yserver/KMS won't work on this kernel."
            );
        }
        self.cursor_caps = Some(negotiate_cursor_caps(self, control));
        require_crtc_in_vblank_event(&self.path, control.crtc_in_vblank_event(self))
    }

    /// Cursor capability and driver identity negotiated for primary KMS opens.
    pub fn cursor_caps(&self) -> Option<&CursorDeviceCaps> {
        self.cursor_caps.as_ref()
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}

#[derive(Debug)]
struct CrtcInVblankCapabilityError {
    path: String,
    source: io::Error,
}

impl fmt::Display for CrtcInVblankCapabilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "DRM device {} failed DRM_CAP_CRTC_IN_VBLANK_EVENT query: {}",
            self.path, self.source
        )
    }
}

impl Error for CrtcInVblankCapabilityError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

pub(crate) fn require_crtc_in_vblank_event(path: &str, result: io::Result<u64>) -> io::Result<()> {
    match result {
        Ok(1) => Ok(()),
        Ok(value) => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("DRM device {path} reports DRM_CAP_CRTC_IN_VBLANK_EVENT={value}, expected 1"),
        )),
        Err(source) => {
            let kind = source.kind();
            Err(io::Error::new(
                kind,
                CrtcInVblankCapabilityError {
                    path: path.to_owned(),
                    source,
                },
            ))
        }
    }
}

#[cfg(test)]
mod tests;

impl Drop for Device {
    fn drop(&mut self) {
        if self.master_ownership != MasterOwnership::AcquiredHere {
            return;
        }
        if let Err(err) = self.release_master_lock() {
            log::warn!(
                "failed to release DRM master on {}: {err} (file close will still drop the fd)",
                self.path
            );
        }
    }
}

fn open_error(path: &str, err: &io::Error) -> io::Error {
    use io::ErrorKind;
    let msg = match err.kind() {
        ErrorKind::NotFound => format!(
            "DRM device {path} not found. In vng: pass --graphics or \
             --qemu-opts=\"-device virtio-gpu-pci\". On bare metal: check `ls /dev/dri/` — \
             the GPU may be at card1 instead. Override with \
             `YSERVER_DRM_DEVICE=/dev/dri/cardN`."
        ),
        ErrorKind::PermissionDenied => format!(
            "opening {path} requires root — vng runs as root by default; on host use sudo \
             (but B is vng-only by design)"
        ),
        _ if err.raw_os_error() == Some(libc::EBUSY) => format!(
            "another DRM master holds {path} — B is vng-only; do not run yserver on a host \
             with an active graphical session"
        ),
        _ => format!("failed to open {path}: {err}"),
    };
    io::Error::new(err.kind(), msg)
}
