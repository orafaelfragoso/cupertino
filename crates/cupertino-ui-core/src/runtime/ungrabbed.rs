//! Popups the window system could not open as asked. A popup that wants a grab opens without one when the window system
//! has had no input event for the grab to answer and no desktop-control action that brought one
//! ([`WindowSystem::can_grab`]), as for a menu an assistive action opens before anyone touched the application; the
//! runtime then does what the grab would have. A popup the window system refuses outright stays closed: its application
//! hears it dismissed, and the runtime goes on.

use super::{Runtime, WindowState};
use crate::{Application, UiError, WindowKey};
use cupertino_semantics::Outcome;
use cupertino_ui_platform::{
    PlatformError, PointerEvent, TouchEvent, WindowEvent, WindowId, WindowSystem,
};

#[cfg(test)]
#[path = "ungrabbed-tests.rs"]
mod tests;

/// Whether `w` is a popup that wanted a grab and opened without one.
fn ungrabbed<M>(w: &WindowState<M>) -> bool {
    w.popup.as_ref().is_some_and(|p| p.ungrabbed)
}

impl<A: Application, W: WindowSystem> Runtime<A, W> {
    /// Whether a popup that `wants` a grab takes one, opened from `parent`: only while the window system can grab, and
    /// only from a window or a popup that holds a grab itself, as the protocol allows a grab nowhere else.
    pub(super) fn popup_grabs(&self, wants: bool, parent: &WindowState<A::Message>) -> bool {
        wants
            && self.system.can_grab(parent.id)
            && parent.popup.as_ref().is_none_or(|p| p.placed.grab)
    }

    /// The window system refused popup `key` of window `parent` with `error`. A lost connection still ends the
    /// application; anything else is reported against the parent, the application hears the popup dismissed, and the
    /// popup stays closed until the view stops declaring it, so a model that keeps it does not ask again every turn. An
    /// assistive action whose answer is still to be sent did not get what it asked for: it is answered as unavailable.
    pub(super) fn popup_refused(
        &mut self,
        key: WindowKey,
        parent: WindowId,
        on_dismiss: A::Message,
        error: PlatformError,
    ) -> Result<(), UiError> {
        if let PlatformError::Disconnected(_) = error {
            return Err(UiError::Platform(error));
        }
        let error = UiError::Platform(error);
        if let Some(report) = &mut self.options.on_window_error {
            report(parent, &error);
        }
        if !self.refused_popups.contains(&key) {
            self.refused_popups.push(key);
        }
        self.messages.push_back(on_dismiss);
        for (_, _, outcome) in &mut self.semantic_finishing {
            if *outcome == Outcome::Applied {
                *outcome = Outcome::Unavailable;
            }
        }
        Ok(())
    }

    /// Whether window `j` is the window keyed `key` or a popup opened from it, however deep.
    fn opened_from(&self, j: usize, key: &WindowKey) -> bool {
        let mut at = &self.windows[j];
        // each popup opens after its parent, so the chain is never longer than the windows open
        for _ in 0..self.windows.len() {
            if &at.key == key {
                return true;
            }
            let Some(popup) = &at.popup else {
                return false;
            };
            match self.windows.iter().find(|w| w.key == popup.parent) {
                Some(parent) => at = parent,
                None => return false,
            }
        }
        false
    }

    /// Sends the dismissal of each popup in `popups`, as a compositor ending a grab tells each popup of it.
    fn dismiss_ungrabbed(&mut self, popups: Vec<usize>) {
        for j in popups {
            if let Some(popup) = &self.windows[j].popup {
                self.messages.push_back(popup.on_dismiss.clone());
            }
        }
    }

    /// Window `i` heard `event` while popups that wanted a grab are open without one; does what the grab would have and
    /// says which window hears the event, `None` for none. A press or a touch outside every such popup closes them all
    /// and goes no further. A key goes to the latest such popup opened from window `i`. The keyboard leaving window `i`
    /// closes the ones opened from it.
    pub(super) fn ungrabbed_input(&mut self, i: usize, event: &WindowEvent) -> Option<usize> {
        let open: Vec<usize> = (0..self.windows.len())
            .filter(|&j| ungrabbed(&self.windows[j]))
            .collect();
        if open.is_empty() {
            return Some(i);
        }
        match event {
            WindowEvent::Pointer(PointerEvent::Button { pressed: true, .. }, _)
            | WindowEvent::Touch(TouchEvent::Down { .. }, _) => {
                let inside = open
                    .iter()
                    .any(|&j| self.opened_from(i, &self.windows[j].key));
                if inside {
                    return Some(i);
                }
                self.dismiss_ungrabbed(open);
                None
            }
            WindowEvent::Key(..) => {
                let key = &self.windows[i].key;
                let latest = open.into_iter().rev().find(|&j| self.opened_from(j, key));
                Some(latest.unwrap_or(i))
            }
            WindowEvent::Focus(false) => {
                let key = &self.windows[i].key;
                let from_here: Vec<usize> = open
                    .into_iter()
                    .filter(|&j| j != i && self.opened_from(j, key))
                    .collect();
                self.dismiss_ungrabbed(from_here);
                Some(i)
            }
            _ => Some(i),
        }
    }
}
