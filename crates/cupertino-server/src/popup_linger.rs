//! Popups the compositor dismissed, still drawn while their clients fade them out. A press outside a popup chain ends
//! its grab, and the dismissed popups leave the popup tree at once, but a client may keep showing a closing menu for a
//! moment, fading it, before it destroys it. Such a popup stays drawn where it was, over its parent, until its client
//! destroys it or [`LINGER_LIMIT`] passes. It is out of the tree, so it takes no input (the pointer and the keyboard
//! find only the tree's popups); it is still called back each frame, so its fade advances.

use std::sync::Mutex;

use smithay::desktop::{PopupKind, PopupManager};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{IsAlive, Logical, Point};
use smithay::wayland::compositor::with_states;

/// Longest a dismissed popup stays drawn, in seconds of model time: a client that never destroys it does not leave it
/// on screen. Longer than any fade a closing popup plays.
pub const LINGER_LIMIT: f64 = 1.0;

/// A dismissed popup: where it lay from its root surface and when it stops being drawn.
#[derive(Clone)]
struct Lingering {
    popup: PopupKind,
    location: Point<i32, Logical>,
    until: f64,
}

/// The dismissed popups of one root surface, kept with the surface.
#[derive(Default)]
struct Dismissed(Mutex<Vec<Lingering>>);

/// The popups of `root` and where they lie, as the popup tree has them now: taken before a grab ends, so the popups it
/// dismisses can be kept by [`keep_dismissed`].
pub fn snapshot(root: &WlSurface) -> Vec<(PopupKind, Point<i32, Logical>)> {
    PopupManager::popups_for_surface(root).collect()
}

/// Keeps drawing the popups of `before` (a [`snapshot`] of `root`) that are alive but no longer in the popup tree, from
/// `now` for up to [`LINGER_LIMIT`].
pub fn keep_dismissed(root: &WlSurface, before: Vec<(PopupKind, Point<i32, Logical>)>, now: f64) {
    if !root.alive() {
        return;
    }
    let still: Vec<WlSurface> = PopupManager::popups_for_surface(root)
        .map(|(p, _)| p.wl_surface().clone())
        .collect();
    let gone: Vec<Lingering> = before
        .into_iter()
        .filter(|(p, _)| p.alive() && !still.contains(p.wl_surface()))
        .map(|(popup, location)| Lingering {
            popup,
            location,
            until: now + LINGER_LIMIT,
        })
        .collect();
    if gone.is_empty() {
        return;
    }
    with_states(root, |states| {
        states
            .data_map
            .insert_if_missing_threadsafe(Dismissed::default);
        if let Some(d) = states.data_map.get::<Dismissed>() {
            let mut list = d.0.lock().unwrap_or_else(|e| e.into_inner());
            list.retain(|l| {
                !gone
                    .iter()
                    .any(|g| g.popup.wl_surface() == l.popup.wl_surface())
            });
            list.extend(gone);
        }
    });
}

/// The dismissed popups of `root` still drawn at `now`, oldest first, with where they lie from it; the ones destroyed or
/// past their limit are forgotten.
pub fn dismissed(root: &WlSurface, now: f64) -> Vec<(PopupKind, Point<i32, Logical>)> {
    if !root.alive() {
        return Vec::new();
    }
    with_states(root, |states| {
        let Some(d) = states.data_map.get::<Dismissed>() else {
            return Vec::new();
        };
        let mut list = d.0.lock().unwrap_or_else(|e| e.into_inner());
        list.retain(|l| l.popup.alive() && now < l.until);
        list.iter().map(|l| (l.popup.clone(), l.location)).collect()
    })
}

/// The popups of `root` to draw at `now`, in the order the scene lists its elements, front first: the popup tree's,
/// then under them the dismissed ones still fading.
pub fn shown(root: &WlSurface, now: f64) -> Vec<(PopupKind, Point<i32, Logical>)> {
    let mut all: Vec<_> = PopupManager::popups_for_surface(root).collect();
    all.extend(dismissed(root, now));
    all
}
