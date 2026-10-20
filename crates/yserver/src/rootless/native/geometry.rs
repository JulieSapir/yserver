//! Native geometry against X's, with host serials (spec §4.2.1, §4.4): the
//! feedback guard at the host edge.

use yserver_core::rootless::{Geometry, GeometryFields, HostRequest, HostSerial, ToplevelId};

/// The host's serial counter: one per geometry or stacking request.
#[derive(Debug, Default)]
pub(crate) struct HostSerials(u64);

impl HostSerials {
    pub(crate) fn next(&mut self) -> HostSerial {
        self.0 += 1;
        HostSerial(self.0)
    }
}

/// What X has or was asked for, per window. The native window is only ever
/// compared with this.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct GeometrySync {
    /// X outer corner.
    origin: (i32, i32),
    /// Native content = X outer extent.
    extent: (u16, u16),
}

/// What to apply to the native window; `None` fields stay.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct NativeTarget {
    pub origin: Option<(i32, i32)>,
    pub extent: Option<(u16, u16)>,
}

impl GeometrySync {
    pub(crate) fn mapped(g: &Geometry) -> Self {
        Self {
            origin: (g.x, g.y),
            extent: outer_extent(g),
        }
    }

    /// `Configured` (§4.2.1 rule 4): apply exactly `client_fields`, also
    /// with newer requests in flight; `None` = leave the native window.
    /// The other fields already are what this host asked for up to `acked`.
    pub(crate) fn configured(
        &mut self,
        g: &Geometry,
        _acked: Option<HostSerial>,
        client_fields: GeometryFields,
    ) -> Option<NativeTarget> {
        let moved =
            client_fields.contains(GeometryFields::X) || client_fields.contains(GeometryFields::Y);
        let sized = [
            GeometryFields::WIDTH,
            GeometryFields::HEIGHT,
            GeometryFields::BORDER,
        ]
        .into_iter()
        .any(|f| client_fields.contains(f));
        if moved {
            self.origin = (g.x, g.y);
        }
        if sized {
            self.extent = outer_extent(g);
        }
        (moved || sized).then_some(NativeTarget {
            origin: moved.then_some(self.origin),
            extent: sized.then_some(self.extent),
        })
    }

    /// The native content is now `(origin, extent)`, whoever moved it: the
    /// requests to post, each with a fresh serial. Equal to what X has →
    /// nothing (an echo).
    pub(crate) fn native_changed(
        &mut self,
        id: ToplevelId,
        origin: (i32, i32),
        extent: (u16, u16),
        serials: &mut HostSerials,
    ) -> Vec<HostRequest> {
        let mut requests = Vec::new();
        if origin != self.origin {
            self.origin = origin;
            requests.push(HostRequest::Moved {
                id,
                x: origin.0,
                y: origin.1,
                serial: serials.next(),
            });
        }
        if extent != self.extent {
            self.extent = extent;
            requests.push(HostRequest::Resized {
                id,
                width: extent.0,
                height: extent.1,
                serial: serials.next(),
            });
        }
        requests
    }
}

/// Spec §4.0: native content = (w + 2bw) × (h + 2bw).
fn outer_extent(g: &Geometry) -> (u16, u16) {
    (
        g.width.saturating_add(g.border.saturating_mul(2)),
        g.height.saturating_add(g.border.saturating_mul(2)),
    )
}
