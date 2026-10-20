//! The main thread's half: one `NSWindow` per top-level, from the core's
//! events (spec §4.5.1 macOS row).

use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

use dispatch2::DispatchQueue;
use objc2::{MainThreadMarker, rc::Retained, runtime::ProtocolObject};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSApplication, NSScreen, NSView, NSWindow,
    NSWindowOrderingMode,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, ns_string};
use objc2_quartz_core::CAMetalLayer;
use yserver_core::rootless::{
    Geometry, GeometryFields, HostRequest, HostSerial, InfoFields, RootlessEvent, ToplevelId,
    ToplevelKind, WindowInfo, WmRequest,
};

use super::{LayerTable, delegate::RootlessWindowDelegate, view::RootlessView};
use crate::{
    macos::input::ShellInput,
    rootless::{
        host::FrameEvent,
        native::{
            geometry::{GeometrySync, HostSerials},
            seat::{CloseAction, CloseTracker, FocusTracker, XStacking, restack_requests},
            space::{GlobalSpace, NativeRect, below_title_bar, content_contains},
            style::{NativeFlags, NetStateSync, content_limits, level_for, native_style},
        },
    },
    surface::link::ShellLink,
};

thread_local! {
    static SHELL: RefCell<Option<RootlessShell>> = const { RefCell::new(None) };
}

/// Set up the shell on the main thread.
pub(super) fn install(
    space: GlobalSpace,
    link: Arc<ShellLink>,
    layers: Arc<LayerTable>,
    input: Rc<ShellInput>,
) {
    SHELL.set(Some(RootlessShell {
        space,
        windows: HashMap::new(),
        link,
        layers,
        input,
        focus: FocusTracker::default(),
        serials: HostSerials::default(),
        cascade: NSPoint::new(0.0, 0.0),
        stacking: XStacking::default(),
    }));
}

/// Run `f` on the shell now, unless the shell is busy (AppKit called back
/// from inside one of its window calls).
pub(super) fn now<R>(f: impl FnOnce(&mut RootlessShell) -> R) -> Option<R> {
    SHELL.with(|shell| {
        let mut shell = shell.try_borrow_mut().ok()?;
        shell.as_mut().map(f)
    })
}

/// Run `f` on the shell, on the main queue.
pub(super) fn on_main(f: impl FnOnce(&mut RootlessShell) + Send + 'static) {
    DispatchQueue::main().exec_async(move || {
        SHELL.with_borrow_mut(|shell| {
            if let Some(shell) = shell {
                f(shell);
            }
        });
    });
}

/// One top-level's native window.
struct NativeWindow {
    window: Retained<NSWindow>,
    /// Retained here while the core thread may use it (`LayerTable`).
    _layer: Retained<CAMetalLayer>,
    /// The window holds its delegate weakly.
    _delegate: Retained<RootlessWindowDelegate>,
    kind: ToplevelKind,
    info: WindowInfo,
    border: u16,
    /// X's geometry, against which native changes are reported.
    sync: GeometrySync,
    /// `_NET_WM_STATE` as last reported.
    net: NetStateSync,
    close: CloseTracker,
    /// Between `windowDidEnterFullScreen:` and `windowDidExitFullScreen:`.
    fullscreen: bool,
    /// The transient parent this window is a child window of.
    parent: Option<ToplevelId>,
    /// A Managed window without a transient parent: its first frame
    /// brings the app forward.
    activates: bool,
}

pub(super) struct RootlessShell {
    space: GlobalSpace,
    windows: HashMap<ToplevelId, NativeWindow>,
    link: Arc<ShellLink>,
    layers: Arc<LayerTable>,
    input: Rc<ShellInput>,
    focus: FocusTracker,
    serials: HostSerials,
    /// Where the next unplaced window cascades from.
    cascade: NSPoint,
    /// X's stacking of our windows (§6.4).
    stacking: XStacking,
}

/// A force-kill to confirm (decision 12), asked outside the shell.
pub(super) struct KillAsk {
    id: ToplevelId,
    title: String,
    deletes: bool,
    link: Arc<ShellLink>,
}

impl KillAsk {
    /// Ask the user; on Kill, the only request that kills.
    pub(super) fn run(self, mtm: MainThreadMarker) {
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(&format!(
            "Kill the X client of \"{}\"?",
            self.title
        )));
        let why = if self.deletes {
            "It has not closed the window."
        } else {
            "It does not support closing a window."
        };
        alert.setInformativeText(&NSString::from_str(&format!(
            "{why} Killing it closes all of its windows."
        )));
        alert.addButtonWithTitle(ns_string!("Kill"));
        alert.addButtonWithTitle(ns_string!("Cancel"));
        if alert.runModal() == NSAlertFirstButtonReturn {
            self.link.request(HostRequest::ForceKill { id: self.id });
        }
    }
}

/// X content + border on both sides.
fn outer_extent(g: &Geometry) -> (u16, u16) {
    (
        g.width.saturating_add(g.border.saturating_mul(2)),
        g.height.saturating_add(g.border.saturating_mul(2)),
    )
}

/// `WM_NORMAL_HINTS` as native content limits, in points.
fn apply_limits(window: &NSWindow, info: &WindowInfo, border: u16, scale: f64) {
    let limits = content_limits(&info.size_hints, border);
    let points = |(w, h): (u16, u16)| NSSize::new(f64::from(w) / scale, f64::from(h) / scale);
    window.setContentMinSize(limits.min.map_or(NSSize::new(1.0, 1.0), points));
    let unlimited = f64::from(f32::MAX);
    window.setContentMaxSize(limits.max.map_or(NSSize::new(unlimited, unlimited), points));
    window.setContentResizeIncrements(limits.inc.map_or(NSSize::new(1.0, 1.0), points));
}

/// `rect` as a native rect.
fn native(rect: NSRect) -> NativeRect {
    NativeRect {
        x: rect.origin.x,
        y: rect.origin.y,
        width: rect.size.width,
        height: rect.size.height,
    }
}

/// `rect` as an `NSRect`.
fn ns(rect: NativeRect) -> NSRect {
    NSRect::new(
        NSPoint::new(rect.x, rect.y),
        NSSize::new(rect.width, rect.height),
    )
}

/// The area below the menu bar of the display holding `rect`'s top-left
/// corner (the main display when none does).
fn visible_frame(mtm: MainThreadMarker, rect: NativeRect) -> Option<NativeRect> {
    let (x, top) = (rect.x, rect.y + rect.height);
    NSScreen::screens(mtm)
        .iter()
        .find(|screen| {
            let f = screen.frame();
            (f.origin.x..f.origin.x + f.size.width).contains(&x)
                && (f.origin.y..=f.origin.y + f.size.height).contains(&top)
        })
        .or_else(|| NSScreen::mainScreen(mtm))
        .map(|screen| native(screen.visibleFrame()))
}

impl RootlessShell {
    pub(super) fn apply(&mut self, events: &[RootlessEvent]) {
        for event in events {
            match event {
                RootlessEvent::Mapped {
                    id,
                    kind,
                    geometry: g,
                    depth,
                    info,
                    above,
                    ..
                } => {
                    self.stacking.place(*id, *above);
                    self.mapped(*id, *kind, g, *depth, info);
                }
                // `surface_released` frees it.
                RootlessEvent::Unmapped { id } => {
                    self.detach(*id);
                    self.stacking.remove(*id);
                    if let Some(w) = self.windows.get(id) {
                        w.window.orderOut(None);
                    }
                }
                RootlessEvent::Restacked {
                    id,
                    above,
                    from_client,
                    ..
                } => {
                    self.stacking.place(*id, *above);
                    if *from_client {
                        self.order_natively(*id, *above);
                    }
                }
                RootlessEvent::InfoChanged { id, info, changed } => {
                    self.info_changed(*id, info, *changed);
                }
                RootlessEvent::Configured {
                    id,
                    geometry,
                    acked,
                    client_fields,
                } => self.configured(*id, geometry, *acked, *client_fields),
                RootlessEvent::WmRequest { id, request } => self.wm_request(*id, *request),
                // A popup's level keeps it on top; no child relation.
                _ => {}
            }
        }
    }

    /// Create the window invisible and set up; the first frame shows it.
    fn mapped(
        &mut self,
        id: ToplevelId,
        kind: ToplevelKind,
        g: &yserver_core::rootless::Geometry,
        depth: u8,
        info: &yserver_core::rootless::WindowInfo,
    ) {
        let mtm = MainThreadMarker::new().expect("the shell runs on the main thread");
        let extent = outer_extent(g);
        let content = ns(self.space.to_native((g.x, g.y), extent));
        let style = native_style(kind, info);
        let view = RootlessView::new(
            mtm,
            NSRect::new(NSPoint::new(0.0, 0.0), content.size),
            id,
            Rc::clone(&self.input),
            self.space.scale(),
            kind != ToplevelKind::Managed,
        );
        let host: &NSView = &view;
        let (window, layer) = super::window::create(
            mtm,
            &style,
            content,
            self.space.scale(),
            depth,
            extent,
            host,
        );
        let delegate = RootlessWindowDelegate::new(mtm, id);
        window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        if style.titled {
            window.setTitle(&NSString::from_str(&info.title));
            // Spec §4.0: the content is at the X position, the title bar
            // above it, kept reachable (XQuartz).
            let title_bar =
                window.frameRectForContentRect(content).size.height - content.size.height;
            if let Some(visible) = visible_frame(mtm, native(content)) {
                let placed = ns(below_title_bar(native(content), title_bar, visible));
                if placed != content {
                    window.setFrame_display(window.frameRectForContentRect(placed), false);
                }
            }
            apply_limits(&window, info, g.border, self.space.scale());
            // §6.1: the host places a window nobody positioned.
            let hints = &info.size_hints;
            if kind == ToplevelKind::Managed
                && !hints.us_position
                && !hints.p_position
                && info.transient_for.is_none()
            {
                self.cascade = window.cascadeTopLeftFromPoint(self.cascade);
            }
        }
        // Mapped invisibly: a present to an ordered-out window may not
        // acquire (spec §4.5.1).
        window.setAlphaValue(0.0);
        window.setIgnoresMouseEvents(true);
        window.orderFront(None);
        // Where the content really is (ours above, or the system's own
        // constraint): X follows it.
        let mut sync = GeometrySync::mapped(g);
        let (origin, actual) = self
            .space
            .to_x(native(window.contentRectForFrameRect(window.frame())));
        for request in sync.native_changed(id, origin, actual, &mut self.serials) {
            log::debug!("macos: {id:?} placed natively: {request:?}");
            self.link.request(request);
        }
        self.layers.insert(id, &layer);
        self.windows.insert(
            id,
            NativeWindow {
                window,
                _layer: layer,
                _delegate: delegate,
                kind,
                info: info.clone(),
                border: g.border,
                sync,
                net: NetStateSync::mapped(info.state),
                close: CloseTracker::default(),
                fullscreen: false,
                parent: None,
                activates: kind == ToplevelKind::Managed && info.transient_for.is_none(),
            },
        );
        if let Some(parent) = style.child_of {
            self.attach(id, parent);
        }
        self.link.post_frame(FrameEvent::SetupDone { id });
    }

    /// Make `id` a child window of its transient parent: it stays above it
    /// and moves with it (D9).
    fn attach(&mut self, id: ToplevelId, parent: ToplevelId) {
        let Some(parent_window) = self.windows.get(&parent).map(|w| w.window.clone()) else {
            return;
        };
        let Some(w) = self.windows.get_mut(&id) else {
            return;
        };
        // SAFETY: both windows are ours and live; `detach` undoes it before
        // either goes.
        unsafe { parent_window.addChildWindow_ordered(&w.window, NSWindowOrderingMode::Above) };
        w.parent = Some(parent);
    }

    /// Undo every child relation of `id`, as child and as parent.
    fn detach(&mut self, id: ToplevelId) {
        let Some(window) = self.windows.get(&id).map(|w| w.window.clone()) else {
            return;
        };
        if let Some(parent) = window.parentWindow() {
            parent.removeChildWindow(&window);
        }
        for w in self.windows.values_mut() {
            if w.parent == Some(id) {
                window.removeChildWindow(&w.window);
                w.parent = None;
            }
        }
        if let Some(w) = self.windows.get_mut(&id) {
            w.parent = None;
        }
    }

    /// X restacked `id` directly above `above` (`None`: the bottom of ours).
    fn order_natively(&self, id: ToplevelId, above: Option<ToplevelId>) {
        let Some(w) = self.windows.get(&id) else {
            return;
        };
        match above.and_then(|a| self.windows.get(&a)) {
            Some(other) => w
                .window
                .orderWindow_relativeTo(NSWindowOrderingMode::Above, other.window.windowNumber()),
            None => {
                let lowest = self
                    .stacking
                    .bottom_to_top()
                    .iter()
                    .filter(|&&other| other != id)
                    .find_map(|other| self.windows.get(other));
                if let Some(lowest) = lowest {
                    w.window.orderWindow_relativeTo(
                        NSWindowOrderingMode::Below,
                        lowest.window.windowNumber(),
                    );
                }
            }
        }
    }

    /// The native order of our Managed windows is authoritative (§6.4):
    /// restack X to match.
    pub(super) fn sync_stacking(&mut self) {
        let mtm = MainThreadMarker::new().expect("the shell runs on the main thread");
        let managed = |w: &NativeWindow| w.kind == ToplevelKind::Managed;
        let native: Vec<ToplevelId> = NSApplication::sharedApplication(mtm)
            .orderedWindows()
            .iter()
            .filter_map(|window| {
                self.windows
                    .iter()
                    .find(|(_, w)| {
                        managed(w) && Retained::as_ptr(&w.window) == Retained::as_ptr(&window)
                    })
                    .map(|(id, _)| *id)
            })
            .collect();
        let x: Vec<ToplevelId> = self
            .stacking
            .bottom_to_top()
            .iter()
            .copied()
            .filter(|id| self.windows.get(id).is_some_and(managed))
            .collect();
        for (id, above) in restack_requests(&native, &x) {
            self.stacking.place(id, above);
            let serial = self.serials.next();
            log::debug!("macos: restack {id:?} above {above:?}");
            self.link
                .request(HostRequest::Restacked { id, above, serial });
        }
    }

    /// The first frame is on screen: show the window. A new Managed window
    /// without a transient parent brings the app forward, as `XQuartz`
    /// does; transients and popups follow their app.
    pub(super) fn first_frame(&mut self, id: ToplevelId) {
        let Some(w) = self.windows.get(&id) else {
            return;
        };
        let mtm = MainThreadMarker::new().expect("the shell runs on the main thread");
        let app = NSApplication::sharedApplication(mtm);
        w.window.setAlphaValue(1.0);
        w.window.setIgnoresMouseEvents(false);
        if w.kind != ToplevelKind::Managed {
            return;
        }
        // The state the client asked for before the map.
        let state = w.info.state;
        if state.fullscreen && native_style(w.kind, &w.info).fullscreen {
            w.window.toggleFullScreen(None);
        } else if state.maximized_vert && state.maximized_horz && !w.window.isZoomed() {
            w.window.zoom(None);
            on_main(move |shell| shell.report_state(id));
        }
        if w.activates && !app.isActive() {
            log::debug!("macos: activating for a new window {id:?}");
            // macOS 14+ may ignore `activate` unless the active app yields
            // (cooperative activation); the older call is still honoured.
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
            app.activate();
            // In front even while activation is pending or refused.
            w.window.orderFrontRegardless();
        }
        w.window.makeKeyAndOrderFront(None);
    }

    fn info_changed(&mut self, id: ToplevelId, info: &WindowInfo, changed: InfoFields) {
        let scale = self.space.scale();
        let Some(w) = self.windows.get_mut(&id) else {
            return;
        };
        if changed.contains(InfoFields::TITLE) {
            w.window.setTitle(&NSString::from_str(&info.title));
        }
        if changed.contains(InfoFields::SIZE_HINTS) {
            apply_limits(&w.window, info, w.border, scale);
        }
        if changed.contains(InfoFields::STATE) {
            w.net.client_state(info.state);
        }
        w.info = info.clone();
        let (kind, old_parent) = (w.kind, w.parent);
        if changed.contains(InfoFields::TRANSIENT_FOR) {
            let parent = native_style(kind, info).child_of;
            if parent != old_parent {
                if let Some(old) = old_parent
                    && let (Some(p), Some(c)) = (
                        self.windows.get(&old).map(|w| w.window.clone()),
                        self.windows.get(&id).map(|w| w.window.clone()),
                    )
                {
                    p.removeChildWindow(&c);
                }
                if let Some(w) = self.windows.get_mut(&id) {
                    w.parent = None;
                }
                if let Some(parent) = parent {
                    self.attach(id, parent);
                }
            }
        }
    }

    /// §4.2.1 rule 4: apply exactly the client's fields natively.
    fn configured(
        &mut self,
        id: ToplevelId,
        g: &Geometry,
        acked: Option<HostSerial>,
        client_fields: GeometryFields,
    ) {
        let mtm = MainThreadMarker::new().expect("the shell runs on the main thread");
        let space = self.space;
        let Some(w) = self.windows.get_mut(&id) else {
            return;
        };
        w.border = g.border;
        if w.sync.configured(g, acked, client_fields).is_none() {
            return;
        }
        let (origin, extent) = w.sync.x();
        let mut content = space.to_native(origin, extent);
        if native_style(w.kind, &w.info).titled {
            let title_bar =
                w.window.frameRectForContentRect(ns(content)).size.height - content.height;
            if let Some(visible) = visible_frame(mtm, content) {
                content = below_title_bar(content, title_bar, visible);
            }
        }
        log::debug!("macos: {id:?} configured by X to {origin:?} {extent:?}");
        // The move notification reports any difference (title bar, the
        // system's constraint) back to X.
        w.window
            .setFrame_display(w.window.frameRectForContentRect(ns(content)), true);
    }

    /// The native frame changed (a drag, a resize, a zoom, our own
    /// `setFrame`): X follows; equal to what X has is an echo.
    pub(super) fn frame_changed(&mut self, id: ToplevelId) {
        let space = self.space;
        let Some(w) = self.windows.get_mut(&id) else {
            return;
        };
        let content = w.window.contentRectForFrameRect(w.window.frame());
        let (origin, extent) = space.to_x(native(content));
        for request in w.sync.native_changed(id, origin, extent, &mut self.serials) {
            log::debug!("macos: {id:?} moved natively: {request:?}");
            self.link.request(request);
        }
        self.report_state(id);
        // Child windows move with their parent, maybe without a notification.
        let children: Vec<ToplevelId> = self
            .windows
            .iter()
            .filter(|(_, w)| w.parent == Some(id))
            .map(|(child, _)| *child)
            .collect();
        for child in children {
            self.frame_changed(child);
        }
    }

    /// Read the window's state and report it when it differs (Task 6).
    pub(super) fn report_state(&mut self, id: ToplevelId) {
        let Some(w) = self.windows.get_mut(&id) else {
            return;
        };
        let flags = NativeFlags {
            // A native zoom has no single axis; fullscreen is not a zoom.
            zoomed: !w.fullscreen && w.window.isZoomed(),
            fullscreen: w.fullscreen,
            minimized: w.window.isMiniaturized(),
            level: super::window::level_of(w.window.level()),
        };
        if let Some(request) = w.net.native(id, flags) {
            log::debug!("macos: {id:?} state {request:?}");
            self.link.request(request);
        }
    }

    pub(super) fn set_fullscreen(&mut self, id: ToplevelId, fullscreen: bool) {
        if let Some(w) = self.windows.get_mut(&id) {
            w.fullscreen = fullscreen;
        }
        self.report_state(id);
    }

    /// The close button: ask the client, the second time offer to kill it
    /// (decision 12); the kill is asked for outside the shell.
    pub(super) fn close_clicked(&mut self, id: ToplevelId) -> Option<KillAsk> {
        let w = self.windows.get_mut(&id)?;
        match w.close.clicked() {
            CloseAction::Request => {
                self.link.request(HostRequest::CloseRequested { id });
                None
            }
            CloseAction::ConfirmKill => Some(KillAsk {
                id,
                title: w.info.title.clone(),
                deletes: w.info.protocols.delete_window,
                link: Arc::clone(&self.link),
            }),
        }
    }

    /// A client asked the WM (§6.3); every state change comes back through
    /// `report_state`, never optimistically.
    fn wm_request(&mut self, id: ToplevelId, request: WmRequest) {
        let Some(w) = self.windows.get(&id) else {
            return;
        };
        log::debug!("macos: {id:?} asks {request:?}");
        match request {
            WmRequest::Activate => w.window.makeKeyAndOrderFront(None),
            // As the close button's first click; never a kill.
            WmRequest::Close => self.link.request(HostRequest::CloseRequested { id }),
            WmRequest::Minimize => w.window.miniaturize(None),
            WmRequest::SetFullscreen(on) => {
                if on != w.fullscreen {
                    w.window.toggleFullScreen(None);
                }
            }
            WmRequest::SetMaximized(on) => {
                if on != w.window.isZoomed() {
                    w.window.zoom(None);
                }
                on_main(move |shell| shell.report_state(id));
            }
            WmRequest::SetLayer(layer) => {
                w.window
                    .setLevel(super::window::window_level(level_for(layer)));
                self.report_state(id);
            }
            WmRequest::BeginMove { button } if button != 0 => match self.input.held_down(id) {
                Some(event) => w.window.performWindowDragWithEvent(&event),
                None => log::debug!("macos: {id:?} move without a held button, dropped"),
            },
            // A keyboard move, and every resize: AppKit has no public call
            // that starts them (D12, follow-up F8).
            WmRequest::BeginMove { .. } | WmRequest::BeginResize { .. } => {
                log::debug!("macos: {id:?} {request:?} not supported, dropped");
            }
        }
    }

    /// Our top-level whose X content is under the screen point `point` (the
    /// window a click would hit; its title bar is none of X's), and `point`
    /// in X pixels of it, or of `current` when it is none of ours.
    pub(super) fn under_pointer(
        &self,
        point: NSPoint,
        current: Option<ToplevelId>,
    ) -> Option<(Option<ToplevelId>, (f64, f64))> {
        let mtm = MainThreadMarker::new().expect("the shell runs on the main thread");
        let number = NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(point, 0, mtm);
        let under = self
            .windows
            .iter()
            .find(|(_, w)| {
                w.window.windowNumber() == number
                    && content_contains(
                        native(w.window.contentRectForFrameRect(w.window.frame())),
                        (point.x, point.y),
                    )
            })
            .map(|(id, _)| *id);
        let target = self.windows.get(&under.or(current)?)?;
        let content = target.window.contentRectForFrameRect(target.window.frame());
        let scale = self.space.scale();
        Some((
            under,
            (
                (point.x - content.origin.x) * scale,
                (content.origin.y + content.size.height - point.y) * scale,
            ),
        ))
    }

    /// The core's cursor, at the global scale.
    pub(super) fn set_cursor(&mut self, image: Option<crate::surface::ui::CursorImage>) {
        let inside = self.input.pointer_inside();
        crate::macos::cursor::set_global(image, self.space.scale(), inside);
    }

    /// The core warped the pointer to root pixel `(x, y)`.
    pub(super) fn warp(&mut self, x: i32, y: i32) {
        let point = self.space.to_warp_point((x, y));
        log::debug!("macos: warp to root {x},{y} = quartz {point:?}");
        crate::macos::cursor::warp_global(point);
    }

    /// Top-level `id`'s window became key (one main-queue turn later).
    pub(super) fn became_key(&mut self, id: ToplevelId) {
        if !self.windows.contains_key(&id) {
            return;
        }
        if let Some(request) = self.focus.became_key(id) {
            self.link.request(request);
        }
        self.input.focus_gained();
        self.sync_stacking();
    }

    /// A window resigned key: decide on the next turn, when the new key
    /// window (ours or none) is known.
    pub(super) fn resigned_key(&mut self) {
        self.focus.resigned_key();
        on_main(Self::settle);
    }

    fn settle(&mut self) {
        let mtm = MainThreadMarker::new().expect("the shell runs on the main thread");
        let key = NSApplication::sharedApplication(mtm).keyWindow();
        let ours = key.and_then(|key| {
            self.windows
                .iter()
                .find(|(_, w)| Retained::as_ptr(&w.window) == Retained::as_ptr(&key))
                .map(|(id, _)| *id)
        });
        if let Some(request) = self.focus.settle(ours) {
            self.link.request(request);
        }
    }

    /// Another application became active: X loses focus, keys are released.
    pub(super) fn app_deactivated(&mut self) {
        if let Some(request) = self.focus.app_deactivated() {
            self.link.request(request);
        }
        self.input.focus_lost();
    }

    /// The core is done with `id`'s surface: free the window.
    pub(super) fn release(&mut self, id: ToplevelId) {
        if let Some(w) = self.windows.remove(&id) {
            w.window.orderOut(None);
            w.window.close();
            w.window.setDelegate(None);
        }
    }
}
