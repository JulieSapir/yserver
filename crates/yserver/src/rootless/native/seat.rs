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
    /// Motion over `id` at pointer-local `local`: `PointerEnter` on a new
    /// window, else `PointerMotionLocal`.
    pub(crate) fn moved(&mut self, id: ToplevelId, local: (f64, f64), time: u32) -> RootlessInput {
        let origin = pointer_origin();
        self.leave_pending = false;
        if self.inside.replace(id) == Some(id) {
            RootlessInput::PointerMotionLocal {
                origin,
                toplevel: id,
                local,
                time,
            }
        } else {
            RootlessInput::PointerEnter {
                origin,
                toplevel: id,
                local,
                time,
            }
        }
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
