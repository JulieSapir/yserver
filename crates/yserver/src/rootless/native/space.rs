//! The window system's global space (points, bottom-left origin, y up) and
//! the X root (spec §4.0, §5 Global).

/// Points, bottom-left origin, y up (the window system's global space).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct NativeRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// X root = bounding box of every display (spec §5 Global, D1); one scale
/// S = X pixels per point (decision 6, D2).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct GlobalSpace {
    left: f64,
    top: f64,
    primary_top: f64,
    scale: f64,
    root: (u16, u16),
}

impl GlobalSpace {
    /// `displays[0]` is the primary (origin (0,0), the menu bar); its height
    /// is `primary_top`. The root is clamped to `max_side`.
    pub(crate) fn new(displays: &[NativeRect], scale: f64, max_side: u16) -> Self {
        let fold = |f: fn(f64, f64) -> f64, side: fn(&NativeRect) -> f64, empty: f64| {
            displays.iter().map(side).reduce(f).unwrap_or(empty)
        };
        let left = fold(f64::min, |d| d.x, 0.0);
        let right = fold(f64::max, |d| d.x + d.width, 0.0);
        let bottom = fold(f64::min, |d| d.y, 0.0);
        let top = fold(f64::max, |d| d.y + d.height, 0.0);
        let side = |points: f64| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let px = (points * scale).ceil().clamp(1.0, f64::from(max_side)) as u16;
            px
        };
        Self {
            left,
            top,
            primary_top: displays.first().map_or(top, |p| p.y + p.height),
            scale,
            root: (side(right - left), side(top - bottom)),
        }
    }

    pub(crate) fn root(&self) -> (u16, u16) {
        self.root
    }

    pub(crate) fn scale(&self) -> f64 {
        self.scale
    }

    /// X outer corner + native content extent (w+2bw, h+2bw) → native
    /// content rect.
    pub(crate) fn to_native(self, origin: (i32, i32), extent: (u16, u16)) -> NativeRect {
        let s = self.scale;
        NativeRect {
            x: self.left + f64::from(origin.0) / s,
            y: self.top - (f64::from(origin.1) + f64::from(extent.1)) / s,
            width: f64::from(extent.0) / s,
            height: f64::from(extent.1) / s,
        }
    }

    /// Native content rect → X outer corner and extent, whole X pixels.
    pub(crate) fn to_x(self, content: NativeRect) -> ((i32, i32), (u16, u16)) {
        let s = self.scale;
        #[allow(clippy::cast_possible_truncation)]
        let px = |v: f64| (v * s).round() as i32;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let len = |v: f64| (v * s).round().clamp(0.0, f64::from(u16::MAX)) as u16;
        (
            (
                px(content.x - self.left),
                px(self.top - (content.y + content.height)),
            ),
            (len(content.width), len(content.height)),
        )
    }

    /// Root pixel → global point with the origin at the primary's top-left,
    /// y down, for the pointer warp.
    pub(crate) fn to_warp_point(self, root: (i32, i32)) -> (f64, f64) {
        let s = self.scale;
        (
            self.left + f64::from(root.0) / s,
            f64::from(root.1) / s - (self.top - self.primary_top),
        )
    }
}

/// Whether `point` is in `content`, the X area of a native window: a title
/// bar or frame around it is none of X's (bottom-left origin, half-open).
pub(crate) fn content_contains(content: NativeRect, point: (f64, f64)) -> bool {
    (content.x..content.x + content.width).contains(&point.0)
        && (content.y..content.y + content.height).contains(&point.1)
}

/// Spec §4.0 puts the native content rect at the X outer rect, with any
/// title bar above it. As `XQuartz` does, the title bar stays reachable: a
/// content rect whose `title_bar` (points) would rise above `visible` (the
/// display's area below the menu bar) moves down until it fits. The host
/// reports the result to X (`GeometrySync::native_changed`).
pub(crate) fn below_title_bar(
    content: NativeRect,
    title_bar: f64,
    visible: NativeRect,
) -> NativeRect {
    let top = visible.y + visible.height - title_bar;
    if content.y + content.height <= top {
        return content;
    }
    NativeRect {
        y: top - content.height,
        ..content
    }
}
