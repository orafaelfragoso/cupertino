//! The semantics pass. Every node declares its role, value, actions and whether it takes keyboard focus. A node
//! without a role stays out of the accessibility tree and its children move up to the nearest node with one; the
//! window's root is always present. A protected node's value is replaced here, before any adapter, inspector or
//! test sees the tree.

use crate::accessibility::{AccessibilityNode, AccessibleValue, Role};
use crate::cx::{Semantics, SemanticsCx};
use crate::route::Geometry;
use crate::scroll::can_scroll;
use crate::tree::Tree;
use crate::NodeId;
use cupertino_geometry::{Rect, Size};
use cupertino_ui_layout::LayoutDirection;
use std::collections::BTreeMap;

pub(crate) struct SemanticsPass {
    pub(crate) nodes: BTreeMap<NodeId, AccessibilityNode>,
    /// Tab stops, in reading order.
    pub(crate) focusable: Vec<NodeId>,
    /// Focus scopes, in reading order.
    pub(crate) scopes: Vec<NodeId>,
    pub(crate) roving_groups: Vec<NodeId>,
    /// Items of roving groups, in reading order.
    pub(crate) roving_items: Vec<NodeId>,
    /// The roots of hidden subtrees, in reading order.
    pub(crate) hidden: Vec<NodeId>,
    /// The node assistive technology is told holds the focus: the focused node, or its nearest ancestor in the tree
    /// when it has no role of its own.
    pub(crate) focus: Option<NodeId>,
}

/// What a mounted node declares, outside a pass.
fn declared<M: 'static>(tree: &Tree<M>, id: NodeId) -> Option<Semantics> {
    let node = tree.get(id)?;
    let mut semantics = Semantics::default();
    node.widget.semantics(
        &*node.state,
        &mut SemanticsCx {
            node: id,
            size: Size::ZERO,
            direction: LayoutDirection::Ltr,
            can_scroll: false,
            semantics: &mut semantics,
        },
    );
    Some(semantics)
}

/// Whether the node marks its value as protected, such as a password field.
pub(crate) fn protected<M: 'static>(tree: &Tree<M>, id: NodeId) -> bool {
    declared(tree, id).is_some_and(|s| s.protected)
}

/// Whether the node asks for the focus when its scope opens.
pub(crate) fn initial_focus<M: 'static>(tree: &Tree<M>, id: NodeId) -> bool {
    declared(tree, id).is_some_and(|s| s.initial_focus)
}

/// Whether the node is a text input, whose focus shows its ring however it got there.
pub(crate) fn text_input<M: 'static>(tree: &Tree<M>, id: NodeId) -> bool {
    declared(tree, id).is_some_and(|s| s.role == Some(Role::TextInput))
}

/// Whether the node asked that a press on it leave a focused text field editing ([`SemanticsCx::keeps_editing`]).
pub(crate) fn keeps_editing<M: 'static>(tree: &Tree<M>, id: NodeId) -> bool {
    declared(tree, id).is_some_and(|s| s.keeps_editing)
}

pub(crate) fn build<M: 'static>(
    tree: &Tree<M>,
    root: NodeId,
    geometry: &Geometry,
    focus: Option<NodeId>,
    direction: LayoutDirection,
    scale: f64,
    raised: &[(NodeId, Rect)],
) -> SemanticsPass {
    let mut pass = SemanticsPass {
        nodes: BTreeMap::new(),
        focusable: Vec::new(),
        scopes: Vec::new(),
        roving_groups: Vec::new(),
        roving_items: Vec::new(),
        hidden: Vec::new(),
        focus: None,
    };
    // Each fallback and where it would stand among the Tab stops, in reading order.
    let mut fallbacks = Vec::new();
    // Descriptions for the first node with a role under each node that gave one.
    let mut pending: Vec<(NodeId, String)> = Vec::new();
    // Each node with relations to keyed nodes, resolved once every node is in.
    let mut keyed = Vec::new();
    // Each entry is a node and its nearest ancestor in the accessibility tree.
    let mut stack = vec![(root, None::<NodeId>)];
    while let Some((id, owner)) = stack.pop() {
        let Some(node) = tree.get(id) else { continue };
        let mut semantics = Semantics::default();
        let rect = geometry.get(&id).map_or(Rect::default(), |p| p.rect);
        node.widget.semantics(
            &*node.state,
            &mut SemanticsCx {
                node: id,
                size: rect.size,
                direction,
                can_scroll: can_scroll(tree, geometry, id, direction, scale),
                semantics: &mut semantics,
            },
        );
        if semantics.hidden {
            pass.hidden.push(id);
            continue;
        }
        if semantics.focusable {
            pass.focusable.push(id);
        } else if semantics.focus_fallback {
            fallbacks.push((id, pass.focusable.len()));
        }
        if semantics.focus_scope {
            pass.scopes.push(id);
        }
        if semantics.roving_group {
            pass.roving_groups.push(id);
        }
        if semantics.roving_item {
            pass.roving_items.push(id);
        }
        let role = match semantics.role {
            Some(role) => Some(role),
            None if id == root => Some(Role::Window),
            None => None,
        };
        let next_owner = match role {
            Some(role) => {
                if !semantics.keyed_relations.is_empty() {
                    keyed.push((id, std::mem::take(&mut semantics.keyed_relations)));
                }
                let value = if semantics.protected {
                    Some(AccessibleValue::Protected)
                } else {
                    semantics.value
                };
                let mut states = semantics.states;
                states.focused = focus == Some(id);
                // a roving item the arrows moved to holds the focus without being a Tab stop; it can still take focus
                states.focusable |= semantics.focusable || states.focused;
                let mut description = semantics.description;
                if !pending.is_empty() {
                    let path = tree.path(id);
                    let (taken, waiting) = pending
                        .drain(..)
                        .partition::<Vec<_>, _>(|(giver, _)| path.contains(giver));
                    pending = waiting;
                    for (_, text) in taken {
                        description = Some(join(description, text));
                    }
                }
                pass.nodes.insert(
                    id,
                    AccessibilityNode {
                        role,
                        name: semantics.name,
                        description,
                        value,
                        states,
                        bounds: {
                            let bounds = semantics.bounds.map_or(rect, |b| {
                                Rect::new(
                                    rect.origin.x + b.origin.x,
                                    rect.origin.y + b.origin.y,
                                    b.size.width,
                                    b.size.height,
                                )
                            });
                            // a scroll view under raised nodes shows its content only below them; what it scrolls
                            // keeps its place, so assistive technology can still find and scroll to it
                            let under = geometry
                                .get(&id)
                                .is_some_and(|p| p.raised != crate::route::Raised::In)
                                && tree.get(id).is_some_and(|n| {
                                    matches!(
                                        n.spec.kind,
                                        cupertino_ui_layout::ContainerKind::Scroll { .. }
                                    )
                                });
                            if under && !raised.is_empty() {
                                crate::paint::unobscured(bounds, raised)
                            } else {
                                bounds
                            }
                        },
                        actions: semantics.actions,
                        relations: semantics.relations,
                        children: Vec::new(),
                        live: semantics.live,
                        item_count: semantics.item_count,
                        level: semantics.level,
                        item_index: semantics.item_index,
                        column_index: semantics.column_index,
                        sort: semantics.sort,
                    },
                );
                if let Some(owner) = owner {
                    pass.nodes
                        .get_mut(&owner)
                        .expect("an owner is inserted before its descendants")
                        .children
                        .push(id);
                }
                Some(id)
            }
            None => {
                if let (Some(description), Some(owner)) = (semantics.description, owner) {
                    let owner = pass
                        .nodes
                        .get_mut(&owner)
                        .expect("an owner is inserted before its descendants");
                    owner.description = Some(join(owner.description.take(), description));
                }
                owner
            }
        };
        if let Some(text) = semantics.content_description {
            pending.push((id, text));
        }
        stack.extend(node.children.iter().rev().map(|&c| (c, next_owner)));
    }
    for (id, relations) in keyed {
        let path = tree.path(id);
        for (relation, key) in relations {
            let target = path.iter().rev().skip(1).find_map(|&ancestor| {
                tree.get(ancestor)?
                    .children
                    .iter()
                    .copied()
                    .find(|&c| tree.get(c).is_some_and(|n| n.key.as_ref() == Some(&key)))
            });
            if let Some(target) = target.filter(|t| pass.nodes.contains_key(t)) {
                pass.nodes
                    .get_mut(&id)
                    .expect("a node with keyed relations is in the tree")
                    .relations
                    .push(relation(target));
            }
        }
    }
    // The last first, so an inner fallback that becomes a stop keeps its outer one from becoming one, and the
    // positions of the earlier ones still hold.
    for (id, at) in fallbacks.into_iter().rev() {
        let covered = pass.focusable[at..]
            .iter()
            .any(|&n| tree.path(n).contains(&id));
        if !covered {
            pass.focusable.insert(at, id);
            if let Some(node) = pass.nodes.get_mut(&id) {
                node.states.focusable = true;
            }
        }
    }
    // A focus left on a node without a role (the container of a list's rows, once the row it was on went away) is
    // read on its nearest ancestor in the tree, which then holds it: assistive technology is never told of a focus it
    // cannot see. The keyboard stays where it is.
    pass.focus = focus.filter(|&n| tree.contains(n)).and_then(|n| {
        tree.path(n)
            .into_iter()
            .rev()
            .find(|a| pass.nodes.contains_key(a))
    });
    if pass.focus != focus {
        if let Some(node) = pass.focus.and_then(|n| pass.nodes.get_mut(&n)) {
            node.states.focused = true;
            node.states.focusable = true;
        }
    }
    pass
}

/// A node's own description, then one it was given.
fn join(own: Option<String>, given: String) -> String {
    match own {
        Some(own) => format!("{own}, {given}"),
        None => given,
    }
}

/// Whether the node is composing text, which Escape cancels before it dismisses anything.
pub(crate) fn composing<M: 'static>(tree: &Tree<M>, id: NodeId) -> bool {
    declared(tree, id).is_some_and(|s| s.composing)
}

/// The nodes under `root` that are dismissible now and not hidden, in reading order.
pub(crate) fn dismissible<M: 'static>(tree: &Tree<M>, root: NodeId) -> Vec<NodeId> {
    tree.preorder(root)
        .into_iter()
        .filter(|&id| declared(tree, id).is_some_and(|s| s.dismissible))
        .filter(|&id| {
            !tree
                .path(id)
                .into_iter()
                .skip_while(|&p| p != root)
                .any(|p| declared(tree, p).is_some_and(|s| s.hidden))
        })
        .collect()
}

/// The nodes that are new or changed in `next`, and those gone since `prev`.
pub(crate) fn diff(
    prev: &BTreeMap<NodeId, AccessibilityNode>,
    next: &BTreeMap<NodeId, AccessibilityNode>,
) -> (Vec<(NodeId, AccessibilityNode)>, Vec<NodeId>) {
    let changed = next
        .iter()
        .filter(|(id, node)| prev.get(id) != Some(node))
        .map(|(&id, node)| (id, node.clone()))
        .collect();
    let removed = prev
        .keys()
        .filter(|id| !next.contains_key(id))
        .copied()
        .collect();
    (changed, removed)
}

#[cfg(test)]
mod tests;
