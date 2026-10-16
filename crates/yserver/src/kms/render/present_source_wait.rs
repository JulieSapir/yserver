//! Asynchronous producer waits for Present sources: dma-buf implicit fences
//! for `PresentPixmap`, and explicit DRM syncobj timeline points for
//! `PresentPixmapSynced`.

use std::{
    os::fd::{AsFd, OwnedFd},
    sync::Arc,
};

use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

use crate::kms::render::store::DrawableId;

/// One imported source held alive until its producer fence/timeline point is
/// ready and the core records the deferred copy.
pub(crate) struct PendingPresentSourceWait {
    pub(crate) fds: Vec<PendingWaitFd>,
    pub(crate) source_id: DrawableId,
    pub(crate) prewaited_destination: Option<DrawableId>,
    /// Keeps an explicitly imported acquire syncobj alive until its timeline
    /// point signals. Implicit dma-buf waits leave this empty.
    pub(crate) syncobj_pin: Option<Arc<dyn super::imported_syncobj::PresentSyncobj>>,
    /// Target acquire point. Set whenever an acquire syncobj is present —
    /// note `is_ready` checks it on every poll, not only when
    /// `DRM_SYNCOBJ_EVENTFD` was unavailable.
    pub(crate) timeline_value: Option<u64>,
    pub(crate) poll_timeline: bool,
    /// False when registration in the stable completion poller failed. Such
    /// entries are still checked from the backend's 1 ms polling fallback.
    pub(crate) ready_reported: bool,
}

pub(crate) struct PendingWaitFd {
    pub(crate) fd: OwnedFd,
    pub(crate) registered: bool,
    pub(crate) ready: bool,
}

impl PendingPresentSourceWait {
    pub(crate) fn is_ready(&self) -> bool {
        let timeline_ready = match (&self.syncobj_pin, self.timeline_value) {
            (Some(syncobj), Some(target)) => match syncobj.timeline_value() {
                Ok(current) => current >= target,
                Err(e) => {
                    log::warn!(
                        "deferred Present acquire: syncobj timeline query failed: {e}; \
                         treating as ready"
                    );
                    true
                }
            },
            _ => true,
        };
        timeline_ready
            && self
                .fds
                .iter()
                .all(|wait_fd| wait_fd.ready || fd_is_ready(&wait_fd.fd))
    }
}

impl PendingWaitFd {
    pub(crate) fn refresh_ready(&mut self) -> bool {
        if !self.ready {
            self.ready = fd_is_ready(&self.fd);
        }
        self.ready
    }
}

fn fd_is_ready(fd: &OwnedFd) -> bool {
    let mut fds = [PollFd::new(fd.as_fd(), PollFlags::POLLIN)];
    match poll(&mut fds, PollTimeout::ZERO) {
        Ok(0) => false,
        Ok(_) => fds[0].revents().is_some_and(|flags| {
            flags.intersects(PollFlags::POLLIN | PollFlags::POLLERR | PollFlags::POLLHUP)
        }),
        Err(e) => {
            log::warn!("deferred Present source: poll(sync_file): {e}; treating as ready");
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::OwnedFd;

    use nix::{
        sys::eventfd::{EfdFlags, EventFd},
        unistd::write,
    };

    use super::*;

    #[test]
    fn readiness_tracks_the_sync_file_signal() {
        let event =
            EventFd::from_value_and_flags(0, EfdFlags::EFD_NONBLOCK | EfdFlags::EFD_CLOEXEC)
                .expect("eventfd");
        let fd: OwnedFd = event.into();
        let wait = PendingPresentSourceWait {
            fds: vec![PendingWaitFd {
                fd,
                registered: false,
                ready: false,
            }],
            source_id: DrawableId::for_tests(1),
            prewaited_destination: None,
            syncobj_pin: None,
            timeline_value: None,
            poll_timeline: false,
            ready_reported: false,
        };

        assert!(!wait.is_ready());
        write(&wait.fds[0].fd, &1_u64.to_ne_bytes()).expect("signal eventfd");
        assert!(wait.is_ready());
    }

    #[test]
    fn readiness_requires_every_sync_file() {
        let first =
            EventFd::from_value_and_flags(0, EfdFlags::EFD_NONBLOCK | EfdFlags::EFD_CLOEXEC)
                .expect("first eventfd");
        let second =
            EventFd::from_value_and_flags(0, EfdFlags::EFD_NONBLOCK | EfdFlags::EFD_CLOEXEC)
                .expect("second eventfd");
        let wait = PendingPresentSourceWait {
            fds: vec![
                PendingWaitFd {
                    fd: first.into(),
                    registered: false,
                    ready: false,
                },
                PendingWaitFd {
                    fd: second.into(),
                    registered: false,
                    ready: false,
                },
            ],
            source_id: DrawableId::for_tests(1),
            prewaited_destination: None,
            syncobj_pin: None,
            timeline_value: None,
            poll_timeline: false,
            ready_reported: false,
        };

        write(&wait.fds[0].fd, &1_u64.to_ne_bytes()).expect("signal first");
        assert!(!wait.is_ready());
        write(&wait.fds[1].fd, &1_u64.to_ne_bytes()).expect("signal second");
        assert!(wait.is_ready());
    }
}
