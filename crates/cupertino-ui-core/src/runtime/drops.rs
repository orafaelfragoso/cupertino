//! Drags from any application over the runtime's windows: which [`DropTarget`](crate::DropTarget) takes one where the
//! pointer is, what the window system is told, the highlight of the place, and the data read at the drop.

use super::Runtime;
use crate::drop_target::{DropFrame, Lit};
use crate::route::{hit_test, Geometry};
use crate::tree::Tree;
use crate::widget::Change;
use crate::{Application, DropOver, DropPlace, Dropped, Key, NodeId, WindowKey};
use cupertino_geometry::{Point, Rect};
use cupertino_ui_platform::{
    ClipboardData, ClipboardToken, DropAction, DropAnswer, DropEvent, DropToken, Mods, WindowSystem,
};
use std::collections::BTreeSet;

/// A drag over a window, from its enter until it leaves or drops.
pub(super) struct DropState {
    token: DropToken,
    /// In window coordinates.
    position: Point,
    mimes: Vec<String>,
    actions: BTreeSet<DropAction>,
    mods: Mods,
    /// What a drop would do, as the window system last said.
    chosen: Option<DropAction>,
    /// The target that takes the drag where it is, and where.
    over: Option<Over>,
    /// What the window system was last told.
    answer: Option<DropAnswer>,
}

/// The target a drag is over and the place it would land on.
#[derive(Clone, Debug, PartialEq)]
struct Over {
    target: NodeId,
    place: DropPlace,
    /// The type the target reads.
    mime: String,
    /// The pointer in the target's coordinates.
    position: Point,
    /// The target's highlight node and what it outlines, in that node's coordinates.
    lit: Option<(NodeId, Rect)>,
}

/// A drop whose data is being read.
pub(super) struct Landed {
    token: DropToken,
    window: WindowKey,
    target: NodeId,
    place: Option<Key>,
    position: Point,
    action: DropAction,
    mods: Mods,
}

/// The action a drop does without the window system's word: the target's preferred one when the source allows it,
/// else the first both allow.
fn settle(
    preferred: DropAction,
    target: &BTreeSet<DropAction>,
    source: &BTreeSet<DropAction>,
) -> Option<DropAction> {
    if target.contains(&preferred) && source.contains(&preferred) {
        return Some(preferred);
    }
    target.intersection(source).next().copied()
}

fn frame<M: 'static>(tree: &Tree<M>, node: NodeId) -> Option<&DropFrame<M>> {
    tree.get(node)?
        .widget
        .as_any()
        .downcast_ref::<DropFrame<M>>()
}

/// The innermost target on `path` that takes one of the drag's types and a drop where it is.
fn over<M: 'static>(
    tree: &Tree<M>,
    geometry: &Geometry,
    path: &[NodeId],
    drop: &DropState,
) -> Option<Over> {
    for (depth, &node) in path.iter().enumerate().rev() {
        let Some(frame) = frame(tree, node) else {
            continue;
        };
        let Some(mime) = frame.takes(&drop.mimes) else {
            continue;
        };
        let Some(placement) = geometry.get(&node) else {
            continue;
        };
        let origin = placement.rect.origin;
        let position = Point::new(drop.position.x - origin.x, drop.position.y - origin.y);
        let keys: Vec<Key> = path[depth + 1..]
            .iter()
            .rev()
            .filter_map(|&n| tree.get(n)?.key.clone())
            .collect();
        let place = (frame.place)(&DropOver {
            position,
            keys: &keys,
            mimes: &drop.mimes,
            actions: &drop.actions,
            mods: drop.mods,
        });
        if let Some(place) = place {
            let lit = lit(tree, geometry, node, &path[depth + 1..], &place);
            return Some(Over {
                target: node,
                place,
                mime: mime.to_owned(),
                position,
                lit,
            });
        }
    }
    None
}

/// The highlight node of `target` and the part of it the place covers: the keyed node of that key under the pointer,
/// or else in the target's subtree, or the whole target; cut by that node's clip.
fn lit<M: 'static>(
    tree: &Tree<M>,
    geometry: &Geometry,
    target: NodeId,
    under: &[NodeId],
    place: &DropPlace,
) -> Option<(NodeId, Rect)> {
    let highlight = *tree.get(target)?.children.last()?;
    let shown = match &place.key {
        None => target,
        Some(key) => {
            let keyed = |n: &NodeId| tree.get(*n).is_some_and(|n| n.key.as_ref() == Some(key));
            match under.iter().copied().find(keyed) {
                Some(node) => node,
                None => tree.preorder(target).into_iter().find(keyed)?,
            }
        }
    };
    let placement = geometry.get(&shown)?;
    let rect = match placement.clip {
        Some(clip) => placement.rect.intersection(&clip)?,
        None => placement.rect,
    };
    let origin = geometry.get(&highlight)?.rect.origin;
    Some((highlight, rect.translate(-origin.x, -origin.y)))
}

/// Shows `rect` on the highlight `node`, or nothing.
fn light<M: 'static>(tree: &mut Tree<M>, node: NodeId, rect: Option<Rect>) {
    let Some(n) = tree.get_mut(node) else { return };
    let Some(lit) = n.state.downcast_mut::<Lit>() else {
        return;
    };
    if lit.0 != rect {
        lit.0 = rect;
        tree.mark(node, Change::Paint);
    }
}

impl<A: Application, W: WindowSystem> Runtime<A, W> {
    pub(super) fn drop_event(&mut self, i: usize, event: DropEvent) {
        match event {
            DropEvent::Enter {
                token,
                position,
                mimes,
                actions,
                mods,
            } => {
                // the window system leaves before it enters again; a drag it never left ends here
                self.drop_left(i);
                self.windows[i].drop = Some(DropState {
                    token,
                    position,
                    mimes,
                    actions,
                    mods,
                    chosen: None,
                    over: None,
                    answer: None,
                });
                self.drop_over(i);
            }
            DropEvent::Motion { position, mods } => {
                if let Some(drop) = &mut self.windows[i].drop {
                    (drop.position, drop.mods) = (position, mods);
                    self.drop_over(i);
                }
            }
            DropEvent::Action(action) => {
                if let Some(drop) = &mut self.windows[i].drop {
                    drop.chosen = action;
                }
            }
            DropEvent::Leave => self.drop_left(i),
            DropEvent::Drop => self.dropped(i),
        }
    }

    /// Asks the targets under the drag of window `i` where it would land, and tells the window system, the targets
    /// and the highlights what changed.
    pub(super) fn drop_over(&mut self, i: usize) {
        let w = &mut self.windows[i];
        let (Some(drop), Some(root)) = (&w.drop, w.root) else {
            return;
        };
        let path = hit_test(&self.tree, root, &w.geometry, drop.position).unwrap_or_default();
        let now = over(&self.tree, &w.geometry, &path, drop);
        let answer = match &now {
            Some(over) => DropAnswer {
                mime: Some(over.mime.clone()),
                actions: frame(&self.tree, over.target)
                    .map(|f| f.actions.clone())
                    .unwrap_or_default(),
                preferred: Some(over.place.action),
            },
            None => DropAnswer::default(),
        };
        let token = drop.token;
        let before = w.drop.as_mut().and_then(|d| d.over.take());
        if w.drop.as_ref().and_then(|d| d.answer.as_ref()) != Some(&answer) {
            // a window system that cannot answer still shows the place
            let _ = self.system.answer_drop(token, &answer);
            if let Some(drop) = &mut w.drop {
                drop.answer = Some(answer);
            }
        }
        self.hover(before.as_ref(), now.as_ref());
        if let Some(drop) = &mut self.windows[i].drop {
            drop.over = now;
        }
    }

    /// The drag moved from `before` to `now`: tell the targets, and move the highlight.
    fn hover(&mut self, before: Option<&Over>, now: Option<&Over>) {
        let same_target = before.map(|o| o.target) == now.map(|o| o.target);
        if let Some(b) = before.filter(|_| !same_target) {
            if let Some(on_hover) = frame(&self.tree, b.target).and_then(|f| f.on_hover.as_ref()) {
                self.messages.push_back(on_hover(None));
            }
        }
        if let Some(n) = now.filter(|n| !same_target || before.map(|b| &b.place) != Some(&n.place))
        {
            if let Some(on_hover) = frame(&self.tree, n.target).and_then(|f| f.on_hover.as_ref()) {
                self.messages.push_back(on_hover(Some(n.place.clone())));
            }
        }
        let shown = |o: Option<&Over>| o.and_then(|o| o.lit);
        if shown(before) != shown(now) {
            if let Some((node, _)) = shown(before) {
                light(&mut self.tree, node, None);
            }
            if let Some((node, rect)) = shown(now) {
                light(&mut self.tree, node, Some(rect));
            }
        }
    }

    /// The drag over window `i` left it or ended without a drop.
    pub(super) fn drop_left(&mut self, i: usize) {
        let Some(mut drop) = self.windows[i].drop.take() else {
            return;
        };
        let before = drop.over.take();
        self.hover(before.as_ref(), None);
    }

    /// The drag over window `i` dropped: read the data for the target it was over, or refuse it.
    fn dropped(&mut self, i: usize) {
        let Some(mut drop) = self.windows[i].drop.take() else {
            return;
        };
        let Some(over) = drop.over.take() else {
            self.system.finish_drop(drop.token, None);
            return;
        };
        self.hover(Some(&over), None);
        let actions = frame(&self.tree, over.target).map(|f| f.actions.clone());
        let action = drop.chosen.or_else(|| {
            actions.and_then(|target| settle(over.place.action, &target, &drop.actions))
        });
        let Some(action) = action else {
            self.system.finish_drop(drop.token, None);
            return;
        };
        match self.system.read_drop(drop.token, &over.mime) {
            Ok(read) => {
                self.landed.insert(
                    read,
                    Landed {
                        token: drop.token,
                        window: self.windows[i].key.clone(),
                        target: over.target,
                        place: over.place.key,
                        position: over.position,
                        action,
                        mods: drop.mods,
                    },
                );
            }
            Err(_) => self.system.finish_drop(drop.token, None),
        }
    }

    /// The data of a drop arrived, or `None` when it could not be read: hand it to its target, and end the drop once
    /// the turn has handled what it became, so a source told the data moved lets it go only after the target has
    /// it. A target gone in the meantime, or its window, refuses it. Whether `read` was a drop's.
    pub(super) fn drop_data(&mut self, read: ClipboardToken, data: Option<ClipboardData>) -> bool {
        let Some(landed) = self.landed.remove(&read) else {
            return false;
        };
        let open = self.windows.iter().any(|w| w.key == landed.window);
        let target = frame(&self.tree, landed.target).filter(|_| open);
        match (target, data) {
            (Some(target), Some(data)) => {
                self.messages.push_back((target.on_drop)(Dropped {
                    place: landed.place,
                    position: landed.position,
                    action: landed.action,
                    mods: landed.mods,
                    data,
                }));
                self.finishing.push((landed.token, landed.action));
            }
            _ => self.system.finish_drop(landed.token, None),
        }
        true
    }

    /// Ends the drops whose targets have handled them.
    pub(super) fn finish_drops(&mut self) {
        for (token, action) in std::mem::take(&mut self.finishing) {
            self.system.finish_drop(token, Some(action));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drop_without_the_window_systems_word_takes_the_preferred_action_both_allow() {
        let both: BTreeSet<_> = [DropAction::Copy, DropAction::Move].into();
        let copy: BTreeSet<_> = [DropAction::Copy].into();
        assert_eq!(
            settle(DropAction::Move, &both, &both),
            Some(DropAction::Move)
        );
        assert_eq!(
            settle(DropAction::Move, &both, &copy),
            Some(DropAction::Copy)
        );
        assert_eq!(
            settle(DropAction::Move, &copy, &[DropAction::Move].into()),
            None
        );
    }
}
