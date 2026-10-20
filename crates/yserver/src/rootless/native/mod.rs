//! What every window-system host with global coordinates
//! (`CoordinatePolicy::Global`) shares: display space, geometry sync, window
//! style and state, seat state and frame pacing. Window-system neutral and
//! tested with fakes.

// The window-system hosts that use these are not built on every target.
#![cfg_attr(not(test), allow(dead_code))]

pub(crate) mod geometry;
pub(crate) mod pacing;
pub(crate) mod seat;
pub(crate) mod space;
pub(crate) mod style;

#[cfg(test)]
mod tests;
