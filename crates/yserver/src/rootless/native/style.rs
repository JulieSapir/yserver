//! A top-level's native window style, content limits and state, from
//! `WindowInfo` (spec §8.1; XWin `winApplyHints` as precedent,
//! `winmultiwindowwm.c:1723-1959`).

use yserver_core::rootless::{
    Decorations, HostRequest, Layer, NetWmState, SizeHints, ToplevelId, ToplevelKind, WindowInfo,
    WindowType,
};

/// `MWM_DECOR_TITLE` (`hw/xwin/winwindow.h`).
const MWM_DECOR_TITLE: u32 = 1 << 3;

/// Window levels. `Below` = one under the normal level (D13).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum NativeLevel {
    Below,
    Normal,
    Floating,
    PopUp,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct NativeStyle {
    pub titled: bool,
    pub closable: bool,
    pub miniaturizable: bool,
    pub resizable: bool,
    pub level: NativeLevel,
    pub can_activate: bool,
    pub child_of: Option<ToplevelId>,
    /// May go fullscreen (collection behaviour).
    pub fullscreen: bool,
}

pub(crate) fn native_style(kind: ToplevelKind, info: &WindowInfo) -> NativeStyle {
    if let ToplevelKind::Popup { .. } = kind {
        // The level keeps it on top; no child relation.
        return NativeStyle {
            titled: false,
            closable: false,
            miniaturizable: false,
            resizable: false,
            level: NativeLevel::PopUp,
            can_activate: false,
            child_of: None,
            fullscreen: false,
        };
    }
    let decorated = !matches!(info.decorations, Decorations::None);
    let titled = match info.decorations {
        Decorations::Full => true,
        Decorations::None => false,
        Decorations::Subset(bits) => bits & MWM_DECOR_TITLE != 0,
    };
    let hints = &info.size_hints;
    let layer = if info.state.above {
        Layer::Above
    } else if info.state.below {
        Layer::Below
    } else {
        Layer::Normal
    };
    NativeStyle {
        titled,
        closable: decorated,
        miniaturizable: decorated,
        resizable: !(hints.min.is_some() && hints.min == hints.max),
        level: level_for(layer),
        can_activate: true,
        child_of: info.transient_for,
        fullscreen: matches!(info.window_type, WindowType::Normal | WindowType::Dialog),
    }
}

/// `_NET_WM_STATE_ABOVE`/`_BELOW` → level.
pub(crate) fn level_for(layer: Layer) -> NativeLevel {
    match layer {
        Layer::Normal => NativeLevel::Normal,
        Layer::Above => NativeLevel::Floating,
        Layer::Below => NativeLevel::Below,
    }
}

/// Native content limits in X pixels (outer: border included).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct ContentLimits {
    pub min: Option<(u16, u16)>,
    pub max: Option<(u16, u16)>,
    pub inc: Option<(u16, u16)>,
}

/// `WM_NORMAL_HINTS` in native content (outer) X pixels: min/max + 2bw,
/// base for a missing min (ICCCM §4.1.2.3).
pub(crate) fn content_limits(hints: &SizeHints, border: u16) -> ContentLimits {
    let side = |v: u32| u16::try_from(v).unwrap_or(u16::MAX);
    let outer = |(w, h): (u32, u32)| {
        let bw = u32::from(border) * 2;
        (side(w.saturating_add(bw)), side(h.saturating_add(bw)))
    };
    ContentLimits {
        min: hints.min.or(hints.base).map(outer),
        max: hints.max.map(outer),
        inc: hints.inc.map(|(w, h)| (side(w), side(h))),
    }
}

/// What the native window is, read after any change, whoever caused it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct NativeFlags {
    pub zoomed: bool,
    pub fullscreen: bool,
    pub minimized: bool,
    pub level: NativeLevel,
}

/// `_NET_WM_STATE` as the host last reported it. The core never writes it
/// for a client's request (it only emits a `WmRequest`) and computes the
/// next toggle from the property, so every native change must come back as
/// `StateChanged` or a later toggle acts on stale state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct NetStateSync {
    reported: NetWmState,
}

impl NetStateSync {
    pub(crate) fn mapped(client: NetWmState) -> Self {
        Self { reported: client }
    }

    /// Client-owned fields (`skip_taskbar`, `modal`) from
    /// `InfoChanged(STATE)`; they ride along in every report.
    pub(crate) fn client_state(&mut self, client: NetWmState) {
        self.reported.skip_taskbar = client.skip_taskbar;
        self.reported.modal = client.modal;
    }

    /// The full state to send when `native` differs from what X has;
    /// `None` when equal (no echo).
    pub(crate) fn native(&mut self, id: ToplevelId, native: NativeFlags) -> Option<HostRequest> {
        let state = NetWmState {
            fullscreen: native.fullscreen,
            // A native zoom has no single axis.
            maximized_vert: native.zoomed,
            maximized_horz: native.zoomed,
            above: native.level == NativeLevel::Floating,
            below: native.level == NativeLevel::Below,
            hidden: native.minimized,
            ..self.reported
        };
        if state == self.reported {
            return None;
        }
        self.reported = state;
        Some(HostRequest::StateChanged { id, state })
    }
}
