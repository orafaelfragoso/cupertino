//! Keyboard focus. Tab order is the tree's reading order over focusable nodes. A focus scope (dialog, popover,
//! menu) keeps Tab inside itself while it is the innermost open scope, and hands the focus back when it unmounts:
//! to the node focused before it opened or, if that node is gone, its nearest surviving ancestor.

use crate::semantics::{initial_focus, text_input};
use crate::tree::Tree;
use crate::NodeId;

struct Scope {
    node: NodeId,
    /// The path to the node focused before the scope opened, root first.
    restore: Vec<NodeId>,
}

/// A focus move the runtime reports to the nodes involved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FocusChange {
    pub(crate) from: Option<NodeId>,
    pub(crate) to: Option<NodeId>,
    pub(crate) visible: bool,
    /// Strict ancestors of the old focus that are no longer ones of the new focus, innermost first.
    pub(crate) left: Vec<NodeId>,
    /// Strict ancestors of the new focus that were not ones of the old focus, outermost first.
    pub(crate) entered: Vec<NodeId>,
}

#[derive(Default)]
pub(crate) struct Focus {
    node: Option<NodeId>,
    visible: bool,
    path: Vec<NodeId>,
    scopes: Vec<Scope>,
    opened: bool,
}

impl Focus {
    /// No focus yet, with the ring of the focus it takes showing when `visible`.
    pub(crate) fn shown(visible: bool) -> Self {
        Self {
            visible,
            ..Self::default()
        }
    }

    pub(crate) fn node(&self) -> Option<NodeId> {
        self.node
    }

    pub(crate) fn visible(&self) -> bool {
        self.visible
    }

    pub(crate) fn set<M: 'static>(
        &mut self,
        tree: &Tree<M>,
        to: Option<NodeId>,
        visible: bool,
    ) -> Option<FocusChange> {
        let to = to.filter(|&n| tree.contains(n));
        if to == self.node && visible == self.visible {
            return None;
        }
        let from = self.node.filter(|&n| tree.contains(n));
        self.node = to;
        self.visible = to.is_some() && visible;
        let old = std::mem::replace(&mut self.path, to.map(|n| tree.path(n)).unwrap_or_default());
        let (old, new) = (ancestors(&old), ancestors(&self.path));
        let common = old.iter().zip(new).take_while(|(a, b)| a == b).count();
        Some(FocusChange {
            from,
            to,
            visible: self.visible,
            left: old[common..]
                .iter()
                .rev()
                .copied()
                .filter(|&n| tree.contains(n))
                .collect(),
            entered: new[common..].to_vec(),
        })
    }

    /// Brings focus up to date after a frame: closed scopes give the focus back, a focus whose node unmounted moves
    /// to its nearest surviving ancestor, and a newly open scope takes the focus to its first Tab stop at or under an
    /// initial-focus node, or else its first focusable one. On the window's first frame an unfocused window takes the
    /// focus to its first Tab stop at or under an initial-focus node, if it has one. A text input given the focus as
    /// its window or scope opens shows its ring. A focus left inside a hidden subtree moves to the next Tab stop after
    /// that subtree, or else the one before it. `scopes`, `focusable` and `hidden` are in reading order.
    pub(crate) fn sync<M: 'static>(
        &mut self,
        tree: &Tree<M>,
        scopes: &[NodeId],
        focusable: &[NodeId],
        hidden: &[NodeId],
    ) -> Option<FocusChange> {
        let from = self.node;
        let mut visible = self.visible;
        let mut target = self.node;
        let mut moved = false;

        while let Some(top) = self.scopes.last() {
            if tree.contains(top.node) {
                break;
            }
            let closed = self.scopes.pop().expect("checked above");
            target = deepest_alive(tree, &closed.restore);
            moved = true;
        }
        self.scopes.retain(|s| tree.contains(s.node));

        if !moved && target.is_some_and(|n| !tree.contains(n)) {
            target = deepest_alive(tree, &self.path);
            moved = true;
        }

        if !std::mem::replace(&mut self.opened, true) && target.is_none() {
            target = focusable
                .iter()
                .copied()
                .find(|&n| tree.path(n).into_iter().any(|p| initial_focus(tree, p)));
            if let Some(node) = target {
                moved = true;
                visible |= text_input(tree, node);
            }
        }

        for &scope in scopes {
            if self.scopes.iter().any(|s| s.node == scope) {
                continue;
            }
            let restore = target.map(|n| tree.path(n)).unwrap_or_default();
            self.scopes.push(Scope {
                node: scope,
                restore,
            });
            if !target.is_some_and(|n| tree.path(n).contains(&scope)) {
                let inside: Vec<NodeId> = focusable
                    .iter()
                    .copied()
                    .filter(|&n| n != scope && tree.path(n).contains(&scope))
                    .collect();
                target = inside
                    .iter()
                    .copied()
                    .find(|&n| {
                        tree.path(n)
                            .into_iter()
                            .skip_while(|&p| p != scope)
                            .any(|p| initial_focus(tree, p))
                    })
                    .or(inside.first().copied())
                    .or(Some(scope));
                visible |= target.is_some_and(|n| text_input(tree, n));
                moved = true;
            }
        }

        if let Some(past) = target.and_then(|n| self.past_hidden(tree, n, focusable, hidden)) {
            target = past;
            moved = true;
        }

        if !moved {
            return None;
        }
        if target == from {
            self.path = target.map(|n| tree.path(n)).unwrap_or_default();
            return None;
        }
        self.node = None;
        self.set(tree, target, visible)
            .map(|change| FocusChange { from, ..change })
    }

    /// Where a focus on `node` goes when a hidden subtree holds it: the first Tab stop after the subtree in the innermost
    /// open scope, or else the last one before it. None when no hidden subtree holds `node`.
    fn past_hidden<M: 'static>(
        &self,
        tree: &Tree<M>,
        node: NodeId,
        focusable: &[NodeId],
        hidden: &[NodeId],
    ) -> Option<Option<NodeId>> {
        let path = tree.path(node);
        let &cut = hidden.iter().find(|h| path.contains(h))?;
        let scope = self.scopes.last().map(|s| s.node);
        let mut before = None;
        let mut past = false;
        for n in tree.preorder(path[0]) {
            let at = tree.path(n);
            if at.contains(&cut) {
                past = true;
            } else if focusable.contains(&n) && scope.is_none_or(|s| at.contains(&s)) {
                if past {
                    return Some(Some(n));
                }
                before = Some(n);
            }
        }
        Some(before)
    }

    /// The next (or previous) Tab stop, inside the innermost open scope.
    pub(crate) fn step<M: 'static>(
        &self,
        tree: &Tree<M>,
        focusable: &[NodeId],
        forward: bool,
    ) -> Option<NodeId> {
        let scope = self.scopes.last().map(|s| s.node);
        let candidates: Vec<NodeId> = focusable
            .iter()
            .copied()
            .filter(|&n| scope.is_none_or(|s| tree.path(n).contains(&s)))
            .collect();
        if candidates.is_empty() {
            return None;
        }
        let len = candidates.len();
        let next = match self
            .node
            .and_then(|n| candidates.iter().position(|&c| c == n))
        {
            Some(i) if forward => (i + 1) % len,
            Some(i) => (i + len - 1) % len,
            None if forward => 0,
            None => len - 1,
        };
        Some(candidates[next])
    }
}

/// The nodes above the last of `path`.
fn ancestors(path: &[NodeId]) -> &[NodeId] {
    path.split_last().map_or(&[], |(_, above)| above)
}

fn deepest_alive<M: 'static>(tree: &Tree<M>, path: &[NodeId]) -> Option<NodeId> {
    path.iter().rev().copied().find(|&n| tree.contains(n))
}

#[cfg(test)]
mod tests;
