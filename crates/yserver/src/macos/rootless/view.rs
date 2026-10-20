//! A top-level's content view: mouse and scroll input in window-local
//! coordinates (spec §4.4), enter and leave.

use std::rc::Rc;

use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained, runtime::AnyObject,
};
use objc2_app_kit::{NSEvent, NSResponder, NSTrackingArea, NSTrackingAreaOptions, NSView};

use super::shell;
use objc2_foundation::{NSObject, NSRect};
use yserver_core::rootless::ToplevelId;

use crate::macos::input::ShellInput;

pub(super) struct RootlessViewIvars {
    id: ToplevelId,
    input: Rc<ShellInput>,
    /// X pixels per point, the global scale (D2).
    scale: f64,
    /// A popup takes the click that activates the app (§6.2).
    first_mouse: bool,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements beyond the main thread,
    // and the class has no `Drop`.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = RootlessViewIvars]
    pub(super) struct RootlessView;

    impl RootlessView {
        // SAFETY: the signature matches `-[NSResponder acceptsFirstResponder]`.
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        // SAFETY: the signature matches `-[NSView acceptsFirstMouse:]`.
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            self.ivars().first_mouse
        }

        // SAFETY: the signature matches `-[NSResponder mouseMoved:]`.
        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            self.motion(event);
        }

        // SAFETY: the signature matches `-[NSResponder mouseEntered:]`.
        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, event: &NSEvent) {
            let ivars = self.ivars();
            ivars.input.entered_local(ivars.id, self, event, ivars.scale);
        }

        // SAFETY: the signature matches `-[NSResponder mouseExited:]`.
        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            let ivars = self.ivars();
            ivars.input.exited_local(ivars.id);
        }

        // SAFETY: the signature matches `-[NSResponder mouseDragged:]`.
        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.motion(event);
        }

        // SAFETY: the signature matches `-[NSResponder rightMouseDragged:]`.
        #[unsafe(method(rightMouseDragged:))]
        fn right_mouse_dragged(&self, event: &NSEvent) {
            self.motion(event);
        }

        // SAFETY: the signature matches `-[NSResponder otherMouseDragged:]`.
        #[unsafe(method(otherMouseDragged:))]
        fn other_mouse_dragged(&self, event: &NSEvent) {
            self.motion(event);
        }

        // SAFETY: the signature matches `-[NSResponder mouseDown:]`.
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.button(event, true);
        }

        // SAFETY: the signature matches `-[NSResponder mouseUp:]`.
        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            self.button(event, false);
        }

        // SAFETY: the signature matches `-[NSResponder rightMouseDown:]`.
        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) {
            self.button(event, true);
        }

        // SAFETY: the signature matches `-[NSResponder rightMouseUp:]`.
        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, event: &NSEvent) {
            self.button(event, false);
        }

        // SAFETY: the signature matches `-[NSResponder otherMouseDown:]`.
        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, event: &NSEvent) {
            self.button(event, true);
        }

        // SAFETY: the signature matches `-[NSResponder otherMouseUp:]`.
        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, event: &NSEvent) {
            self.button(event, false);
        }

        // SAFETY: the signature matches `-[NSResponder cursorUpdate:]`.
        #[unsafe(method(cursorUpdate:))]
        fn cursor_update(&self, _event: &NSEvent) {
            crate::macos::cursor::apply_current();
        }

        // SAFETY: the signature matches `-[NSResponder scrollWheel:]`.
        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            self.ivars().input.scroll(event);
        }
    }
);

impl RootlessView {
    /// A view of `frame` points for top-level `id`, tracking the pointer
    /// whether or not the window is key.
    pub(super) fn new(
        mtm: MainThreadMarker,
        frame: NSRect,
        id: ToplevelId,
        input: Rc<ShellInput>,
        scale: f64,
        first_mouse: bool,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(RootlessViewIvars {
            id,
            input,
            scale,
            first_mouse,
        });
        // SAFETY: `NSView`'s designated initialiser.
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        view.add_tracking_area(
            NSTrackingAreaOptions::MouseMoved
                | NSTrackingAreaOptions::MouseEnteredAndExited
                | NSTrackingAreaOptions::ActiveAlways
                | NSTrackingAreaOptions::InVisibleRect,
        );
        // `cursorUpdate:` never comes from an `ActiveAlways` area; any of
        // our windows shows the X cursor while the app is active.
        view.add_tracking_area(
            NSTrackingAreaOptions::CursorUpdate
                | NSTrackingAreaOptions::ActiveInActiveApp
                | NSTrackingAreaOptions::InVisibleRect,
        );
        view
    }

    fn add_tracking_area(&self, options: NSTrackingAreaOptions) {
        let owner: &AnyObject = self;
        // SAFETY: `InVisibleRect` areas ignore the rect; the owner is this
        // view, which outlives its tracking areas.
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::ZERO,
                options,
                Some(owner),
                None,
            )
        };
        self.addTrackingArea(&area);
    }

    fn motion(&self, event: &NSEvent) {
        let ivars = self.ivars();
        match self.drag_target(event) {
            Some((under, local)) => ivars.input.dragged_local(under, local),
            None => ivars.input.motion_local(ivars.id, self, event, ivars.scale),
        }
    }

    fn button(&self, event: &NSEvent, pressed: bool) {
        let ivars = self.ivars();
        // A release ends a drag where the pointer is, not where it began.
        let under = if pressed {
            None
        } else {
            self.drag_target(event)
        };
        ivars
            .input
            .button_local(ivars.id, self, event, pressed, ivars.scale, under);
    }

    /// While a button is held, AppKit sends every event to the window that
    /// got the mouse-down: the top-level really under the event's position,
    /// and the position in it (or in the current one, over none of ours).
    /// The event's own position, not the pointer's now: queued motion keeps
    /// its hover transitions.
    fn drag_target(&self, event: &NSEvent) -> Option<(Option<ToplevelId>, (f64, f64))> {
        let (dragging, current) = self.ivars().input.drag_state();
        if !dragging {
            return None;
        }
        let window = event.window(self.mtm()).or_else(|| self.window())?;
        let point = window.convertPointToScreen(event.locationInWindow());
        shell::now(|shell| shell.under_pointer(point, current)).flatten()
    }
}
