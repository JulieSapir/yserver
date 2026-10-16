use std::collections::HashMap;

use drm::control::crtc;

const EINVAL_SUSPEND_THRESHOLD: u8 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FlipTiming {
    Vsync,
    Async,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct AsyncShape {
    pub(crate) fourcc: u32,
    pub(crate) modifier: u64,
    pub(crate) pitch: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) crtcs: Vec<crtc::Handle>,
    pub(crate) direct: bool,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum AsyncStream {
    Direct,
    Composed(crtc::Handle),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AsyncOutcome {
    NotRequested,
    Accepted,
    FellBack { suspended: bool },
}

#[derive(Default)]
pub(crate) struct AsyncFlipLedger {
    streams: HashMap<AsyncStream, StreamState>,
}

struct StreamState {
    shape: AsyncShape,
    consecutive_einval: u8,
    suspended: bool,
}

impl AsyncFlipLedger {
    /// Make this submitted shape current for its stream before checking
    /// suspension or attempting the atomic request. Rejections other than
    /// EINVAL still replace a different shape's history.
    pub(crate) fn observe_shape(&mut self, stream: AsyncStream, shape: &AsyncShape) {
        let state = self.streams.entry(stream).or_insert_with(|| StreamState {
            shape: shape.clone(),
            consecutive_einval: 0,
            suspended: false,
        });
        if state.shape != *shape {
            *state = StreamState {
                shape: shape.clone(),
                consecutive_einval: 0,
                suspended: false,
            };
        }
    }

    pub(crate) fn allows(&self, stream: AsyncStream, shape: &AsyncShape) -> bool {
        self.streams
            .get(&stream)
            .is_none_or(|state| state.shape != *shape || !state.suspended)
    }

    /// Record one rejected async request. Returns true only when this error
    /// moves the current stream shape into suspension.
    pub(crate) fn note_einval(&mut self, stream: AsyncStream, shape: &AsyncShape) -> bool {
        let state = self.streams.entry(stream).or_insert_with(|| StreamState {
            shape: shape.clone(),
            consecutive_einval: 0,
            suspended: false,
        });
        if state.shape != *shape {
            *state = StreamState {
                shape: shape.clone(),
                consecutive_einval: 0,
                suspended: false,
            };
        }
        state.consecutive_einval = state.consecutive_einval.saturating_add(1);
        if state.consecutive_einval >= EINVAL_SUSPEND_THRESHOLD && !state.suspended {
            state.suspended = true;
            log::warn!(
                "kms: suspending async page flips for stream {stream:?} shape {shape:?} after {} consecutive EINVALs",
                state.consecutive_einval,
            );
            true
        } else {
            false
        }
    }

    /// An accepted async commit breaks the consecutive EINVAL sequence. A
    /// changed shape replaces this stream's previous history and suspension.
    pub(crate) fn note_accepted(&mut self, stream: AsyncStream, shape: &AsyncShape) {
        let state = self.streams.entry(stream).or_insert_with(|| StreamState {
            shape: shape.clone(),
            consecutive_einval: 0,
            suspended: false,
        });
        if state.shape != *shape {
            *state = StreamState {
                shape: shape.clone(),
                consecutive_einval: 0,
                suspended: false,
            };
        } else {
            state.consecutive_einval = 0;
        }
    }

    #[allow(dead_code)] // invoked by the backend lifecycle transition in Task 3
    pub(crate) fn clear(&mut self) {
        self.streams.clear();
    }

    #[cfg(test)]
    pub(crate) fn for_tests_consecutive_einval(&self, stream: AsyncStream) -> u8 {
        self.streams
            .get(&stream)
            .map_or(0, |state| state.consecutive_einval)
    }

    #[cfg(test)]
    pub(crate) fn for_tests_is_empty(&self) -> bool {
        self.streams.is_empty()
    }
}
