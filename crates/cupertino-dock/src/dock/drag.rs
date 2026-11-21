//! Dragging a tile along the bar: the press that picks it up, the slots the others open for it, the drop that keeps
//! it where it lands or removes it, and the tile drawn on its own while it settles or fades.

use std::collections::HashMap;

use cupertino_kit::motion::{Animated, Easing};
use cupertino_wm::geometry::{Point, Rect};

use super::{
    Departing, Dock, DragOutcome, DragState, DraggedTile, PlacedTile, Section, Settle, Slide,
};
use crate::config::Orientation;
use crate::ease::cosine_ease;
use crate::magnify::fish_ramp_ms;
use crate::tile_drag::{
    LOOK_S, REMOVE_DELAY_S, REMOVE_FADE_CURVE, REMOVE_FADE_S, REMOVING_OPACITY, SETTLE_S,
};
use crate::tiles::{Tile, TileId, TileKind, TileLifecycle};

impl Dock {
    pub(super) fn section_of(&self, id: TileId) -> Option<(Section, usize)> {
        let live = |list: &[Tile]| list.iter().filter(|t| !t.leaving).position(|t| t.id == id);
        live(&self.apps)
            .map(|i| (Section::Apps, i))
            .or_else(|| live(&self.recents).map(|i| (Section::Recents, i)))
    }

    /// Where the dragged icon is drawn: where it was grabbed, relative to the pointer.
    pub(super) fn dragged_rect(&self, d: &DragState) -> Rect {
        Rect::new(
            d.pointer.x + d.grab.x,
            d.pointer.y + d.grab.y,
            d.size.x,
            d.size.y,
        )
    }

    /// Start dragging an app or recent tile pressed at `press`, the pointer now at `pointer`
    /// (more than the drag threshold away). The icon keeps the place it was grabbed at, at the
    /// size it had; its slot becomes the gap. Magnification is released.
    pub fn begin_drag(
        &mut self,
        id: TileId,
        press: Point,
        pointer: Point,
        screen: Rect,
        now: f64,
    ) -> bool {
        let Some((section, from)) = self.section_of(id) else {
            return false;
        };
        if self.tile(id).is_some_and(|t| t.built_in) {
            return false;
        }
        let Some(rect) = self
            .layout(screen, now)
            .tiles
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.rect)
        else {
            return false;
        };
        let look = Animated::new(0.0);
        self.drag = Some(DragState {
            id,
            section,
            pointer,
            insertion: from,
            from,
            grab: Point::new(rect.x - press.x, rect.y - press.y),
            size: Point::new(rect.w, rect.h),
            start: now,
            inserting: true,
            zone: false,
            over_trash: false,
            look,
        });
        self.settle = None;
        self.rest = self.rest_positions(screen);
        self.slides.clear();
        self.hot = false;
        let ms = if self.reduce_motion {
            0.0
        } else {
            fish_ramp_ms(&self.cfg, 0.0, true)
        };
        self.fish.animate_to(0.0, ms / 1000.0, now);
        self.drag_move(pointer, screen, now);
        true
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Whether releasing at `now` would remove the dragged tile: the pointer is past the removal
    /// edge and the delay is over, or it is on the trash.
    pub fn drag_will_remove(&self, now: f64) -> bool {
        self.drag.as_ref().is_some_and(|d| d.removes(now))
    }

    /// How far the dragged icon has dimmed and its "Remove" label shown, 0 to 1.
    pub fn drag_look(&self, now: f64) -> f64 {
        self.drag.as_ref().map_or(0.0, |d| d.look.value(now))
    }

    /// Where along the dock axis the flow puts each tile at rest, with the drag as it is.
    pub(super) fn rest_positions(&self, screen: Rect) -> HashMap<TileId, f64> {
        let horizontal = self.cfg.orientation.is_horizontal();
        let drag = self.drag.clone();
        let rest = self.build(screen, None, 0.0, 0.0, None, drag.as_ref(), false);
        rest.tiles
            .iter()
            .map(|t| (t.id, if horizontal { t.rect.x } else { t.rect.y }))
            .collect()
    }

    /// The gap moved: every tile whose place changed slides from where it is drawn to the new one.
    pub(super) fn retarget_slides(&mut self, screen: Rect, now: f64) {
        let next = self.rest_positions(screen);
        for (id, to) in &next {
            let Some(old) = self.rest.get(id) else {
                continue;
            };
            if (old - to).abs() > 1e-6 {
                let lag = self.slides.get(id).map_or(0.0, |s| s.remaining(now));
                self.slides.insert(
                    *id,
                    Slide {
                        offset: lag + old - to,
                        start: now,
                    },
                );
            }
        }
        self.rest = next;
    }

    /// The pointer moved with a tile held: where the gap goes, whether it is open, whether a drop
    /// would remove the tile.
    pub fn drag_move(&mut self, pointer: Point, screen: Rect, now: f64) {
        let Some(mut d) = self.drag.clone() else {
            return;
        };
        d.pointer = pointer;
        let axis = self.pointer_axis(pointer);
        let horizontal = self.cfg.orientation.is_horizontal();
        let span = |r: &Rect| {
            if horizontal {
                (r.x, r.right())
            } else {
                (r.y, r.bottom())
            }
        };
        let current = self.layout(screen, now);
        let rest = self.build(screen, None, 0.0, 0.0, None, None, false);

        // the tile the pointer is over decides the index: before its middle the gap goes before
        // it, after its middle after it, on the tiles as they are drawn now
        let list = if d.section == Section::Apps {
            &self.apps
        } else {
            &self.recents
        };
        let ids: Vec<TileId> = list
            .iter()
            .filter(|t| !t.leaving && t.id != d.id)
            .map(|t| t.id)
            .collect();
        let mut insertion = ids.len();
        for (k, id) in ids.iter().enumerate() {
            let Some(r) = current
                .tiles
                .iter()
                .find(|p| p.id == *id)
                .map(|p| span(&p.rect))
            else {
                continue;
            };
            if axis < r.0 {
                insertion = k;
                break;
            }
            if axis <= r.1 {
                insertion = if axis >= (r.0 + r.1) / 2.0 { k + 1 } else { k };
                break;
            }
        }
        // nothing goes ahead of the built-in tile
        d.insertion = if d.section == Section::Apps {
            insertion.max(self.first_movable())
        } else {
            insertion
        };

        // the gap is open while the pointer is within a tile of the screen edge and over the flow
        // of the section the tile came from
        let tile = self.cfg.tile_size;
        let near_edge = match self.cfg.orientation {
            Orientation::Bottom => pointer.y >= screen.bottom() - tile,
            Orientation::Left => pointer.x <= screen.x + tile,
            Orientation::Right => pointer.x >= screen.right() - tile,
        };
        let start_of = |kind: TileKind| {
            rest.tiles
                .iter()
                .find(|p| p.kind == kind)
                .map(|p| span(&p.rect))
        };
        let separators: Vec<(f64, f64)> = rest
            .tiles
            .iter()
            .filter(|p| p.kind == TileKind::Separator)
            .map(|p| span(&p.rect))
            .collect();
        let trash_start = start_of(TileKind::Trash).map_or(f64::INFINITY, |r| r.0);
        let (lower, upper) = match d.section {
            Section::Apps => (
                f64::NEG_INFINITY,
                separators.first().map_or(trash_start, |s| s.0),
            ),
            Section::Recents => (
                separators.first().map_or(f64::NEG_INFINITY, |s| s.1),
                separators.get(1).map_or(trash_start, |s| s.0),
            ),
        };
        d.inserting = near_edge && axis >= lower && axis <= upper;

        // the removal edge is a bar thickness beyond the bar, away from the screen edge
        let thickness = self.cfg.bar_thickness();
        d.zone = match self.cfg.orientation {
            Orientation::Bottom => pointer.y < rest.bar.y - thickness,
            Orientation::Left => pointer.x > rest.bar.right() + thickness,
            Orientation::Right => pointer.x < rest.bar.x - thickness,
        };
        d.over_trash = rest
            .tiles
            .iter()
            .any(|p| p.kind == TileKind::Trash && p.rect.contains(pointer));
        self.drag = Some(d);
        self.sync_look(now);
        self.retarget_slides(screen, now);
    }

    /// The icon dims and the label shows once a drop would remove the tile by the pointer's place
    /// alone, and go back when it no longer would.
    pub(super) fn sync_look(&mut self, now: f64) {
        let Some(d) = self.drag.as_mut() else { return };
        let target = if d.zone && now - d.start >= REMOVE_DELAY_S {
            1.0
        } else {
            0.0
        };
        if d.look.target() != target {
            d.look
                .animate_to(target, Easing::EASE_IN_OUT.over(LOOK_S), now);
        }
    }

    pub fn cancel_drag(&mut self) {
        self.drag = None;
        self.rest.clear();
        self.slides.clear();
    }

    /// Finish the drag and apply the reorder or removal. A removal fades the icon out where it was
    /// released; anything else settles the icon into its slot.
    pub fn end_drag(&mut self, screen: Rect, now: f64) -> DragOutcome {
        let Some(d) = self.drag.take() else {
            return DragOutcome::Cancelled;
        };
        let from_rect = self.dragged_rect(&d);
        let list = if d.section == Section::Apps {
            &mut self.apps
        } else {
            &mut self.recents
        };
        let Some(pos) = list.iter().position(|t| t.id == d.id) else {
            return DragOutcome::Cancelled;
        };
        let key = list[pos].key.clone();
        let kind = list[pos].kind;
        if d.removes(now) {
            let t = &mut list[pos];
            if d.section == Section::Apps {
                t.pinned = false;
            }
            if d.section == Section::Recents || !t.running {
                // its slot was closed since the pointer left the dock: it goes at once
                t.presence = Animated::new(0.0);
                t.leaving = true;
            }
            let tile = PlacedTile {
                id: d.id,
                kind,
                rect: from_rect,
                lift: 0.0,
                pulse: 0.0,
                lifecycle: TileLifecycle::Idle,
                indicator: false,
                indicator_alpha: 1.0,
                presence: 1.0,
            };
            self.departing = Some(Departing {
                tile,
                key: key.clone(),
                start: now,
            });
            self.retarget_slides(screen, now);
            return DragOutcome::Removed { key, kind };
        }
        let look = d.look.value(now);
        self.settle = Some(Settle {
            id: d.id,
            from: from_rect,
            opacity: 1.0 - look * (1.0 - REMOVING_OPACITY),
            label: look,
            start: now,
        });
        let outcome = if !d.inserting || d.insertion == d.from {
            DragOutcome::Cancelled
        } else {
            let t = list.remove(pos);
            let live_before: Vec<usize> = list
                .iter()
                .enumerate()
                .filter(|(_, t)| !t.leaving)
                .map(|(i, _)| i)
                .collect();
            let at = live_before.get(d.insertion).copied().unwrap_or(list.len());
            list.insert(at, t);
            DragOutcome::Reordered {
                key,
                from: d.from,
                to: d.insertion,
            }
        };
        self.retarget_slides(screen, now);
        outcome
    }

    pub fn drag_insertion(&self) -> Option<usize> {
        self.drag.as_ref().map(|d| d.insertion)
    }

    /// The tile that settles into its slot after a drop, taken out of `tiles`, while it does.
    pub(super) fn settling_tile(
        &self,
        tiles: &mut Vec<PlacedTile>,
        now: f64,
    ) -> Option<DraggedTile> {
        let s = self
            .settle
            .filter(|s| (s.start..s.start + SETTLE_S).contains(&now))?;
        let slot = tiles.remove(tiles.iter().position(|t| t.id == s.id)?);
        let e = cosine_ease((now - s.start) / SETTLE_S);
        let lerp = |a: f64, b: f64| a + (b - a) * e;
        let rect = Rect::new(
            lerp(s.from.x, slot.rect.x),
            lerp(s.from.y, slot.rect.y),
            lerp(s.from.w, slot.rect.w),
            lerp(s.from.h, slot.rect.h),
        );
        let key = self
            .apps
            .iter()
            .chain(&self.recents)
            .find(|t| t.id == s.id)?
            .key
            .clone();
        Some(DraggedTile {
            indicator: if slot.indicator { e } else { 0.0 },
            tile: PlacedTile { rect, ..slot },
            key,
            opacity: lerp(s.opacity, 1.0),
            label: s.label * (1.0 - e),
        })
    }

    /// The tile that fades out after a removal, while it does.
    pub(super) fn departing_tile(&self, now: f64) -> Option<DraggedTile> {
        let d = self
            .departing
            .as_ref()
            .filter(|d| (d.start..d.start + REMOVE_FADE_S).contains(&now))?;
        let fade = 1.0 - REMOVE_FADE_CURVE.eval((now - d.start) / REMOVE_FADE_S);
        Some(DraggedTile {
            tile: d.tile.clone(),
            key: d.key.clone(),
            opacity: fade,
            label: fade,
            indicator: 0.0,
        })
    }
}
