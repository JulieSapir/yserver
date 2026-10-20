//! A top-level's `NSWindow` and `CAMetalLayer` from its `NativeStyle`.

use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSFloatingWindowLevel, NSNormalWindowLevel,
    NSPopUpMenuWindowLevel, NSResponder, NSView, NSWindow, NSWindowAnimationBehavior,
    NSWindowCollectionBehavior, NSWindowLevel, NSWindowStyleMask, NSWindowTabbingMode,
};
use objc2_foundation::{NSRect, NSSize};
use objc2_quartz_core::{CALayer, CAMetalLayer, kCAFilterNearest, kCAGravityTopLeft};

use crate::rootless::native::style::{NativeLevel, NativeStyle};

/// The window level of `level`; `Below` is one under normal (D13).
pub(super) fn window_level(level: NativeLevel) -> NSWindowLevel {
    match level {
        NativeLevel::Below => NSNormalWindowLevel - 1,
        NativeLevel::Normal => NSNormalWindowLevel,
        NativeLevel::Floating => NSFloatingWindowLevel,
        NativeLevel::PopUp => NSPopUpMenuWindowLevel,
    }
}

/// What `level` is, read back.
pub(super) fn level_of(level: NSWindowLevel) -> NativeLevel {
    if level < NSNormalWindowLevel {
        NativeLevel::Below
    } else if level >= NSPopUpMenuWindowLevel {
        NativeLevel::PopUp
    } else if level >= NSFloatingWindowLevel {
        NativeLevel::Floating
    } else {
        NativeLevel::Normal
    }
}

fn style_mask(style: &NativeStyle) -> NSWindowStyleMask {
    if !style.titled {
        return NSWindowStyleMask::Borderless;
    }
    let mut mask = NSWindowStyleMask::Titled;
    if style.closable {
        mask |= NSWindowStyleMask::Closable;
    }
    if style.miniaturizable {
        mask |= NSWindowStyleMask::Miniaturizable;
    }
    if style.resizable {
        mask |= NSWindowStyleMask::Resizable;
    }
    mask
}

/// A window whose content rect is `content` (points, global) and whose
/// content is a layer of `extent` X pixels at `scale` X pixels per point,
/// copied 1:1 and magnified nearest (D2), hosted by `view`.
pub(super) fn create(
    mtm: MainThreadMarker,
    style: &NativeStyle,
    content: NSRect,
    scale: f64,
    depth: u8,
    extent: (u16, u16),
    view: &NSView,
) -> (Retained<NSWindow>, Retained<CAMetalLayer>) {
    // SAFETY: a fresh allocation; released-when-closed is turned off below.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            content,
            style_mask(style),
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: the shell owns the window outside a window controller.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setRestorable(false);
    window.setTabbingMode(NSWindowTabbingMode::Disallowed);
    window.setLevel(window_level(style.level));
    if style.fullscreen {
        window.setCollectionBehavior(NSWindowCollectionBehavior::FullScreenPrimary);
    }
    if style.level == NativeLevel::PopUp {
        window.setAnimationBehavior(NSWindowAnimationBehavior::None);
    }
    let opaque = depth != 32;
    window.setOpaque(opaque);
    let background = if opaque {
        NSColor::blackColor()
    } else {
        NSColor::clearColor()
    };
    window.setBackgroundColor(Some(&background));

    let layer = CAMetalLayer::new();
    layer.setContentsScale(scale);
    layer.setDrawableSize(NSSize::new(f64::from(extent.0), f64::from(extent.1)));
    layer.setOpaque(opaque);
    // SAFETY: immutable CoreAnimation constants.
    unsafe {
        layer.setContentsGravity(kCAGravityTopLeft);
        layer.setMagnificationFilter(kCAFilterNearest);
    }
    // A layer set before `wantsLayer` makes the view layer-hosting.
    let host: &CALayer = &layer;
    view.setLayer(Some(host));
    view.setWantsLayer(true);
    window.setContentView(Some(view));
    let responder: &NSResponder = view;
    let _ = window.makeFirstResponder(Some(responder));
    (window, layer)
}
