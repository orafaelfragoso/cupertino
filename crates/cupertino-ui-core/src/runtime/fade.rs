//! A menu that closes fades out instead of vanishing: a popup on the menu surface, or one that draws a menu's glass
//! itself (a menu with a pointer). The application stops declaring the popup, and the runtime keeps its window, takes
//! it out of the input, and shows it and its glass at a falling opacity until the fade is over; then it destroys it.
//! Only a menu's first level fades, and only when no other menu opens in its place; a submenu that closes while its
//! menu stays open, and a menu that another one replaces, go at once, as does every menu with Reduce Motion.
//! A popover that closes shrinks back into its anchor as it fades ([`super::morph`]), unless another opens in its
//! place or Reduce Motion is on.

use super::{morph, set_background, sync_backdrop, Runtime, WindowState};
use crate::{Application, WindowBackground, WindowKey};
use cupertino_geometry::Point;
use cupertino_ui_platform::{Timestamp, WindowRequest, WindowSystem};
use cupertino_ui_render::{DisplayList, Group, Layer};
use cupertino_ui_theme::Surface;
use std::collections::HashMap;

/// How long a closing menu takes to fade out, in nanoseconds.
const MENU_FADE_NS: u64 = 236_000_000;

/// A popup that is closing: when it began and how much of its opacity it has left; a popover also shrinks into the
/// point of its window it grew from, to `scale` of its size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Fade {
    start: Timestamp,
    pub(super) opacity: f32,
    pub(super) into: Option<Point>,
    pub(super) scale: f64,
}

impl Fade {
    fn ends(&self) -> Timestamp {
        let length = if self.into.is_some() {
            morph::CLOSE_NS
        } else {
            MENU_FADE_NS
        };
        Timestamp(self.start.0.saturating_add(length))
    }
}

/// The opacity a closing menu has `elapsed` nanoseconds into its fade: `(1 - t / T)³`, which falls fast first and
/// settles slowly.
fn opacity_at(elapsed: u64) -> f32 {
    let left = 1.0 - elapsed as f64 / MENU_FADE_NS as f64;
    left.clamp(0.0, 1.0).powi(3) as f32
}

/// Shows the display list at `opacity`, each of its roots as one layer.
pub(super) fn dim(list: &mut DisplayList, opacity: f32) {
    for node in list.nodes.iter_mut().filter(|n| n.parent.is_none()) {
        // its content stays as it was while the opacity falls: the renderer keeps it
        node.layer = node.layer.or(Some(Layer::Opacity));
        node.group = Some(match node.group.take() {
            Some(group) => Group {
                opacity: group.opacity * opacity,
                effects: group.effects,
            },
            None => Group {
                opacity,
                effects: Vec::new(),
            },
        });
    }
}

impl<A: Application, W: WindowSystem> Runtime<A, W> {
    /// Whether `w` is a menu: a popup on the menu surface, or one whose content draws a menu's material itself
    /// ([`crate::Widget::closes_as_menu`]).
    fn is_menu(&self, w: &WindowState<A::Message>) -> bool {
        w.popup.is_some()
            && (matches!(
                w.background,
                WindowBackground::Surface {
                    surface: Surface::Menu,
                    ..
                }
            ) || w
                .root
                .and_then(|root| self.tree.get(root))
                .is_some_and(|n| n.widget.closes_as_menu()))
    }

    /// Keeps the menus among `old`, the windows the view no longer declares, that fade out, and gives back the windows
    /// to close. `reopened` is whether the view opened a menu the last one did not have.
    pub(super) fn fade_closed(
        &mut self,
        old: Vec<WindowState<A::Message>>,
        reopened: bool,
    ) -> Vec<WindowState<A::Message>> {
        let now = self.options.clock.now();
        let mut closing = Vec::new();
        // the keys the closing menus were given, so the menus opened from them find them
        let mut renamed: HashMap<WindowKey, WindowKey> = HashMap::new();
        // parents come before their popups
        for mut w in old {
            let Some(popup) = &w.popup else {
                closing.push(w);
                continue;
            };
            let parent_key = renamed.get(&popup.parent).unwrap_or(&popup.parent).clone();
            let keep = match self.windows.iter().find(|p| p.key == parent_key) {
                None => false,
                Some(_) if w.fade.is_some() => w.fade.is_some_and(|f| now < f.ends()),
                Some(_) if w.grows_from.is_some() => {
                    w.committed && !reopened && !w.env.theme.preferences.reduce_motion
                }
                Some(parent) => {
                    self.is_menu(&w)
                        && w.committed
                        && !reopened
                        && !w.env.theme.preferences.reduce_motion
                        // a submenu goes with its menu, or at once
                        && (!self.is_menu(parent) || parent.fade.is_some())
                }
            };
            if !keep {
                closing.push(w);
                continue;
            }
            if w.fade.is_none() {
                self.begin_fade(&mut w, now, &mut renamed);
            }
            if let Some(popup) = &mut w.popup {
                popup.parent = parent_key;
            }
            self.windows.push(w);
        }
        closing
    }

    /// `w` starts to fade: it takes no more input and no one hears of it any more.
    fn begin_fade(
        &mut self,
        w: &mut WindowState<A::Message>,
        now: Timestamp,
        renamed: &mut HashMap<WindowKey, WindowKey>,
    ) {
        w.fade = Some(Fade {
            start: now,
            opacity: 1.0,
            into: w.grows_from,
            scale: 1.0,
        });
        let _ = self
            .system
            .request(w.id, WindowRequest::SetInputRegion(Some(Vec::new())));
        if let Some(popup) = &w.popup {
            let parent = popup.parent.clone();
            let key = w.key.clone();
            self.forget_placement(&parent, &key);
        }
        if let Some(adapter) = &mut self.options.accessibility {
            adapter.window_closed(w.id);
        }
        // a menu declared under the same key meanwhile is another window
        let key = WindowKey(crate::Key::from(format!("closing-{}", w.id.0)));
        renamed.insert(std::mem::replace(&mut w.key, key.clone()), key);
        w.invalidate_damage();
        w.dirty = true;
    }

    /// Destroys the menus whose fade is over and sets the opacity of the others for the frame to come.
    pub(super) fn advance_fades(&mut self) {
        if self.windows.iter().all(|w| w.fade.is_none()) {
            return;
        }
        let now = self.options.clock.now();
        // the latest first, as the protocol destroys popups
        let mut i = self.windows.len();
        while i > 0 {
            i -= 1;
            if self.windows[i].fade.is_some_and(|f| now >= f.ends()) {
                let w = self.windows.remove(i);
                self.close(w);
            }
        }
        for w in &mut self.windows {
            let Some(fade) = &mut w.fade else { continue };
            let elapsed = now.0.saturating_sub(fade.start.0);
            let opacity = if fade.into.is_some() {
                fade.scale = morph::close_scale(elapsed);
                fade.scale as f32
            } else {
                opacity_at(elapsed)
            };
            fade.opacity = opacity;
            // the material the compositor draws fades with the content; an opaque fill the window paints itself does
            // so as part of it
            if w.backdrop.is_some() {
                if let WindowBackground::Surface {
                    surface, radius, ..
                } = w.background
                {
                    let alpha = f64::from(opacity);
                    set_background(
                        w,
                        WindowBackground::Surface {
                            surface,
                            radius,
                            alpha,
                        },
                    );
                    sync_backdrop(&mut self.system, w);
                }
            }
            w.dirty = true;
            w.invalidate_damage();
        }
    }

    /// When the first closing menu is gone.
    pub(super) fn next_fade_end(&self) -> Option<Timestamp> {
        self.windows
            .iter()
            .filter_map(|w| w.fade.map(|f| f.ends()))
            .min()
    }
}
