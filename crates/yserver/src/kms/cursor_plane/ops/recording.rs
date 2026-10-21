use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    io,
    os::fd::{AsFd, AsRawFd},
    rc::Rc,
};

use super::{
    CursorBufferIo, CursorDumbBuffer, CursorFramebufferDescriptor, CursorMapping, ensure_device,
    write_mapped_image,
};
use crate::drm::Device;
use drm::control::framebuffer;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CursorBufferRecord {
    Create {
        device_fd: i32,
        handle: u32,
        width: u32,
        height: u32,
    },
    Map {
        device_fd: i32,
        handle: u32,
    },
    AddFb {
        device_fd: i32,
        handle: u32,
        fb: u32,
        width: u32,
        height: u32,
        format: u32,
    },
    RemoveFb {
        device_fd: i32,
        fb: u32,
    },
    Unmap {
        device_fd: i32,
        handle: u32,
    },
    Destroy {
        device_fd: i32,
        handle: u32,
    },
}

#[derive(Default)]
pub(crate) struct CursorBufferLedger {
    records: RefCell<Vec<CursorBufferRecord>>,
    live_dumb: RefCell<HashSet<(i32, u32)>>,
    live_mappings: RefCell<HashSet<(i32, u32)>>,
    live_framebuffers: RefCell<HashSet<(i32, u32)>>,
    storage: RefCell<CursorBufferStorageMap>,
    failures: RefCell<Vec<CursorBufferFailure>>,
    failures_taken: RefCell<Vec<CursorBufferOperation>>,
    write_count: std::cell::Cell<usize>,
}

type CursorBufferStorageMap = HashMap<(i32, u32), Rc<RefCell<Box<[u8]>>>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CursorBufferOperation {
    Create,
    Map,
    AddFb,
    Write,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CursorBufferFailure {
    operation: CursorBufferOperation,
    successful_calls_before_failure: usize,
    raw_os_error: i32,
}

impl CursorBufferLedger {
    pub(crate) fn records(&self) -> Vec<CursorBufferRecord> {
        self.records.borrow().clone()
    }

    pub(crate) fn end_state(&self) -> (usize, usize, usize) {
        (
            self.live_dumb.borrow().len(),
            self.live_mappings.borrow().len(),
            self.live_framebuffers.borrow().len(),
        )
    }

    pub(crate) fn failures_taken(&self) -> Vec<CursorBufferOperation> {
        self.failures_taken.borrow().clone()
    }

    pub(crate) fn write_count(&self) -> usize {
        self.write_count.get()
    }

    pub(crate) fn fail_after(
        &self,
        operation: CursorBufferOperation,
        successful_calls: usize,
        raw_os_error: i32,
    ) {
        self.failures.borrow_mut().push(CursorBufferFailure {
            operation,
            successful_calls_before_failure: successful_calls,
            raw_os_error,
        });
    }

    pub(crate) fn mapped_bytes(&self, device_fd: i32, handle: u32) -> Option<Vec<u8>> {
        self.storage
            .borrow()
            .get(&(device_fd, handle))
            .map(|storage| storage.borrow().to_vec())
    }

    fn take_failure(&self, operation: CursorBufferOperation) -> Option<i32> {
        let mut failures = self.failures.borrow_mut();
        let index = failures
            .iter()
            .position(|failure| failure.operation == operation)?;
        let failure = &mut failures[index];
        if failure.successful_calls_before_failure != 0 {
            failure.successful_calls_before_failure -= 1;
            return None;
        }
        let raw_os_error = failures.remove(index).raw_os_error;
        self.failures_taken.borrow_mut().push(operation);
        Some(raw_os_error)
    }
}

pub(crate) struct RecordingCursorBufferIo {
    ledger: Rc<CursorBufferLedger>,
    next_handle: RefCell<u32>,
    next_fb: RefCell<u32>,
    framebuffer_devices: RefCell<HashSet<(i32, u32)>>,
    pitch_alignment: u32,
}

impl RecordingCursorBufferIo {
    pub(crate) fn new(pitch_alignment: u32) -> (Self, Rc<CursorBufferLedger>) {
        let ledger = Rc::new(CursorBufferLedger::default());
        (
            Self {
                ledger: Rc::clone(&ledger),
                next_handle: RefCell::new(1),
                next_fb: RefCell::new(100),
                framebuffer_devices: RefCell::new(HashSet::new()),
                pitch_alignment: pitch_alignment.max(1),
            },
            ledger,
        )
    }

    #[cfg(test)]
    #[allow(dead_code, reason = "consumed by P2a Task 3 sprite pool")]
    pub(crate) fn with_shared_ledger(ledger: Rc<CursorBufferLedger>, pitch_alignment: u32) -> Self {
        Self {
            ledger,
            next_handle: RefCell::new(1),
            next_fb: RefCell::new(100),
            framebuffer_devices: RefCell::new(HashSet::new()),
            pitch_alignment: pitch_alignment.max(1),
        }
    }
}

impl CursorBufferIo for RecordingCursorBufferIo {
    fn create_dumb(
        &self,
        device: &Device,
        descriptor: CursorFramebufferDescriptor,
    ) -> io::Result<CursorDumbBuffer> {
        if let Some(raw_os_error) = self.ledger.take_failure(CursorBufferOperation::Create) {
            return Err(io::Error::from_raw_os_error(raw_os_error));
        }
        let fd = device.as_fd().as_raw_fd();
        let width = descriptor.width;
        let height = descriptor.height;
        let mut next = self.next_handle.borrow_mut();
        let handle = *next;
        *next = next.wrapping_add(1).max(1);
        let tight_pitch = width
            .checked_mul(4)
            .ok_or_else(|| io::Error::other("cursor pitch overflow"))?;
        let alignment = self.pitch_alignment;
        let pitch = tight_pitch
            .checked_add(alignment - 1)
            .map(|value| value / alignment * alignment)
            .ok_or_else(|| io::Error::other("cursor padded pitch overflow"))?;
        let len = usize::try_from(pitch)
            .ok()
            .and_then(|pitch| pitch.checked_mul(usize::try_from(height).ok()?))
            .ok_or_else(|| io::Error::other("cursor mapping size overflow"))?;
        self.ledger.live_dumb.borrow_mut().insert((fd, handle));
        self.ledger.storage.borrow_mut().insert(
            (fd, handle),
            Rc::new(RefCell::new(vec![0_u8; len].into_boxed_slice())),
        );
        self.ledger
            .records
            .borrow_mut()
            .push(CursorBufferRecord::Create {
                device_fd: fd,
                handle,
                width,
                height,
            });
        Ok(CursorDumbBuffer::recording(device, handle, pitch, len))
    }

    fn map_dumb(&self, device: &Device, dumb: &mut CursorDumbBuffer) -> io::Result<CursorMapping> {
        if let Some(raw_os_error) = self.ledger.take_failure(CursorBufferOperation::Map) {
            return Err(io::Error::from_raw_os_error(raw_os_error));
        }
        let fd = device.as_fd().as_raw_fd();
        ensure_device(device, dumb.device_fd)?;
        if !self.ledger.live_dumb.borrow().contains(&(fd, dumb.handle)) {
            return Err(io::Error::other("recording cursor dumb buffer is not live"));
        }
        self.ledger
            .live_mappings
            .borrow_mut()
            .insert((fd, dumb.handle));
        self.ledger
            .records
            .borrow_mut()
            .push(CursorBufferRecord::Map {
                device_fd: fd,
                handle: dumb.handle,
            });
        let storage = self
            .ledger
            .storage
            .borrow()
            .get(&(fd, dumb.handle))
            .cloned()
            .ok_or_else(|| io::Error::other("recording cursor backing is not live"))?;
        CursorMapping::recording(storage)
    }

    fn add_framebuffer(
        &self,
        device: &Device,
        dumb: &CursorDumbBuffer,
        descriptor: CursorFramebufferDescriptor,
    ) -> io::Result<framebuffer::Handle> {
        if let Some(raw_os_error) = self.ledger.take_failure(CursorBufferOperation::AddFb) {
            return Err(io::Error::from_raw_os_error(raw_os_error));
        }
        let fd = device.as_fd().as_raw_fd();
        ensure_device(device, dumb.device_fd)?;
        if !self
            .ledger
            .live_mappings
            .borrow()
            .contains(&(fd, dumb.handle))
        {
            return Err(io::Error::other("recording cursor buffer is not mapped"));
        }
        let mut next = self.next_fb.borrow_mut();
        let raw_fb = *next;
        *next = next.wrapping_add(1).max(100);
        let fb = ::drm::control::from_u32(raw_fb)
            .ok_or_else(|| io::Error::other("invalid recording framebuffer handle"))?;
        self.ledger
            .live_framebuffers
            .borrow_mut()
            .insert((fd, raw_fb));
        self.framebuffer_devices.borrow_mut().insert((fd, raw_fb));
        self.ledger
            .records
            .borrow_mut()
            .push(CursorBufferRecord::AddFb {
                device_fd: fd,
                handle: dumb.handle,
                fb: raw_fb,
                width: descriptor.width,
                height: descriptor.height,
                format: descriptor.format as u32,
            });
        Ok(fb)
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
        if let Some(raw_os_error) = self.ledger.take_failure(CursorBufferOperation::Write) {
            return Err(io::Error::from_raw_os_error(raw_os_error));
        }
        self.ledger
            .write_count
            .set(self.ledger.write_count.get().saturating_add(1));
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
        let fd = device.as_fd().as_raw_fd();
        let raw_fb = u32::from(fb);
        if !self.framebuffer_devices.borrow_mut().remove(&(fd, raw_fb))
            || !self
                .ledger
                .live_framebuffers
                .borrow_mut()
                .remove(&(fd, raw_fb))
        {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "unknown recording cursor FB",
            ));
        }
        self.ledger
            .records
            .borrow_mut()
            .push(CursorBufferRecord::RemoveFb {
                device_fd: fd,
                fb: raw_fb,
            });
        Ok(())
    }

    fn unmap_dumb(&self, device: &Device, dumb: &CursorDumbBuffer, _mapping: CursorMapping) {
        let fd = device.as_fd().as_raw_fd();
        if ensure_device(device, dumb.device_fd).is_err()
            || !self
                .ledger
                .live_mappings
                .borrow_mut()
                .remove(&(fd, dumb.handle))
        {
            log::error!("unknown recording cursor mapping during unmap");
            return;
        }
        self.ledger
            .records
            .borrow_mut()
            .push(CursorBufferRecord::Unmap {
                device_fd: fd,
                handle: dumb.handle,
            });
    }

    fn destroy_dumb(&self, device: &Device, dumb: CursorDumbBuffer) -> io::Result<()> {
        let fd = device.as_fd().as_raw_fd();
        ensure_device(device, dumb.device_fd)?;
        if !self
            .ledger
            .live_dumb
            .borrow_mut()
            .remove(&(fd, dumb.handle))
        {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "unknown recording cursor dumb buffer",
            ));
        }
        self.ledger.storage.borrow_mut().remove(&(fd, dumb.handle));
        self.ledger
            .records
            .borrow_mut()
            .push(CursorBufferRecord::Destroy {
                device_fd: fd,
                handle: dumb.handle,
            });
        Ok(())
    }
}
