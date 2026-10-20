//! ICCCM/EWMH window information as a host sees it (spec §4.3).

use yserver_protocol::x11::{AtomId, ResourceId};

use super::types::ToplevelId;
use crate::{
    properties::{PropertyFormat, PropertyValue},
    server::ServerState,
};

/// `WM_NORMAL_HINTS` (ICCCM §4.1.2.3).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SizeHints {
    pub us_position: bool,
    pub p_position: bool,
    pub min: Option<(u32, u32)>,
    pub max: Option<(u32, u32)>,
    pub inc: Option<(u32, u32)>,
    pub min_aspect: Option<(u32, u32)>,
    pub max_aspect: Option<(u32, u32)>,
    pub base: Option<(u32, u32)>,
    pub gravity: Option<u32>,
}

/// `WM_PROTOCOLS` atoms the core understands.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Protocols {
    pub delete_window: bool,
    pub take_focus: bool,
    pub ping: bool,
}

/// `_MOTIF_WM_HINTS` decorations.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Decorations {
    #[default]
    Full,
    None,
    /// Motif decoration bits (`MWM_DECOR_*`) when neither none nor all.
    Subset(u32),
}

/// `_NET_WM_WINDOW_TYPE`, first known atom (EWMH §5.6).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum WindowType {
    #[default]
    Normal,
    Dialog,
    Utility,
    Toolbar,
    Menu,
    Splash,
    Dock,
    Desktop,
    DropdownMenu,
    PopupMenu,
    Tooltip,
    Notification,
    Combo,
    Dnd,
}

/// `_NET_WM_STATE` (EWMH §5.7).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct NetWmState {
    pub fullscreen: bool,
    pub maximized_vert: bool,
    pub maximized_horz: bool,
    pub above: bool,
    pub below: bool,
    pub hidden: bool,
    pub skip_taskbar: bool,
    pub modal: bool,
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct WindowInfo {
    pub title: String,
    pub class: Option<(String, String)>,
    pub transient_for: Option<ToplevelId>,
    pub window_type: WindowType,
    pub size_hints: SizeHints,
    pub input_hint: Option<bool>,
    pub urgency: bool,
    pub protocols: Protocols,
    pub decorations: Decorations,
    pub state: NetWmState,
    pub pid: Option<u32>,
    /// `_NET_WM_ICON` is present; the host fetches pixels on demand.
    pub has_icon: bool,
}

/// Which `WindowInfo` fields an `InfoChanged` carries new values for.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct InfoFields(u16);

impl InfoFields {
    pub const TITLE: Self = Self(1);
    pub const CLASS: Self = Self(1 << 1);
    pub const TRANSIENT_FOR: Self = Self(1 << 2);
    pub const WINDOW_TYPE: Self = Self(1 << 3);
    pub const SIZE_HINTS: Self = Self(1 << 4);
    pub const INPUT_HINT: Self = Self(1 << 5);
    pub const URGENCY: Self = Self(1 << 6);
    pub const PROTOCOLS: Self = Self(1 << 7);
    pub const DECORATIONS: Self = Self(1 << 8);
    pub const STATE: Self = Self(1 << 9);
    pub const PID: Self = Self(1 << 10);
    pub const ICON: Self = Self(1 << 11);

    #[must_use]
    pub fn empty() -> Self {
        Self(0)
    }

    #[must_use]
    pub fn bits(self) -> u16 {
        self.0
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[must_use]
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for InfoFields {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for InfoFields {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl WindowInfo {
    /// The fields whose values differ from `old`.
    #[must_use]
    pub fn changed_from(&self, old: &Self) -> InfoFields {
        let mut f = InfoFields::empty();
        for (changed, field) in [
            (self.title != old.title, InfoFields::TITLE),
            (self.class != old.class, InfoFields::CLASS),
            (
                self.transient_for != old.transient_for,
                InfoFields::TRANSIENT_FOR,
            ),
            (self.window_type != old.window_type, InfoFields::WINDOW_TYPE),
            (self.size_hints != old.size_hints, InfoFields::SIZE_HINTS),
            (self.input_hint != old.input_hint, InfoFields::INPUT_HINT),
            (self.urgency != old.urgency, InfoFields::URGENCY),
            (self.protocols != old.protocols, InfoFields::PROTOCOLS),
            (self.decorations != old.decorations, InfoFields::DECORATIONS),
            (self.state != old.state, InfoFields::STATE),
            (self.pid != old.pid, InfoFields::PID),
            (self.has_icon != old.has_icon, InfoFields::ICON),
        ] {
            if changed {
                f |= field;
            }
        }
        f
    }
}

/// Properties `parse_window_info` reads; a change to any other is ignored.
pub(super) const INFO_PROPERTIES: &[&str] = &[
    "_NET_WM_NAME",
    "WM_NAME",
    "WM_CLASS",
    "WM_TRANSIENT_FOR",
    "_NET_WM_WINDOW_TYPE",
    "WM_NORMAL_HINTS",
    "WM_HINTS",
    "WM_PROTOCOLS",
    "_MOTIF_WM_HINTS",
    "_NET_WM_STATE",
    "_NET_WM_PID",
    "_NET_WM_ICON",
];

fn prop<'a>(state: &'a ServerState, window: ResourceId, name: &str) -> Option<&'a PropertyValue> {
    let atom = state.atoms.id_for(name)?;
    state.resources.window_property(window, atom)
}

/// Format-32 items, in server (LE) order since property data is canonical.
fn words(v: &PropertyValue) -> Vec<u32> {
    if v.format != PropertyFormat::F32 {
        return Vec::new();
    }
    v.data
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn atom_names<'a>(state: &'a ServerState, v: &PropertyValue) -> Vec<&'a str> {
    words(v)
        .into_iter()
        .filter_map(|a| state.atoms.name(AtomId(a)))
        .collect()
}

fn is_type(state: &ServerState, v: &PropertyValue, name: &str) -> bool {
    state.atoms.name(v.r#type) == Some(name)
}

fn trim_nul(bytes: &[u8]) -> &[u8] {
    let end = bytes.iter().rposition(|b| *b != 0).map_or(0, |i| i + 1);
    &bytes[..end]
}

/// `_NET_WM_NAME` (UTF8_STRING) in preference to `WM_NAME` (EWMH §5.1);
/// `WM_NAME` `STRING` is Latin-1 (ICCCM §2.7.1), `COMPOUND_TEXT` its ASCII.
fn title(state: &ServerState, window: ResourceId) -> String {
    if let Some(v) = prop(state, window, "_NET_WM_NAME")
        && v.format == PropertyFormat::F8
        && is_type(state, v, "UTF8_STRING")
    {
        return String::from_utf8_lossy(trim_nul(&v.data)).into_owned();
    }
    let Some(v) = prop(state, window, "WM_NAME").filter(|v| v.format == PropertyFormat::F8) else {
        return String::new();
    };
    let bytes = trim_nul(&v.data);
    match state.atoms.name(v.r#type) {
        Some("UTF8_STRING") => String::from_utf8_lossy(bytes).into_owned(),
        Some("COMPOUND_TEXT") => bytes
            .iter()
            .filter(|b| b.is_ascii() && (!b.is_ascii_control() || **b == b'\t' || **b == b'\n'))
            .map(|b| char::from(*b))
            .collect(),
        _ => bytes.iter().map(|b| char::from(*b)).collect(),
    }
}

/// `WM_CLASS`: instance and class, NUL-terminated (ICCCM §4.1.2.5).
fn wm_class(v: &PropertyValue) -> Option<(String, String)> {
    if v.format != PropertyFormat::F8 {
        return None;
    }
    let mut parts = v.data.split(|b| *b == 0);
    let instance = String::from_utf8_lossy(parts.next()?).into_owned();
    let class = String::from_utf8_lossy(parts.next().unwrap_or_default()).into_owned();
    Some((instance, class))
}

/// `WM_NORMAL_HINTS` (ICCCM §4.1.2.3; flag bits `Xutil.h:102-112`): flags,
/// 4 pad, min, max, inc, min/max aspect, then base and gravity (18 words).
/// Fewer than 15 words is no hint, as `XGetWMNormalHints`.
fn wm_normal_hints(v: &PropertyValue) -> SizeHints {
    let w = words(v);
    if w.len() < 15 {
        return SizeHints::default();
    }
    let flags = w[0];
    let pair = |bit: u32, at: usize| (flags & bit != 0).then(|| (w[at], w[at + 1]));
    let long = w.len() >= 18;
    SizeHints {
        us_position: flags & 1 != 0,
        p_position: flags & (1 << 2) != 0,
        min: pair(1 << 4, 5),
        max: pair(1 << 5, 7),
        inc: pair(1 << 6, 9),
        min_aspect: pair(1 << 7, 11),
        max_aspect: pair(1 << 7, 13),
        base: if long { pair(1 << 8, 15) } else { None },
        gravity: (long && flags & (1 << 9) != 0).then(|| w[17]),
    }
}

/// `WM_HINTS` (ICCCM §4.1.2.4; `Xutil.h:134-143`): (input, urgency).
fn wm_hints(v: &PropertyValue) -> (Option<bool>, bool) {
    let w = words(v);
    let Some(&flags) = w.first() else {
        return (None, false);
    };
    let input = (flags & 1 != 0).then(|| w.get(1).is_some_and(|i| *i != 0));
    (input, flags & (1 << 8) != 0)
}

fn wm_protocols(state: &ServerState, v: &PropertyValue) -> Protocols {
    let names = atom_names(state, v);
    Protocols {
        delete_window: names.contains(&"WM_DELETE_WINDOW"),
        take_focus: names.contains(&"WM_TAKE_FOCUS"),
        ping: names.contains(&"_NET_WM_PING"),
    }
}

/// `_MOTIF_WM_HINTS` flags, functions, decorations (`hw/xwin/winwindow.h:114-136`).
fn motif_decorations(v: &PropertyValue) -> Decorations {
    let w = words(v);
    if w.len() < 3 || w[0] & (1 << 1) == 0 {
        return Decorations::Full;
    }
    match w[2] {
        0 => Decorations::None,
        d if d & 1 != 0 => Decorations::Full,
        d => Decorations::Subset(d),
    }
}

/// First known `_NET_WM_WINDOW_TYPE` atom; none → Dialog when transient,
/// else Normal (EWMH §5.6).
fn net_wm_window_type(
    state: &ServerState,
    v: Option<&PropertyValue>,
    transient: bool,
) -> WindowType {
    let known = v
        .map(|v| atom_names(state, v))
        .unwrap_or_default()
        .into_iter()
        .find_map(|n| {
            Some(match n.strip_prefix("_NET_WM_WINDOW_TYPE_")? {
                "NORMAL" => WindowType::Normal,
                "DIALOG" => WindowType::Dialog,
                "UTILITY" => WindowType::Utility,
                "TOOLBAR" => WindowType::Toolbar,
                "MENU" => WindowType::Menu,
                "SPLASH" => WindowType::Splash,
                "DOCK" => WindowType::Dock,
                "DESKTOP" => WindowType::Desktop,
                "DROPDOWN_MENU" => WindowType::DropdownMenu,
                "POPUP_MENU" => WindowType::PopupMenu,
                "TOOLTIP" => WindowType::Tooltip,
                "NOTIFICATION" => WindowType::Notification,
                "COMBO" => WindowType::Combo,
                "DND" => WindowType::Dnd,
                _ => return None,
            })
        });
    known.unwrap_or(if transient {
        WindowType::Dialog
    } else {
        WindowType::Normal
    })
}

fn net_wm_state(state: &ServerState, v: &PropertyValue) -> NetWmState {
    let mut s = NetWmState::default();
    for name in atom_names(state, v) {
        match name {
            "_NET_WM_STATE_FULLSCREEN" => s.fullscreen = true,
            "_NET_WM_STATE_MAXIMIZED_VERT" => s.maximized_vert = true,
            "_NET_WM_STATE_MAXIMIZED_HORZ" => s.maximized_horz = true,
            "_NET_WM_STATE_ABOVE" => s.above = true,
            "_NET_WM_STATE_BELOW" => s.below = true,
            "_NET_WM_STATE_HIDDEN" => s.hidden = true,
            "_NET_WM_STATE_SKIP_TASKBAR" => s.skip_taskbar = true,
            "_NET_WM_STATE_MODAL" => s.modal = true,
            _ => {}
        }
    }
    s
}

/// The window's `WM_NORMAL_HINTS` as set now.
pub(super) fn size_hints_of(state: &ServerState, window: ResourceId) -> SizeHints {
    prop(state, window, "WM_NORMAL_HINTS").map_or_else(SizeHints::default, wm_normal_hints)
}

/// The window's `_NET_WM_STATE` as set now.
pub(super) fn current_net_wm_state(state: &ServerState, window: ResourceId) -> NetWmState {
    prop(state, window, "_NET_WM_STATE")
        .map_or_else(NetWmState::default, |v| net_wm_state(state, v))
}

/// The `WM_TRANSIENT_FOR` window, if any.
pub(super) fn transient_for_window(state: &ServerState, window: ResourceId) -> Option<ResourceId> {
    prop(state, window, "WM_TRANSIENT_FOR")
        .and_then(|v| words(v).first().copied())
        .filter(|w| *w != 0)
        .map(ResourceId)
}

/// Everything a host needs about `window` (spec §4.3). `ids` resolves a
/// window to its live top-level id.
pub fn parse_window_info(
    state: &ServerState,
    window: ResourceId,
    ids: &dyn Fn(ResourceId) -> Option<ToplevelId>,
) -> WindowInfo {
    let transient = transient_for_window(state, window);
    let (input_hint, urgency) = prop(state, window, "WM_HINTS").map_or((None, false), wm_hints);
    WindowInfo {
        title: title(state, window),
        class: prop(state, window, "WM_CLASS").and_then(wm_class),
        transient_for: transient.and_then(ids),
        window_type: net_wm_window_type(
            state,
            prop(state, window, "_NET_WM_WINDOW_TYPE"),
            transient.is_some(),
        ),
        size_hints: prop(state, window, "WM_NORMAL_HINTS")
            .map_or_else(SizeHints::default, wm_normal_hints),
        input_hint,
        urgency,
        protocols: prop(state, window, "WM_PROTOCOLS")
            .map_or_else(Protocols::default, |v| wm_protocols(state, v)),
        decorations: prop(state, window, "_MOTIF_WM_HINTS")
            .map_or(Decorations::Full, motif_decorations),
        state: prop(state, window, "_NET_WM_STATE")
            .map_or_else(NetWmState::default, |v| net_wm_state(state, v)),
        pid: prop(state, window, "_NET_WM_PID").and_then(|v| words(v).first().copied()),
        has_icon: prop(state, window, "_NET_WM_ICON").is_some(),
    }
}
