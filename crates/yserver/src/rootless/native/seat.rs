//! Focus, pointer, close and stacking state of a host's native windows
//! (spec §4.4, §6.2, §6.4).

use yserver_core::rootless::{HostRequest, RootlessInput, ToplevelId};

use crate::surface::input::devices::pointer_origin;

/// Native key window → `Activated`/`Deactivated` (§6.2): focus moves
/// between our windows without a `Deactivated` in between (D8).
#[derive(Debug, Default)]
pub(crate) struct FocusTracker {
    active: Option<ToplevelId>,
    resign_pending: bool,
}

impl FocusTracker {
    /// `id` became key: `Activated`, once.
    pub(crate) fn became_key(&mut self, id: ToplevelId) -> Option<HostRequest> {
        self.resign_pending = false;
        self.activate(id)
    }

    fn activate(&mut self, id: ToplevelId) -> Option<HostRequest> {
        (self.active.replace(id) != Some(id)).then_some(HostRequest::Activated { id })
    }

    /// The key window resigned; only marks, [`Self::settle`] decides.
    pub(crate) fn resigned_key(&mut self) {
        self.resign_pending = true;
    }

    /// After the key change settled (the next main-queue turn): `key` = our
    /// window that is key now, if any.
    pub(crate) fn settle(&mut self, key: Option<ToplevelId>) -> Option<HostRequest> {
        if !std::mem::take(&mut self.resign_pending) {
            return None;
        }
        match key {
            Some(id) => self.activate(id),
            None => self.deactivate(),
        }
    }

    fn deactivate(&mut self) -> Option<HostRequest> {
        self.active.take().map(|_| HostRequest::Deactivated)
    }

    /// Another application became active.
    pub(crate) fn app_deactivated(&mut self) -> Option<HostRequest> {
        self.resign_pending = false;
        self.deactivate()
    }
}

/// Native enter/exit/motion/buttons → local pointer input (§4.4).
#[derive(Debug, Default)]
pub(crate) struct PointerTracker {
    inside: Option<ToplevelId>,
    buttons: u8,
    leave_pending: bool,
}

impl PointerTracker {
    /// Motion over `id` at pointer-local `local` (outside it during a drag):
    /// `PointerEnter` on a new window, else `PointerMotionLocal`.
    pub(crate) fn moved(&mut self, id: ToplevelId, local: (f64, f64), time: u32) -> RootlessInput {
        let origin = pointer_origin();
        // Drag motion after the exit stays the grab window's motion and
        // keeps the deferred leave; only `entered` cancels it.
        if self.inside.replace(id) == Some(id) {
            RootlessInput::PointerMotionLocal {
                origin,
                toplevel: id,
                local,
                time,
            }
        } else {
            self.leave_pending = false;
            RootlessInput::PointerEnter {
                origin,
                toplevel: id,
                local,
                time,
            }
        }
    }

    /// The pointer entered `id`'s view; cancels a leave deferred by a held
    /// button when it comes back to the same window.
    pub(crate) fn entered(
        &mut self,
        id: ToplevelId,
        local: (f64, f64),
        time: u32,
    ) -> RootlessInput {
        if self.inside == Some(id) {
            self.leave_pending = false;
        }
        self.moved(id, local, time)
    }

    /// Motion while a button is held, at the top-level the window system
    /// says is under the pointer (`under`, `local` in its coordinates), not
    /// the one that got the mouse-down, so a drag crosses into a popup or
    /// another window as on Xorg (the core's grabs route the events).
    /// `None`: over none of ours, `local` relative to the current window;
    /// the leave then waits for the release.
    pub(crate) fn dragged(
        &mut self,
        under: Option<ToplevelId>,
        local: (f64, f64),
        time: u32,
    ) -> Option<RootlessInput> {
        match under {
            Some(id) => {
                self.leave_pending = false;
                Some(self.moved(id, local, time))
            }
            None => {
                let id = self.inside?;
                self.leave_pending = true;
                Some(self.moved(id, local, time))
            }
        }
    }

    /// A button is held.
    pub(crate) fn dragging(&self) -> bool {
        self.buttons > 0
    }

    /// The pointer left `id`. `PointerLeave` releases buttons in the core,
    /// so it waits for the last button release.
    pub(crate) fn exited(&mut self, id: ToplevelId, time: u32) -> Option<RootlessInput> {
        if self.inside != Some(id) {
            return None;
        }
        if self.buttons > 0 {
            self.leave_pending = true;
            return None;
        }
        self.leave(time)
    }

    fn leave(&mut self, time: u32) -> Option<RootlessInput> {
        self.leave_pending = false;
        self.inside.take().map(|_| RootlessInput::PointerLeave {
            origin: pointer_origin(),
            time,
        })
    }

    /// A button went down or up; the last release flushes a deferred leave.
    pub(crate) fn button(&mut self, pressed: bool, time: u32) -> Option<RootlessInput> {
        if pressed {
            self.buttons = self.buttons.saturating_add(1);
            return None;
        }
        self.buttons = self.buttons.saturating_sub(1);
        if self.buttons == 0 && self.leave_pending {
            return self.leave(time);
        }
        None
    }

    pub(crate) fn inside(&self) -> Option<ToplevelId> {
        self.inside
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CloseAction {
    /// Post `CloseRequested`.
    Request,
    /// Ask the user, then `ForceKill` (decision 12).
    ConfirmKill,
}

/// Close-button clicks of one mapped lifetime.
#[derive(Debug, Default)]
pub(crate) struct CloseTracker {
    asked: bool,
}

impl CloseTracker {
    pub(crate) fn clicked(&mut self) -> CloseAction {
        if std::mem::replace(&mut self.asked, true) {
            CloseAction::ConfirmKill
        } else {
            CloseAction::Request
        }
    }
}

/// X's stacking of our top-levels, bottom to top, from `Mapped.above` and
/// `Restacked` (spec §4.2: `above` = directly above it, `None` = bottom).
#[derive(Debug, Default)]
pub(crate) struct XStacking(Vec<ToplevelId>);

impl XStacking {
    /// `id` now sits directly above `above` (`None`: at the bottom).
    pub(crate) fn place(&mut self, id: ToplevelId, above: Option<ToplevelId>) {
        self.remove(id);
        let at = match above {
            None => 0,
            // A sibling not (yet) known: on top.
            Some(below) => self
                .0
                .iter()
                .position(|&w| w == below)
                .map_or(self.0.len(), |i| i + 1),
        };
        self.0.insert(at, id);
    }

    pub(crate) fn remove(&mut self, id: ToplevelId) {
        self.0.retain(|&w| w != id);
    }

    pub(crate) fn bottom_to_top(&self) -> &[ToplevelId] {
        &self.0
    }
}

/// Native front-to-back vs X bottom-to-top → `(id, above)` restacks that
/// make X match (§6.4); `above` `None` = the bottom of ours. Windows only
/// one side knows are left out.
pub(crate) fn restack_requests(
    native_front_to_back: &[ToplevelId],
    x_bottom_to_top: &[ToplevelId],
) -> Vec<(ToplevelId, Option<ToplevelId>)> {
    let target: Vec<ToplevelId> = native_front_to_back
        .iter()
        .rev()
        .copied()
        .filter(|id| x_bottom_to_top.contains(id))
        .collect();
    let mut model: Vec<ToplevelId> = x_bottom_to_top
        .iter()
        .copied()
        .filter(|id| target.contains(id))
        .collect();
    let mut restacks = Vec::new();
    for (i, &id) in target.iter().enumerate() {
        if model[i] == id {
            continue;
        }
        let above = i.checked_sub(1).map(|j| target[j]);
        model.retain(|&w| w != id);
        model.insert(i, id);
        restacks.push((id, above));
    }
    restacks
}
