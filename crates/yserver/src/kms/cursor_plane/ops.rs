//! Raw cursor-buffer operations and the shared dumb-buffer allocation path.

use std::{
    cell::RefCell,
    io,
    os::fd::{AsFd, AsRawFd},
    ptr::NonNull,
    rc::Rc,
};

use drm::{
    buffer::{Buffer, DrmFourcc, DrmModifier, Handle as DrmBufferHandle, PlanarBuffer},
    control::{Device as ControlDevice, FbCmd2Flags, dumbbuffer::DumbBuffer, framebuffer},
};

use crate::drm::Device;

pub(crate) struct CursorDumbBuffer {
    device_fd: i32,
    handle: u32,
    pitch: u32,
    #[allow(dead_code, reason = "kept for the recording buffer descriptor")]
    len: usize,
    dumb: Option<DumbBuffer>,
}

impl CursorDumbBuffer {
    #[allow(dead_code, reason = "used by the recording cursor buffer")]
    pub(crate) fn handle(&self) -> u32 {
        self.handle
    }

    pub(crate) fn pitch(&self) -> u32 {
        self.pitch
    }

    #[allow(dead_code, reason = "kept for the recording buffer descriptor")]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    #[cfg(test)]
    pub(crate) fn recording(device: &Device, handle: u32, pitch: u32, len: usize) -> Self {
        Self {
            device_fd: device.as_fd().as_raw_fd(),
            handle,
            pitch,
            len,
            dumb: None,
        }
    }
}

pub(crate) struct CursorMapping {
    ptr: NonNull<u8>,
    len: usize,
    owned: Option<Box<[u8]>>,
    _recording: Option<Rc<RefCell<Box<[u8]>>>>,
}

impl CursorMapping {
    pub(crate) fn ptr(&self) -> NonNull<u8> {
        self.ptr
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    #[cfg(test)]
    pub(crate) fn recording(backing: Rc<RefCell<Box<[u8]>>>) -> io::Result<Self> {
        let ptr = NonNull::new(backing.borrow_mut().as_mut_ptr())
            .ok_or_else(|| io::Error::other("empty cursor mapping"))?;
        let len = backing.borrow().len();
        Ok(Self {
            ptr,
            len,
            owned: None,
            _recording: Some(backing),
        })
    }
}

pub(crate) trait CursorBufferIo {
    fn create_dumb(
        &self,
        device: &Device,
        descriptor: CursorFramebufferDescriptor,
    ) -> io::Result<CursorDumbBuffer>;
    fn map_dumb(&self, device: &Device, dumb: &mut CursorDumbBuffer) -> io::Result<CursorMapping>;
    fn add_framebuffer(
        &self,
        device: &Device,
        dumb: &CursorDumbBuffer,
        descriptor: CursorFramebufferDescriptor,
    ) -> io::Result<framebuffer::Handle>;
    fn write_image(
        &self,
        mapping: &CursorMapping,
        stride: u32,
        capacity_width: u32,
        capacity_height: u32,
        image_width: u32,
        image_height: u32,
        bgra_bytes: &[u8],
    ) -> io::Result<()>;
    fn remove_framebuffer(&self, device: &Device, fb: framebuffer::Handle) -> io::Result<()>;
    fn unmap_dumb(&self, device: &Device, dumb: &CursorDumbBuffer, mapping: CursorMapping);
    fn destroy_dumb(&self, device: &Device, dumb: CursorDumbBuffer) -> io::Result<()>;
}

#[derive(Clone, Copy)]
pub(crate) struct CursorFramebufferDescriptor {
    width: u32,
    height: u32,
    format: DrmFourcc,
}

impl CursorFramebufferDescriptor {
    fn argb8888(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            format: DrmFourcc::Argb8888,
        }
    }
}

#[derive(Default)]
pub(crate) struct DrmCursorBufferIo;

impl CursorBufferIo for DrmCursorBufferIo {
    fn create_dumb(
        &self,
        device: &Device,
        descriptor: CursorFramebufferDescriptor,
    ) -> io::Result<CursorDumbBuffer> {
        let dumb = device.create_dumb_buffer(
            (descriptor.width, descriptor.height),
            descriptor.format,
            32,
        )?;
        let handle = dumb.handle().into();
        let pitch = dumb.pitch();
        let len = usize::try_from(dumb.size().0)
            .ok()
            .and_then(|row| row.checked_mul(usize::try_from(dumb.size().1).ok()?))
            .and_then(|_| {
                usize::try_from(dumb.pitch())
                    .ok()?
                    .checked_mul(usize::try_from(dumb.size().1).ok()?)
            })
            .ok_or_else(|| io::Error::other("cursor dumb buffer size overflow"))?;
        Ok(CursorDumbBuffer {
            device_fd: device.as_fd().as_raw_fd(),
            handle,
            pitch,
            len,
            dumb: Some(dumb),
        })
    }

    fn map_dumb(&self, device: &Device, dumb: &mut CursorDumbBuffer) -> io::Result<CursorMapping> {
        ensure_device(device, dumb.device_fd)?;
        let raw_dumb = dumb
            .dumb
            .as_mut()
            .ok_or_else(|| io::Error::other("recording cursor token used by DRM mapper"))?;
        let mut mapping = device.map_dumb_buffer(raw_dumb)?;
        let len = mapping.len();
        let ptr = NonNull::new(mapping.as_mut_ptr())
            .ok_or_else(|| io::Error::other("non-null dumb mapping expected"))?;
        std::mem::forget(mapping);
        Ok(CursorMapping {
            ptr,
            len,
            owned: None,
            _recording: None,
        })
    }

    fn add_framebuffer(
        &self,
        device: &Device,
        dumb: &CursorDumbBuffer,
        descriptor: CursorFramebufferDescriptor,
    ) -> io::Result<framebuffer::Handle> {
        ensure_device(device, dumb.device_fd)?;
        let raw_dumb = dumb
            .dumb
            .as_ref()
            .ok_or_else(|| io::Error::other("recording cursor token used by DRM AddFB2"))?;
        device.add_planar_framebuffer(
            &CursorPlanarBuffer {
                dumb: *raw_dumb,
                descriptor,
            },
            FbCmd2Flags::empty(),
        )
    }

    fn write_image(
        &self,
        mapping: &CursorMapping,
        stride: u32,
        capacity_width: u32,
        capacity_height: u32,
        image_width: u32,
        image_height: u32,
        bgra_bytes: &[u8],
    ) -> io::Result<()> {
        write_mapped_image(
            mapping,
            stride,
            capacity_width,
            capacity_height,
            image_width,
            image_height,
            bgra_bytes,
        )
    }

    fn remove_framebuffer(&self, device: &Device, fb: framebuffer::Handle) -> io::Result<()> {
        device.destroy_framebuffer(fb)
    }

    fn unmap_dumb(&self, device: &Device, dumb: &CursorDumbBuffer, mapping: CursorMapping) {
        if ensure_device(device, dumb.device_fd).is_err() {
            log::error!("cursor buffer mapping belonged to a different DRM device");
            return;
        }
        if mapping.owned.is_none()
            // SAFETY: this mapping was created by drm-rs for `dumb` and its
            // ownership was transferred out of DumbMapping in `map_dumb`.
            && unsafe { libc::munmap(mapping.ptr.as_ptr().cast(), mapping.len) } != 0
        {
            log::warn!(
                "cursor dumb-buffer munmap failed: {}",
                io::Error::last_os_error()
            );
        }
    }

    fn destroy_dumb(&self, device: &Device, dumb: CursorDumbBuffer) -> io::Result<()> {
        ensure_device(device, dumb.device_fd)?;
        let raw_dumb = dumb
            .dumb
            .ok_or_else(|| io::Error::other("recording cursor token used by DRM destroy"))?;
        device.destroy_dumb_buffer(raw_dumb)
    }
}

fn ensure_device(device: &Device, expected_fd: i32) -> io::Result<()> {
    if device.as_fd().as_raw_fd() == expected_fd {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cursor resource used with a different DRM device",
        ))
    }
}

#[allow(dead_code, reason = "consumed by P2a Task 3 sprite pool")]
struct CursorPlanarBuffer {
    dumb: DumbBuffer,
    descriptor: CursorFramebufferDescriptor,
}

impl PlanarBuffer for CursorPlanarBuffer {
    fn size(&self) -> (u32, u32) {
        (self.descriptor.width, self.descriptor.height)
    }

    fn format(&self) -> DrmFourcc {
        self.descriptor.format
    }

    fn modifier(&self) -> Option<DrmModifier> {
        None
    }

    fn pitches(&self) -> [u32; 4] {
        [self.dumb.pitch(), 0, 0, 0]
    }

    fn handles(&self) -> [Option<DrmBufferHandle>; 4] {
        [Some(self.dumb.handle()), None, None, None]
    }

    fn offsets(&self) -> [u32; 4] {
        [0; 4]
    }
}

pub(crate) struct CursorBuffer {
    device: Rc<Device>,
    io: Rc<dyn CursorBufferIo>,
    dumb: Option<CursorDumbBuffer>,
    mapping: Option<CursorMapping>,
    fb: Option<framebuffer::Handle>,
    stride: u32,
    len: usize,
    width: u32,
    height: u32,
}

impl CursorBuffer {
    /// Shared production allocation algorithm. Rollback releases only
    /// successfully acquired resources, in reverse acquisition order.
    pub(crate) fn allocate(
        device: Rc<Device>,
        io: Rc<dyn CursorBufferIo>,
        width: u32,
        height: u32,
    ) -> io::Result<Self> {
        let descriptor = CursorFramebufferDescriptor::argb8888(width, height);
        let mut dumb = io.create_dumb(&device, descriptor)?;
        let stride = dumb.pitch();
        let mapping = match io.map_dumb(&device, &mut dumb) {
            Ok(mapping) => mapping,
            Err(error) => {
                let handle = dumb.handle;
                let cleanup = io.destroy_dumb(&device, dumb);
                return Err(allocation_cleanup_error(
                    error,
                    format!("destroy cursor dumb buffer {handle:#x}"),
                    cleanup,
                ));
            }
        };
        let fb = match io.add_framebuffer(&device, &dumb, descriptor) {
            Ok(fb) => fb,
            Err(error) => {
                io.unmap_dumb(&device, &dumb, mapping);
                let handle = dumb.handle;
                let cleanup = io.destroy_dumb(&device, dumb);
                return Err(allocation_cleanup_error(
                    error,
                    format!("destroy cursor dumb buffer {handle:#x}"),
                    cleanup,
                ));
            }
        };
        let len = mapping.len();
        let mut buffer = Self {
            device,
            io,
            dumb: Some(dumb),
            mapping: Some(mapping),
            fb: Some(fb),
            stride,
            len,
            width,
            height,
        };
        // The newly allocated ARGB image starts transparent.
        buffer.clear();
        Ok(buffer)
    }

    pub(crate) fn release(mut self) -> io::Result<()> {
        self.release_resources()
    }

    fn release_resources(&mut self) -> io::Result<()> {
        let mut cleanup_error = None;
        if let Some(fb) = self.fb.take()
            && let Err(error) = self.io.remove_framebuffer(&self.device, fb)
        {
            cleanup_error = Some(error);
        }
        if let (Some(dumb), Some(mapping)) = (self.dumb.as_ref(), self.mapping.take()) {
            self.io.unmap_dumb(&self.device, dumb, mapping);
        }
        if let Some(dumb) = self.dumb.take()
            && let Err(error) = self.io.destroy_dumb(&self.device, dumb)
        {
            if let Some(previous) = cleanup_error {
                cleanup_error = Some(io::Error::other(format!(
                    "{previous}; destroy cursor dumb buffer failed: {error}"
                )));
            } else {
                cleanup_error = Some(error);
            }
        }
        cleanup_error.map_or(Ok(()), Err)
    }

    pub(crate) fn fb_id(&self) -> framebuffer::Handle {
        self.fb.expect("cursor framebuffer is live")
    }

    pub(crate) fn ptr(&self) -> NonNull<u8> {
        self.mapping.as_ref().expect("cursor mapping is live").ptr()
    }

    pub(crate) fn write_image(
        &mut self,
        image_w: u32,
        image_h: u32,
        bgra_bytes: &[u8],
    ) -> io::Result<()> {
        if image_w == 0 || image_h == 0 || image_w > self.width || image_h > self.height {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cursor exceeds sprite framebuffer dimensions",
            ));
        }
        let row_len = usize::try_from(image_w)
            .ok()
            .and_then(|width| width.checked_mul(4))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "cursor width overflow"))?;
        let expected = row_len
            .checked_mul(usize::try_from(image_h).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "cursor height overflow")
            })?)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "cursor byte size overflow")
            })?;
        if bgra_bytes.len() != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cursor bytes must equal width*height*4",
            ));
        }
        let _ = expected;
        self.io.write_image(
            self.mapping
                .as_ref()
                .expect("cursor mapping is live during upload"),
            self.stride,
            self.width,
            self.height,
            image_w,
            image_h,
            bgra_bytes,
        )
    }

    pub(crate) fn clear(&mut self) {
        // SAFETY: mapping remains live and covers `len` writable bytes.
        unsafe { std::ptr::write_bytes(self.ptr().as_ptr(), 0, self.len) };
    }

    #[cfg(test)]
    pub(crate) fn for_tests_bytes(&self) -> Vec<u8> {
        // SAFETY: the live mapping spans `len` readable bytes.
        unsafe { std::slice::from_raw_parts(self.ptr().as_ptr(), self.len) }.to_vec()
    }
}

fn write_mapped_image(
    mapping: &CursorMapping,
    stride: u32,
    capacity_width: u32,
    capacity_height: u32,
    image_width: u32,
    image_height: u32,
    bgra_bytes: &[u8],
) -> io::Result<()> {
    if image_width == 0
        || image_height == 0
        || image_width > capacity_width
        || image_height > capacity_height
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cursor image dimensions must be nonzero and fit the mapped framebuffer geometry",
        ));
    }

    let pitch = usize::try_from(stride)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "cursor pitch overflow"))?;
    let row_len = usize::try_from(image_width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "cursor row overflow"))?;
    let capacity_row_len = usize::try_from(capacity_width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "cursor width overflow"))?;
    let image_len = row_len
        .checked_mul(
            usize::try_from(image_height).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "cursor height overflow")
            })?,
        )
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "cursor byte size overflow"))?;
    if bgra_bytes.len() != image_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cursor bytes must equal width*height*4",
        ));
    }
    if row_len > pitch || capacity_row_len > pitch {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cursor image does not fit the mapped framebuffer geometry",
        ));
    }
    let len = pitch
        .checked_mul(
            usize::try_from(capacity_height).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "cursor height overflow")
            })?,
        )
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "cursor mapping overflow"))?;
    if len > mapping.len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cursor mapping is smaller than its framebuffer geometry",
        ));
    }
    // SAFETY: the allocation mapping covers its padded pitch and height.
    unsafe { std::ptr::write_bytes(mapping.ptr.as_ptr(), 0, len) };
    for row in 0..usize::try_from(image_height)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "cursor height overflow"))?
    {
        // SAFETY: image validation checked the source size and each
        // destination row fits within the padded framebuffer pitch.
        unsafe {
            std::ptr::copy_nonoverlapping(
                bgra_bytes.as_ptr().add(row * row_len),
                mapping.ptr.as_ptr().add(row * pitch),
                row_len,
            );
        }
    }
    Ok(())
}

fn allocation_cleanup_error(
    operation_error: io::Error,
    cleanup_context: String,
    cleanup_result: io::Result<()>,
) -> io::Error {
    match cleanup_result {
        Ok(()) => operation_error,
        Err(cleanup_error) => io::Error::other(format!(
            "{operation_error}; rollback {cleanup_context} failed: {cleanup_error}"
        )),
    }
}

impl Drop for CursorBuffer {
    fn drop(&mut self) {
        if let Err(error) = self.release_resources() {
            log::warn!("cursor buffer cleanup failed: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kms::cursor_plane::ops::recording::{
        CursorBufferOperation, CursorBufferRecord, RecordingCursorBufferIo,
    };

    fn test_device() -> Rc<Device> {
        Rc::new(Device::for_tests().expect("test DRM device"))
    }

    // Mutation: shrink the allocated geometry or add the framebuffer before mapping; the exact
    // geometry and acquisition order assertions must fail.
    #[test]
    fn cursor_buffer_allocation_order_and_full_geometry() {
        let device = test_device();
        let fd = device.as_fd().as_raw_fd();
        let (buffer_io, ledger) = RecordingCursorBufferIo::new(256);
        let allocation = CursorBuffer::allocate(device, Rc::new(buffer_io), 70, 64);
        assert!(
            allocation.is_ok(),
            "cursor buffer allocation should succeed"
        );
        let buffer = allocation.expect("checked cursor buffer allocation");

        assert_eq!(
            ledger.records(),
            vec![
                CursorBufferRecord::Create {
                    device_fd: fd,
                    handle: 1,
                    width: 70,
                    height: 64,
                },
                CursorBufferRecord::Map {
                    device_fd: fd,
                    handle: 1,
                },
                CursorBufferRecord::AddFb {
                    device_fd: fd,
                    handle: 1,
                    fb: 100,
                    width: 70,
                    height: 64,
                    format: DrmFourcc::Argb8888 as u32,
                },
            ]
        );
        assert_eq!(ledger.end_state(), (1, 1, 1));

        buffer.release().expect("cursor buffer release");
        assert_eq!(
            &ledger.records()[3..],
            &[
                CursorBufferRecord::RemoveFb {
                    device_fd: fd,
                    fb: 100,
                },
                CursorBufferRecord::Unmap {
                    device_fd: fd,
                    handle: 1,
                },
                CursorBufferRecord::Destroy {
                    device_fd: fd,
                    handle: 1,
                },
            ]
        );
        assert_eq!(ledger.end_state(), (0, 0, 0));
    }

    // Mutation: omit rollback unmap after AddFB failure or dumb destruction after map failure;
    // the recorded reverse-order cleanup and empty ledger assertions must fail.
    #[test]
    fn cursor_buffer_failure_rolls_back_only_what_was_acquired_in_reverse_order() {
        for operation in [
            CursorBufferOperation::Create,
            CursorBufferOperation::Map,
            CursorBufferOperation::AddFb,
        ] {
            let device = test_device();
            let fd = device.as_fd().as_raw_fd();
            let (buffer_io, ledger) = RecordingCursorBufferIo::new(256);
            ledger.fail_after(operation, 0, libc::EIO);

            let error = match CursorBuffer::allocate(device, Rc::new(buffer_io), 70, 64) {
                Ok(_) => panic!("injected {operation:?} failure must reject allocation"),
                Err(error) => error,
            };
            assert_eq!(error.raw_os_error(), Some(libc::EIO));
            assert!(error.to_string().contains(&libc::EIO.to_string()));

            let expected = match operation {
                CursorBufferOperation::Create => vec![],
                CursorBufferOperation::Map => vec![
                    CursorBufferRecord::Create {
                        device_fd: fd,
                        handle: 1,
                        width: 70,
                        height: 64,
                    },
                    CursorBufferRecord::Destroy {
                        device_fd: fd,
                        handle: 1,
                    },
                ],
                CursorBufferOperation::AddFb => vec![
                    CursorBufferRecord::Create {
                        device_fd: fd,
                        handle: 1,
                        width: 70,
                        height: 64,
                    },
                    CursorBufferRecord::Map {
                        device_fd: fd,
                        handle: 1,
                    },
                    CursorBufferRecord::Unmap {
                        device_fd: fd,
                        handle: 1,
                    },
                    CursorBufferRecord::Destroy {
                        device_fd: fd,
                        handle: 1,
                    },
                ],
                CursorBufferOperation::Write => unreachable!("not an allocation failure"),
            };
            assert_eq!(ledger.records(), expected, "rollback after {operation:?}");
            assert_eq!(ledger.end_state(), (0, 0, 0));
        }
    }

    // Mutation: skip the full mapping clear or copy rows at image-width stride; the byte-for-byte
    // comparison must find stale pixels, padding, or misplaced rows.
    #[test]
    fn cursor_buffer_write_places_top_left_and_clears_padding() {
        let device = test_device();
        let (buffer_io, ledger) = RecordingCursorBufferIo::new(256);
        let allocation = CursorBuffer::allocate(device, Rc::new(buffer_io), 70, 64);
        assert!(
            allocation.is_ok(),
            "cursor buffer allocation should succeed"
        );
        let mut buffer = allocation.expect("checked cursor buffer allocation");
        assert_eq!(buffer.stride, 512);

        let first_pixel = [0x11, 0x22, 0x33, 0x44];
        let second_pixel = [0xaa, 0xbb, 0xcc, 0xdd];
        buffer
            .write_image(4, 4, &first_pixel.repeat(16))
            .expect("first cursor write");
        buffer
            .write_image(2, 2, &second_pixel.repeat(4))
            .expect("second cursor write");

        let mut expected = vec![0_u8; 512 * 64];
        for row in 0..2 {
            for column in 0..2 {
                let offset = row * 512 + column * 4;
                expected[offset..offset + 4].copy_from_slice(&second_pixel);
            }
        }
        assert_eq!(buffer.for_tests_bytes(), expected);

        buffer.release().expect("cursor buffer release");
        assert_eq!(ledger.end_state(), (0, 0, 0));
    }

    // Mutations: remove the capacity check, accept a short byte slice, or clear before validation;
    // every invalid request must remain InvalidInput and leave the seeded mapping unchanged.
    #[test]
    fn cursor_buffer_rejects_oversize_and_wrong_length() {
        let device = test_device();
        let (buffer_io, ledger) = RecordingCursorBufferIo::new(256);
        let allocation = CursorBuffer::allocate(device, Rc::new(buffer_io), 70, 64);
        assert!(
            allocation.is_ok(),
            "cursor buffer allocation should succeed"
        );
        let mut buffer = allocation.expect("checked cursor buffer allocation");
        let initial_image = [0x5a; 2 * 2 * 4];
        buffer
            .write_image(2, 2, &initial_image)
            .expect("seed mapping before invalid requests");
        let original = buffer.for_tests_bytes();

        let too_wide = vec![1; 71 * 64 * 4];
        let too_tall = vec![2; 70 * 65 * 4];
        let zero_width = [];
        let short_storage = [3; 2 * 2 * 4];
        for (width, height, bytes) in [
            (71, 64, too_wide.as_slice()),
            (70, 65, too_tall.as_slice()),
            (0, 4, zero_width.as_slice()),
            (2, 2, &short_storage[..short_storage.len() - 1]),
        ] {
            let result = buffer.write_image(width, height, bytes);
            assert!(
                result.is_err(),
                "invalid cursor image {width}x{height} must be rejected"
            );
            let error = result.expect_err("checked invalid cursor image");
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(buffer.for_tests_bytes(), original);
        }

        buffer.release().expect("cursor buffer release");
        assert_eq!(ledger.end_state(), (0, 0, 0));
    }

    // Mutations: allow overlong input by weakening the exact-size check, or remove the image-width
    // capacity guard; the raw I/O boundary must reject both before changing the mapping.
    #[test]
    fn cursor_buffer_io_write_rejects_invalid_images_before_mutation() {
        let device = test_device();
        let fd = device.as_fd().as_raw_fd();
        let (buffer_io, ledger) = RecordingCursorBufferIo::new(256);
        let mut dumb = buffer_io
            .create_dumb(&device, CursorFramebufferDescriptor::argb8888(4, 4))
            .expect("recording cursor dumb buffer");
        let mapping = buffer_io
            .map_dumb(&device, &mut dumb)
            .expect("recording cursor mapping");

        let initial_image = [0x6b; 2 * 2 * 4];
        buffer_io
            .write_image(&mapping, dumb.pitch(), 4, 4, 2, 2, &initial_image)
            .expect("seed raw mapping before invalid requests");
        let original = ledger
            .mapped_bytes(fd, dumb.handle())
            .expect("recorded mapped bytes");

        let overlong = [0x7c; 2 * 2 * 4 + 1];
        let too_wide = [0x8d; 5 * 2 * 4];
        let too_tall = [0x9e; 2 * 5 * 4];
        let short = [0xaf; 2 * 2 * 4 - 1];
        for (width, height, bytes) in [
            (2, 2, overlong.as_slice()),
            (5, 2, too_wide.as_slice()),
            (2, 5, too_tall.as_slice()),
            (0, 2, &[]),
            (2, 0, &[]),
            (2, 2, short.as_slice()),
        ] {
            let result = buffer_io.write_image(&mapping, dumb.pitch(), 4, 4, width, height, bytes);
            assert!(
                result.is_err(),
                "raw invalid cursor image {width}x{height} must be rejected"
            );
            let error = result.expect_err("checked raw invalid cursor image");
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(
                ledger.mapped_bytes(fd, dumb.handle()),
                Some(original.clone()),
                "raw invalid cursor image {width}x{height} must not mutate the mapping"
            );
        }

        buffer_io.unmap_dumb(&device, &dumb, mapping);
        buffer_io
            .destroy_dumb(&device, dumb)
            .expect("destroy raw cursor dumb buffer");
        assert_eq!(ledger.end_state(), (0, 0, 0));
    }

    // Mutation: discard destroy_dumb's rollback error; the primary map error and cleanup context
    // must both remain observable.
    #[test]
    fn cursor_allocation_preserves_map_and_destroy_errors() {
        struct MapAndDestroyFailure;

        impl CursorBufferIo for MapAndDestroyFailure {
            fn create_dumb(
                &self,
                device: &Device,
                _descriptor: CursorFramebufferDescriptor,
            ) -> io::Result<CursorDumbBuffer> {
                Ok(CursorDumbBuffer::recording(device, 0x51, 256, 4096))
            }

            fn map_dumb(
                &self,
                _device: &Device,
                _dumb: &mut CursorDumbBuffer,
            ) -> io::Result<CursorMapping> {
                Err(io::Error::other("map failed"))
            }

            fn add_framebuffer(
                &self,
                _device: &Device,
                _dumb: &CursorDumbBuffer,
                _descriptor: CursorFramebufferDescriptor,
            ) -> io::Result<framebuffer::Handle> {
                unreachable!("framebuffer creation follows a successful map")
            }

            fn write_image(
                &self,
                _mapping: &CursorMapping,
                _stride: u32,
                _capacity_width: u32,
                _capacity_height: u32,
                _image_width: u32,
                _image_height: u32,
                _bgra_bytes: &[u8],
            ) -> io::Result<()> {
                unreachable!("image write follows successful allocation")
            }

            fn remove_framebuffer(
                &self,
                _device: &Device,
                _fb: framebuffer::Handle,
            ) -> io::Result<()> {
                unreachable!("allocation did not reach framebuffer creation")
            }

            fn unmap_dumb(
                &self,
                _device: &Device,
                _dumb: &CursorDumbBuffer,
                _mapping: CursorMapping,
            ) {
                unreachable!("allocation did not create a mapping")
            }

            fn destroy_dumb(&self, _device: &Device, _dumb: CursorDumbBuffer) -> io::Result<()> {
                Err(io::Error::other("destroy failed"))
            }
        }

        let device = test_device();
        let error = match CursorBuffer::allocate(device, Rc::new(MapAndDestroyFailure), 32, 32) {
            Ok(_) => panic!("map failure must reject cursor allocation"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), io::ErrorKind::Other);
        let message = error.to_string();
        assert!(message.contains("map failed"), "{message}");
        assert!(
            message.contains("destroy cursor dumb buffer 0x51"),
            "{message}"
        );
        assert!(message.contains("destroy failed"), "{message}");
    }
}

#[cfg(test)]
pub(crate) mod recording;
