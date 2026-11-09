//! Which outputs each surface is on, and the scale and transform it is asked to draw at.
//!
//! **Outputs a window is on.** A window shown on screen is on every output its frame overlaps: each surface of it, its
//! subsurfaces and its popups get `wl_surface.enter` for those outputs and `leave` for the others, so a client that
//! looks at its outputs learns where it is. A window that is not shown (minimized, of a hidden application, on a
//! workspace that is not on screen) is on none. Popups and subsurfaces follow their window: they are on the window's
//! outputs and draw at its scale, wherever they reach. The set is worked out again whenever the window model changes
//! (a window maps, moves, resizes, changes workspace or output) and when an output comes, goes or changes, so an
//! output that goes is left by every surface that was on it. A layer surface is on its own output, and so are its
//! subsurfaces and popups; a lock surface is on the output it covers.
//!
//! **Scale.** Each window draws at the scale of one output, its *scale output*: the output its frame overlaps the
//! most. Near a seam the choice holds still: another output takes over only once its overlap is larger than the
//! current one's by [`SCALE_SWITCH_MARGIN`] of the window's area, or the window leaves the current output, or a move
//! or resize of the window ends (then the largest overlap is taken as it is). A window dragged across a seam so
//! changes scale once, not back and forth. A window that overlaps no output keeps its scale output, else takes the
//! output the window model has it on.
//!
//! The scale goes to the client twice: exactly through `wp_fractional_scale_v1.preferred_scale`, and as a whole number,
//! rounded up so a buffer is never drawn with fewer pixels than the output shows, through
//! `wl_surface.preferred_buffer_scale` (a `wl_surface` of version 6). `preferred_buffer_transform` carries the scale
//! output's transform. A client of an older `wl_compositor` gets neither event, as before.

use std::collections::HashMap;

use cupertino_wm::{OutputId, Rect, WindowId};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::Transform;
use smithay::wayland::compositor::{
    get_parent, send_surface_state, with_states, with_surface_tree_downward, SurfaceData,
    TraversalAction,
};
use smithay::wayland::fractional_scale::with_fractional_scale;

use crate::state::Cupertino;

/// The share of a window's area by which another output's overlap must exceed the current scale output's before the
/// window takes that output's scale.
pub const SCALE_SWITCH_MARGIN: f64 = 0.1;

/// The area of `frame` that lies on `output`.
fn overlap(frame: Rect, output: Rect) -> f64 {
    frame.intersection(&output).map_or(0.0, |r| r.w * r.h)
}

/// The outputs of `outputs` that `frame` overlaps, in their order.
pub fn overlapping(frame: Rect, outputs: &[(OutputId, Rect)]) -> Vec<OutputId> {
    outputs
        .iter()
        .filter(|(_, rect)| overlap(frame, *rect) > 0.0)
        .map(|(id, _)| *id)
        .collect()
}

/// The scale output of a window with frame `frame`, which was `current` (see the module documentation). `settle`: a
/// move or resize of the window just ended, so the largest overlap is taken without the margin. `None` when the frame
/// overlaps no output and `current` is not among `outputs`.
pub fn scale_output(
    current: Option<OutputId>,
    frame: Rect,
    outputs: &[(OutputId, Rect)],
    settle: bool,
) -> Option<OutputId> {
    let current = current.filter(|c| outputs.iter().any(|(id, _)| id == c));
    let current_overlap = current
        .and_then(|c| outputs.iter().find(|(id, _)| *id == c))
        .map_or(0.0, |(_, rect)| overlap(frame, *rect));
    // the first of the largest, so equal overlaps resolve the same way every time
    let best = outputs
        .iter()
        .map(|(id, rect)| (*id, overlap(frame, *rect)))
        .filter(|(_, area)| *area > 0.0)
        .fold(
            None,
            |best: Option<(OutputId, f64)>, (id, area)| match best {
                Some((_, b)) if b >= area => best,
                _ => Some((id, area)),
            },
        );
    let Some((best, best_overlap)) = best else {
        return current;
    };
    let Some(current) = current.filter(|_| current_overlap > 0.0 && !settle) else {
        return Some(best);
    };
    let margin = SCALE_SWITCH_MARGIN * (frame.w * frame.h).max(0.0);
    if best_overlap - current_overlap > margin {
        Some(best)
    } else {
        Some(current)
    }
}

/// The whole-number buffer scale for the fractional `scale`: rounded up, in the 120ths the fractional scale is sent in,
/// so 1.5 asks for 2 and 2 for 2; at least 1.
pub fn buffer_scale(scale: f64) -> i32 {
    let n = (scale * 120.0).round();
    if !n.is_finite() || n <= 120.0 {
        return 1;
    }
    (n / 120.0).ceil().min(f64::from(i32::MAX)) as i32
}

/// Where a window is (see the module documentation), as last told to its surfaces.
#[derive(Clone, Debug, PartialEq)]
struct Placement {
    /// The outputs the window is on.
    entered: Vec<Output>,
    scale_output: Option<OutputId>,
    scale: f64,
    transform: Transform,
    /// A move or resize of the window was in progress.
    moving: bool,
}

/// The outputs and scale of each mapped window, as last told to its surfaces.
#[derive(Debug, Default)]
pub struct SurfaceOutputs {
    windows: HashMap<WindowId, Placement>,
}

impl SurfaceOutputs {
    /// The output whose scale `window` draws at, once it was placed.
    pub fn scale_output(&self, window: WindowId) -> Option<OutputId> {
        self.windows.get(&window).and_then(|p| p.scale_output)
    }
}

/// Tell `surface` that it is on `entered` and on none of `left`, and the scale and transform to draw at.
fn tell(
    surface: &WlSurface,
    states: &SurfaceData,
    entered: &[Output],
    left: &[&Output],
    scale: f64,
    transform: Transform,
) {
    for o in left {
        o.leave(surface);
    }
    for o in entered {
        o.enter(surface);
    }
    with_fractional_scale(states, |fs| fs.set_preferred_scale(scale));
    send_surface_state(surface, states, buffer_scale(scale), transform);
}

/// [`tell`] every surface of the tree of `root`.
fn tell_tree(
    root: &WlSurface,
    entered: &[Output],
    left: &[&Output],
    scale: f64,
    transform: Transform,
) {
    with_surface_tree_downward(
        root,
        (),
        |_, _, _| TraversalAction::DoChildren(()),
        |surface, states, _| tell(surface, states, entered, left, scale, transform),
        |_, _, _| true,
    );
}

/// What a root surface is: its outputs, scale and transform.
struct RootPlacement {
    entered: Vec<Output>,
    scale: f64,
    transform: Transform,
}

impl Cupertino {
    /// The output whose scale `window` draws at (see the module documentation), the output the window model has it on
    /// before it was placed.
    pub fn window_scale_output(&self, window: WindowId) -> Option<OutputId> {
        self.surface_outputs
            .scale_output(window)
            .filter(|o| self.output_entry(*o).is_some())
            .or_else(|| self.wm.window(window).map(|w| w.output))
    }

    /// Work out again which outputs each window is on and the scale it draws at, and tell the surfaces of each window
    /// whose placement changed, with their subsurfaces and popups (all of them with `force`).
    pub fn refresh_window_outputs(&mut self, force: bool) {
        let outputs: Vec<(OutputId, Rect)> = self.outputs.iter().map(|o| (o.id, o.rect)).collect();
        let interacting = self.wm.interacting_window();
        let mut placed = HashMap::with_capacity(self.windows.len());
        for entry in &self.windows {
            let Some(w) = self.wm.window(entry.id) else {
                continue;
            };
            let frame = w.geometry();
            let before = self.surface_outputs.windows.get(&entry.id);
            let moving = interacting == Some(entry.id);
            let settle = before.is_some_and(|b| b.moving && !moving);
            let scale_output =
                scale_output(before.and_then(|b| b.scale_output), frame, &outputs, settle)
                    .or_else(|| Some(w.output).filter(|o| self.output_entry(*o).is_some()));
            let entered = if self.wm.window_on_screen(entry.id) {
                overlapping(frame, &outputs)
                    .into_iter()
                    .filter_map(|id| self.output_entry(id).map(|o| o.output.clone()))
                    .collect()
            } else {
                Vec::new()
            };
            let (scale, transform) = match scale_output.and_then(|o| self.output_entry(o)) {
                Some(o) => (o.scale, o.output.current_transform()),
                None => (self.output_scale(None), Transform::Normal),
            };
            let now = Placement {
                entered,
                scale_output,
                scale,
                transform,
                moving,
            };
            if force || before != Some(&now) {
                let left: Vec<&Output> = before
                    .into_iter()
                    .flat_map(|b| b.entered.iter())
                    .chain(self.outputs.iter().map(|o| &o.output))
                    .filter(|o| !now.entered.contains(o))
                    .collect();
                entry.window.with_surfaces(|surface, states| {
                    tell(
                        surface,
                        states,
                        &now.entered,
                        &left,
                        now.scale,
                        now.transform,
                    )
                });
            }
            placed.insert(entry.id, now);
        }
        self.surface_outputs.windows = placed;
    }

    /// Tell every surface where it is: each window ([`Cupertino::refresh_window_outputs`]), each layer surface and its
    /// popups its output and that output's scale, each lock surface the output it covers.
    pub fn refresh_surface_outputs(&mut self) {
        self.refresh_window_outputs(true);
        for o in &self.outputs {
            let transform = o.output.current_transform();
            let entered = std::slice::from_ref(&o.output);
            let map = smithay::desktop::layer_map_for_output(&o.output);
            for layer in map.layers() {
                tell_tree(layer.wl_surface(), entered, &[], o.scale, transform);
                for (popup, _) in
                    smithay::desktop::PopupManager::popups_for_surface(layer.wl_surface())
                {
                    tell_tree(popup.wl_surface(), entered, &[], o.scale, transform);
                }
            }
            if let Some(surface) = o.lock.wl_surface() {
                tell_tree(surface, entered, &[], o.scale, transform);
            }
        }
    }

    /// A surface committed: a subsurface or popup that is new to its window, layer or lock surface, or a window not
    /// placed yet, learns where it is. A window that is not placed yet is on no output and draws at the scale its window
    /// will open at ([`Cupertino::surface_scale`]).
    pub(crate) fn surface_output_committed(&self, surface: &WlSurface) {
        let Some(root) = self.placement_root(surface) else {
            return;
        };
        match self.root_placement(&root) {
            Some(p) => with_states(surface, |states| {
                tell(surface, states, &p.entered, &[], p.scale, p.transform)
            }),
            None if self.windows.iter().any(|w| w.has_surface(&root)) => {
                let scale = self.surface_scale(surface);
                with_states(surface, |states| {
                    tell(surface, states, &[], &[], scale, Transform::Normal)
                });
            }
            None => {}
        }
    }

    /// The surface whose placement `surface` follows: the root of its subsurface tree, or, for a popup, the root of the
    /// tree its popups hang from.
    fn placement_root(&self, surface: &WlSurface) -> Option<WlSurface> {
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        match self.popups.find_popup(&root) {
            Some(popup) => smithay::desktop::find_popup_root_surface(&popup).ok(),
            None => Some(root),
        }
    }

    /// Where the root surface `root` is: a placed window's placement, a layer surface's or a lock surface's output.
    /// `None` for a window not placed yet and for any other surface.
    fn root_placement(&self, root: &WlSurface) -> Option<RootPlacement> {
        if let Some(entry) = self.windows.iter().find(|w| w.has_surface(root)) {
            let p = self.surface_outputs.windows.get(&entry.id)?;
            return Some(RootPlacement {
                entered: p.entered.clone(),
                scale: p.scale,
                transform: p.transform,
            });
        }
        let o = self.outputs.iter().find(|o| {
            o.lock.wl_surface() == Some(root)
                || smithay::desktop::layer_map_for_output(&o.output)
                    .layer_for_surface(root, smithay::desktop::WindowSurfaceType::TOPLEVEL)
                    .is_some()
        })?;
        Some(RootPlacement {
            entered: vec![o.output.clone()],
            scale: o.scale,
            transform: o.output.current_transform(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT: OutputId = OutputId(1);
    const RIGHT: OutputId = OutputId(2);

    /// Two outputs side by side, 1000 points wide, with their seam at x = 1000.
    fn outputs() -> [(OutputId, Rect); 2] {
        [
            (LEFT, Rect::new(0.0, 0.0, 1000.0, 800.0)),
            (RIGHT, Rect::new(1000.0, 0.0, 1000.0, 800.0)),
        ]
    }

    fn window(x: f64) -> Rect {
        Rect::new(x, 100.0, 400.0, 300.0)
    }

    #[test]
    fn a_window_is_on_every_output_it_overlaps() {
        let outputs = outputs();
        assert_eq!(overlapping(window(100.0), &outputs), vec![LEFT]);
        assert_eq!(overlapping(window(800.0), &outputs), vec![LEFT, RIGHT]);
        assert_eq!(
            overlapping(window(1000.0), &outputs),
            vec![RIGHT],
            "touching the seam is not overlapping it"
        );
        assert_eq!(
            overlapping(window(3000.0), &outputs),
            Vec::<OutputId>::new()
        );
    }

    #[test]
    fn a_new_window_takes_the_output_it_overlaps_most() {
        let outputs = outputs();
        assert_eq!(
            scale_output(None, window(100.0), &outputs, false),
            Some(LEFT)
        );
        assert_eq!(
            scale_output(None, window(850.0), &outputs, false),
            Some(RIGHT)
        );
        // an even split goes to the first output
        assert_eq!(
            scale_output(None, window(800.0), &outputs, false),
            Some(LEFT)
        );
        assert_eq!(scale_output(None, window(3000.0), &outputs, false), None);
    }

    /// Sweep a window across the seam one point at a time, there and back, several times: it changes output once per
    /// crossing, past the middle by the margin, and never back and forth.
    #[test]
    fn a_window_swept_across_a_seam_changes_scale_once_per_crossing() {
        let outputs = outputs();
        let area = 400.0 * 300.0;
        let mut current = scale_output(None, window(500.0), &outputs, false);
        assert_eq!(current, Some(LEFT));
        let mut switches = Vec::new();
        let there: Vec<f64> = (500..=1100).map(f64::from).collect();
        let back: Vec<f64> = there.iter().rev().copied().collect();
        for (pass, xs) in [&there, &back, &there, &back].into_iter().enumerate() {
            for &x in xs {
                let next = scale_output(current, window(x), &outputs, false);
                if next != current {
                    switches.push((pass, x, next));
                    current = next;
                }
            }
        }
        assert_eq!(switches.len(), 4, "{switches:?}");
        for (pass, x, to) in switches {
            let on_right = overlap(window(x), outputs[1].1);
            let on_left = overlap(window(x), outputs[0].1);
            if pass % 2 == 0 {
                assert_eq!(to, Some(RIGHT));
                assert!(on_right - on_left > SCALE_SWITCH_MARGIN * area, "{x}");
            } else {
                assert_eq!(to, Some(LEFT));
                assert!(on_left - on_right > SCALE_SWITCH_MARGIN * area, "{x}");
            }
        }
    }

    /// Jitter around the middle of the seam: the window holds its output whatever the order of the jitter.
    #[test]
    fn jitter_near_the_seam_does_not_flap() {
        let outputs = outputs();
        for start in [LEFT, RIGHT] {
            let mut current = Some(start);
            for i in 0..200 {
                let x = 800.0 + f64::from(i % 7) * 3.0 - 9.0;
                current = scale_output(current, window(x), &outputs, false);
                assert_eq!(current, Some(start), "{x}");
            }
        }
    }

    #[test]
    fn the_end_of_a_move_takes_the_largest_overlap() {
        let outputs = outputs();
        // 53.75% on the right, 7.5% more than on the left: inside the margin, so the left holds while moving
        let frame = window(815.0);
        assert_eq!(scale_output(Some(LEFT), frame, &outputs, false), Some(LEFT));
        assert_eq!(scale_output(Some(LEFT), frame, &outputs, true), Some(RIGHT));
    }

    #[test]
    fn a_window_that_leaves_its_output_or_whose_output_goes_takes_another() {
        let outputs = outputs();
        assert_eq!(
            scale_output(Some(LEFT), window(1200.0), &outputs, false),
            Some(RIGHT)
        );
        assert_eq!(
            scale_output(Some(OutputId(9)), window(100.0), &outputs, false),
            Some(LEFT)
        );
        // off every output, it keeps the output it had
        assert_eq!(
            scale_output(Some(RIGHT), window(3000.0), &outputs, false),
            Some(RIGHT)
        );
    }

    #[test]
    fn the_buffer_scale_is_the_scale_rounded_up() {
        for (scale, whole) in [
            (1.0, 1),
            (1.25, 2),
            (1.5, 2),
            (2.0, 2),
            (2.0 + 1e-9, 2),
            (2.25, 3),
            (3.0, 3),
            (0.5, 1),
            (0.0, 1),
            (f64::NAN, 1),
        ] {
            assert_eq!(buffer_scale(scale), whole, "{scale}");
        }
    }
}
