//! Full-capacity cursor sprite framebuffers retained for accepted atomic
//! cursor bindings.

use std::{io, rc::Rc};

use super::ops::{CursorBuffer, CursorBufferIo};
use crate::drm::Device;

/// A staged image/hotspot identity for the atomic binding phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SpriteBufferLease {
    slot: usize,
    version: u64,
    width: u32,
    height: u32,
    hot_x: u16,
    hot_y: u16,
}

impl SpriteBufferLease {
    pub(crate) fn version(self) -> u64 {
        self.version
    }

    pub(crate) fn matches(self, version: u64, hot_x: u16, hot_y: u16) -> bool {
        self.version == version && self.hot_x == hot_x && self.hot_y == hot_y
    }
}

/// Whether a requested cursor image is ready to bind or must be retried.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SpriteStage {
    Ready,
    Deferred,
}

struct SpriteBuffer {
    buffer: CursorBuffer,
    version: Option<u64>,
    hotspot: Option<(u16, u16)>,
    image_size: Option<(u32, u32)>,
    users: usize,
    prepared_users: usize,
}

/// A per-DRM-device pool. Every slot uses the full driver cursor capacity;
/// each image occupies the top-left corner and every other mapped byte is
/// cleared before a successful write.
pub(crate) struct SpritePool {
    width: u32,
    height: u32,
    buffers: Vec<SpriteBuffer>,
    max_buffers: usize,
    wanted: Option<WantedSprite>,
    candidate: Option<SpriteBufferLease>,
    last_prepare_outcome: Option<SpriteStage>,
}

struct WantedSprite {
    version: u64,
    hot_x: u16,
    hot_y: u16,
    width: u32,
    height: u32,
    bgra_bytes: Vec<u8>,
}

impl SpritePool {
    pub(crate) fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            buffers: Vec::new(),
            max_buffers: 2,
            wanted: None,
            candidate: None,
            last_prepare_outcome: None,
        }
    }

    pub(crate) fn set_active_crtc_count(&mut self, count: usize) {
        self.max_buffers = count.saturating_add(1).max(2);
    }

    pub(crate) fn prepare(
        &mut self,
        device: Rc<Device>,
        io: Rc<dyn CursorBufferIo>,
        version: u64,
        hot_x: u16,
        hot_y: u16,
        image_width: u32,
        image_height: u32,
        bgra_bytes: &[u8],
    ) -> io::Result<SpriteStage> {
        let outcome = self.prepare_inner(
            device,
            io,
            version,
            hot_x,
            hot_y,
            image_width,
            image_height,
            bgra_bytes,
        )?;
        self.last_prepare_outcome = Some(outcome);
        Ok(outcome)
    }

    fn prepare_inner(
        &mut self,
        device: Rc<Device>,
        io: Rc<dyn CursorBufferIo>,
        version: u64,
        hot_x: u16,
        hot_y: u16,
        image_width: u32,
        image_height: u32,
        bgra_bytes: &[u8],
    ) -> io::Result<SpriteStage> {
        validate_image(
            self.width,
            self.height,
            image_width,
            image_height,
            bgra_bytes,
        )?;

        // Keep the latest valid request before any allocation or write can fail.
        self.wanted = Some(WantedSprite {
            version,
            hot_x,
            hot_y,
            width: image_width,
            height: image_height,
            bgra_bytes: bgra_bytes.to_vec(),
        });

        if self.buffers.is_empty() {
            let first = match CursorBuffer::allocate(
                Rc::clone(&device),
                Rc::clone(&io),
                self.width,
                self.height,
            ) {
                Ok(buffer) => buffer,
                Err(error) => return allocation_outcome(error),
            };
            let second = match CursorBuffer::allocate(
                Rc::clone(&device),
                Rc::clone(&io),
                self.width,
                self.height,
            ) {
                Ok(buffer) => buffer,
                Err(error) => {
                    if let Err(cleanup_error) = first.release() {
                        return Err(io::Error::other(format!(
                            "{error}; releasing first cursor sprite allocation failed: {cleanup_error}"
                        )));
                    }
                    return allocation_outcome(error);
                }
            };
            self.buffers = vec![
                SpriteBuffer {
                    buffer: first,
                    version: None,
                    hotspot: None,
                    image_size: None,
                    users: 0,
                    prepared_users: 0,
                },
                SpriteBuffer {
                    buffer: second,
                    version: None,
                    hotspot: None,
                    image_size: None,
                    users: 0,
                    prepared_users: 0,
                },
            ];
        }

        // A matching identity can live in any slot, including one currently
        // held by another CRTC. Reusing it never writes to the framebuffer.
        if let Some((slot, buffer)) = self.buffers.iter().enumerate().find(|(_, buffer)| {
            buffer.version == Some(version) && buffer.hotspot == Some((hot_x, hot_y))
        }) {
            let (width, height) = buffer.image_size.expect("identified sprite has dimensions");
            let lease = SpriteBufferLease {
                slot,
                version,
                width,
                height,
                hot_x,
                hot_y,
            };
            self.candidate = Some(lease);
            return Ok(SpriteStage::Ready);
        }

        let available = |buffer: &SpriteBuffer| buffer.users == 0 && buffer.prepared_users == 0;
        let slot = if let Some((slot, _)) =
            self.buffers.iter().enumerate().find(|(slot, buffer)| {
                available(buffer)
                    && self
                        .candidate
                        .is_none_or(|candidate| candidate.slot != *slot)
            }) {
            slot
        } else if let Some((slot, _)) = self
            .buffers
            .iter()
            .enumerate()
            .find(|(_, buffer)| available(buffer))
        {
            slot
        } else if self.buffers.len() < self.max_buffers {
            let buffer = match CursorBuffer::allocate(device, io, self.width, self.height) {
                Ok(buffer) => buffer,
                Err(error) => return allocation_outcome(error),
            };
            self.buffers.push(SpriteBuffer {
                buffer,
                version: None,
                hotspot: None,
                image_size: None,
                users: 0,
                prepared_users: 0,
            });
            self.buffers.len() - 1
        } else {
            return Ok(SpriteStage::Deferred);
        };

        let wanted = self.wanted.as_ref().expect("wanted sprite was recorded");
        match self.buffers[slot]
            .buffer
            .write_image(wanted.width, wanted.height, &wanted.bgra_bytes)
        {
            Ok(()) => {
                self.buffers[slot].version = Some(wanted.version);
                self.buffers[slot].hotspot = Some((wanted.hot_x, wanted.hot_y));
                self.buffers[slot].image_size = Some((wanted.width, wanted.height));
                self.candidate = Some(SpriteBufferLease {
                    slot,
                    version: wanted.version,
                    width: wanted.width,
                    height: wanted.height,
                    hot_x: wanted.hot_x,
                    hot_y: wanted.hot_y,
                });
                Ok(SpriteStage::Ready)
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(SpriteStage::Deferred),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn candidate_lease(&self) -> Option<SpriteBufferLease> {
        self.candidate
    }

    pub(crate) fn framebuffer_for_lease(&self, lease: SpriteBufferLease) -> Option<u32> {
        self.buffers
            .get(lease.slot)
            .filter(|buffer| lease_matches(buffer, lease))
            .map(|buffer| u32::from(buffer.buffer.fb_id()))
    }

    pub(crate) fn reserve(&mut self, lease: SpriteBufferLease) -> bool {
        let Some(buffer) = self.buffers.get_mut(lease.slot) else {
            return false;
        };
        if !lease_matches(buffer, lease) {
            return false;
        }
        buffer.prepared_users = buffer.prepared_users.saturating_add(1);
        true
    }

    pub(crate) fn can_accept_reserved(&self, lease: SpriteBufferLease) -> bool {
        self.buffers
            .get(lease.slot)
            .is_some_and(|buffer| lease_matches(buffer, lease) && buffer.prepared_users > 0)
    }

    pub(crate) fn accept_reserved(&mut self, lease: SpriteBufferLease) -> bool {
        let Some(buffer) = self.buffers.get_mut(lease.slot) else {
            return false;
        };
        if !lease_matches(buffer, lease) || buffer.prepared_users == 0 {
            return false;
        }
        buffer.prepared_users -= 1;
        buffer.users = buffer.users.saturating_add(1);
        true
    }

    pub(crate) fn reject_reserved(&mut self, lease: SpriteBufferLease) -> bool {
        let Some(buffer) = self.buffers.get_mut(lease.slot) else {
            return false;
        };
        if !lease_matches(buffer, lease) || buffer.prepared_users == 0 {
            return false;
        }
        buffer.prepared_users -= 1;
        true
    }

    pub(crate) fn release_user(&mut self, lease: SpriteBufferLease) -> bool {
        let Some(buffer) = self.buffers.get_mut(lease.slot) else {
            return false;
        };
        if !lease_matches(buffer, lease) || buffer.users == 0 {
            return false;
        }
        buffer.users -= 1;
        true
    }

    #[cfg(test)]
    pub(crate) fn cancel_candidate(&mut self, lease: SpriteBufferLease) -> bool {
        if self.candidate != Some(lease) {
            return false;
        }
        let Some(buffer) = self.buffers.get_mut(lease.slot) else {
            return false;
        };
        if buffer.users != 0 || buffer.prepared_users != 0 || !lease_matches(buffer, lease) {
            return false;
        }
        buffer.version = None;
        buffer.hotspot = None;
        buffer.image_size = None;
        self.candidate = None;
        true
    }

    #[cfg(test)]
    pub(crate) fn wanted_version(&self) -> Option<u64> {
        self.wanted.as_ref().map(|wanted| wanted.version)
    }

    #[cfg(test)]
    pub(crate) fn user_count(&self, lease: SpriteBufferLease) -> Option<usize> {
        self.buffers
            .get(lease.slot)
            .filter(|buffer| lease_matches(buffer, lease))
            .map(|buffer| buffer.users)
    }

    #[cfg(test)]
    pub(crate) fn all_users_zero(&self) -> bool {
        self.buffers
            .iter()
            .all(|buffer| buffer.users == 0 && buffer.prepared_users == 0)
    }

    #[cfg(test)]
    pub(crate) fn for_tests_max_buffer_count(&self) -> usize {
        self.max_buffers
    }

    #[cfg(test)]
    pub(crate) fn for_tests_buffer_count(&self) -> usize {
        self.buffers.len()
    }

    #[cfg(test)]
    pub(crate) fn prepared_user_count(&self) -> usize {
        self.buffers
            .iter()
            .map(|buffer| buffer.prepared_users)
            .sum()
    }

    #[cfg(test)]
    pub(crate) fn candidate_version(&self) -> Option<u64> {
        self.candidate.map(|lease| lease.version)
    }

    #[cfg(test)]
    pub(crate) fn for_tests_last_prepare_outcome(&self) -> Option<SpriteStage> {
        self.last_prepare_outcome
    }

    #[cfg(test)]
    pub(crate) fn for_tests_wanted_identity(&self) -> Option<(u64, u16, u16)> {
        self.wanted
            .as_ref()
            .map(|wanted| (wanted.version, wanted.hot_x, wanted.hot_y))
    }

    #[cfg(test)]
    pub(crate) fn for_tests_sprite_bytes(&self, lease: SpriteBufferLease) -> Option<Vec<u8>> {
        self.buffers
            .get(lease.slot)
            .filter(|buffer| lease_matches(buffer, lease))
            .map(|buffer| buffer.buffer.for_tests_bytes())
    }
}

impl Drop for SpritePool {
    fn drop(&mut self) {
        for slot in self.buffers.drain(..) {
            if slot.users != 0 || slot.prepared_users != 0 {
                std::mem::forget(slot.buffer);
            } else if let Err(error) = slot.buffer.release() {
                log::warn!("cursor sprite buffer release failed during pool drop: {error}");
            }
        }
    }
}

fn lease_matches(buffer: &SpriteBuffer, lease: SpriteBufferLease) -> bool {
    buffer.version == Some(lease.version) && buffer.hotspot == Some((lease.hot_x, lease.hot_y))
}

fn allocation_outcome(error: io::Error) -> io::Result<SpriteStage> {
    if error.kind() == io::ErrorKind::WouldBlock {
        log::debug!("cursor sprite pool allocation deferred: {error}");
        Ok(SpriteStage::Deferred)
    } else {
        Err(error)
    }
}

fn validate_image(
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
            "cursor exceeds hardware plane size",
        ));
    }
    let expected = usize::try_from(image_width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .and_then(|row| {
            usize::try_from(image_height)
                .ok()
                .and_then(|height| row.checked_mul(height))
        })
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "cursor byte size overflow"))?;
    if bgra_bytes.len() != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cursor bytes must equal width*height*4",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
