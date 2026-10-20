//! Frame pacing for a host with one display tick (D3, RL1 D7).

use std::sync::{Arc, Mutex};

use yserver_core::rootless::ToplevelId;

use crate::{rootless::host::FrameEvent, surface::link::ShellLink};

/// Ids whose frame is wanted, answered with `Ready` on the next tick.
pub(crate) struct FramePacer {
    wanted: Mutex<Vec<ToplevelId>>,
    link: Arc<ShellLink>,
}

impl FramePacer {
    pub(crate) fn new(link: Arc<ShellLink>) -> Self {
        Self {
            wanted: Mutex::new(Vec::new()),
            link,
        }
    }

    /// `RootlessHost::frame_wanted`, on the core thread.
    pub(crate) fn want(&self, id: ToplevelId) {
        let mut wanted = self
            .wanted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !wanted.contains(&id) {
            wanted.push(id);
        }
    }

    /// The display tick, on its own thread.
    pub(crate) fn tick(&self) {
        let wanted = std::mem::take(
            &mut *self
                .wanted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for id in wanted {
            self.link.post_frame(FrameEvent::Ready { id });
        }
    }
}
