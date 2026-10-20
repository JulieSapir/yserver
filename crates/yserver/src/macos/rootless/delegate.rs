//! One delegate per top-level window: key changes to the shell, a main-queue
//! turn later (AppKit calls these inside the shell's own window calls).

use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained};
use objc2_app_kit::{NSWindow, NSWindowDelegate};
use objc2_foundation::{NSNotification, NSObject, NSObjectProtocol, NSRect};
use objc2_quartz_core::CATransaction;
use yserver_core::rootless::ToplevelId;

use super::shell;

pub(super) struct DelegateIvars {
    id: ToplevelId,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and the class has no
    // `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = DelegateIvars]
    pub(super) struct RootlessWindowDelegate;

    // SAFETY: `NSObjectProtocol` has no safety requirements.
    unsafe impl NSObjectProtocol for RootlessWindowDelegate {}

    // SAFETY: `NSWindowDelegate` has no safety requirements.
    unsafe impl NSWindowDelegate for RootlessWindowDelegate {
        // SAFETY: the signature matches the protocol method.
        #[unsafe(method(windowDidBecomeKey:))]
        fn did_become_key(&self, _notification: &NSNotification) {
            let id = self.ivars().id;
            shell::on_main(move |shell| shell.became_key(id));
        }

        // SAFETY: the signature matches the protocol method.
        #[unsafe(method(windowDidResignKey:))]
        fn did_resign_key(&self, _notification: &NSNotification) {
            shell::on_main(shell::RootlessShell::resigned_key);
        }

        // SAFETY: the signature matches the protocol method.
        #[unsafe(method(windowDidMove:))]
        fn did_move(&self, _notification: &NSNotification) {
            self.frame_changed();
        }

        // SAFETY: the signature matches the protocol method.
        #[unsafe(method(windowDidResize:))]
        fn did_resize(&self, notification: &NSNotification) {
            // The hosted layer does not follow the view by itself.
            if let Some(view) = notification
                .object()
                .and_then(|o| o.downcast::<NSWindow>().ok())
                .and_then(|window| window.contentView())
                && let Some(layer) = view.layer()
            {
                CATransaction::begin();
                CATransaction::setDisableActions(true);
                layer.setFrame(view.bounds());
                CATransaction::commit();
            }
            self.frame_changed();
        }

        // SAFETY: the signature matches the protocol method.
        #[unsafe(method(windowShouldClose:))]
        fn should_close(&self, _sender: &NSWindow) -> bool {
            let id = self.ivars().id;
            match shell::now(|shell| shell.close_clicked(id)) {
                Some(Some(ask)) => ask.run(self.mtm()),
                Some(None) => {}
                None => shell::on_main(move |shell| {
                    // Busy now: the kill question waits for the next click.
                    let _ = shell.close_clicked(id);
                }),
            }
            // X closes it, or not.
            false
        }

        // SAFETY: the signature matches the protocol method.
        #[unsafe(method(windowShouldZoom:toFrame:))]
        fn should_zoom(&self, _window: &NSWindow, _frame: NSRect) -> bool {
            // No did-zoom notification: read the state a turn later.
            self.report_state();
            true
        }

        // SAFETY: the signature matches the protocol method.
        #[unsafe(method(windowDidMiniaturize:))]
        fn did_miniaturize(&self, _notification: &NSNotification) {
            self.report_state();
        }

        // SAFETY: the signature matches the protocol method.
        #[unsafe(method(windowDidDeminiaturize:))]
        fn did_deminiaturize(&self, _notification: &NSNotification) {
            self.report_state();
        }

        // SAFETY: the signature matches the protocol method.
        #[unsafe(method(windowDidEnterFullScreen:))]
        fn did_enter_full_screen(&self, _notification: &NSNotification) {
            let id = self.ivars().id;
            shell::on_main(move |shell| shell.set_fullscreen(id, true));
        }

        // SAFETY: the signature matches the protocol method.
        #[unsafe(method(windowDidExitFullScreen:))]
        fn did_exit_full_screen(&self, _notification: &NSNotification) {
            let id = self.ivars().id;
            shell::on_main(move |shell| shell.set_fullscreen(id, false));
        }
    }
);

impl RootlessWindowDelegate {
    fn frame_changed(&self) {
        let id = self.ivars().id;
        shell::on_main(move |shell| shell.frame_changed(id));
    }

    fn report_state(&self) {
        let id = self.ivars().id;
        shell::on_main(move |shell| shell.report_state(id));
    }

    pub(super) fn new(mtm: MainThreadMarker, id: ToplevelId) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars { id });
        // SAFETY: `NSObject`'s `init` has this signature.
        unsafe { msg_send![super(this), init] }
    }
}
