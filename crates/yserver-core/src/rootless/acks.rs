//! Host serial acknowledgment and client-field coalescing (spec §4.2.1).

use yserver_protocol::x11::ConfigureWindowRequest;

use super::types::{GeometryFields, HostSerial};
use crate::server::ServerState;

/// Who asked for a ConfigureWindow.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConfigureSource {
    Client,
    Host(HostSerial),
    /// The core's own policy (size-hint clamp, transient placement).
    Policy,
}

/// Per top-level, reset after each emitted event (rule 1).
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Acks {
    pub geometry_acked: Option<HostSerial>,
    /// Fields a non-host source set after the last host write of them.
    pub client_fields: GeometryFields,
    /// Fields host requests set since the last event.
    pub host_fields: GeometryFields,
    pub stack_acked: Option<HostSerial>,
    /// A non-host restack was applied after the acked host restack.
    pub from_client: bool,
    /// Any restack request was applied since the last event.
    pub restacked: bool,
}

impl Acks {
    pub fn note(&mut self, fields: GeometryFields, stack: bool, source: ConfigureSource) {
        match source {
            ConfigureSource::Host(serial) => {
                if !fields.is_empty() {
                    self.geometry_acked = self.geometry_acked.max(Some(serial));
                    self.client_fields = self.client_fields.without(fields);
                    self.host_fields |= fields;
                }
                if stack {
                    self.stack_acked = self.stack_acked.max(Some(serial));
                    self.from_client = false;
                }
            }
            ConfigureSource::Client | ConfigureSource::Policy => {
                self.client_fields |= fields;
                if stack {
                    self.from_client = true;
                }
            }
        }
        self.restacked |= stack;
    }

    pub fn take_geometry(&mut self) {
        self.geometry_acked = None;
        self.client_fields = GeometryFields::empty();
        self.host_fields = GeometryFields::empty();
    }

    pub fn take_stack(&mut self) {
        self.stack_acked = None;
        self.from_client = false;
        self.restacked = false;
    }
}

/// Every ConfigureWindow that is applied (not redirected) reports here.
/// No-op unless rootless is on and the window is a tracked top-level.
pub(crate) fn note_configure(
    state: &mut ServerState,
    request: &ConfigureWindowRequest,
    source: ConfigureSource,
) {
    let Some(t) = state
        .rootless
        .as_mut()
        .and_then(|r| r.toplevels.get_mut(&request.window))
    else {
        return;
    };
    let fields = GeometryFields::from_value_mask(request.value_mask);
    t.acks.note(fields, request.value_mask & 0x40 != 0, source);
}

/// Pin `ConfigureSource::Policy` fields onto a window (rule 5: a clamp of a
/// host resize is a non-host change).
pub(super) fn note_policy_fields(
    state: &mut ServerState,
    window: yserver_protocol::x11::ResourceId,
    fields: GeometryFields,
) {
    if let Some(t) = state
        .rootless
        .as_mut()
        .and_then(|r| r.toplevels.get_mut(&window))
    {
        t.acks.client_fields |= fields;
    }
}
