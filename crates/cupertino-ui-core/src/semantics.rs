//! The semantics pass. Every node declares its role, value, actions and whether it takes keyboard focus. A node
//! without a role stays out of the accessibility tree and its children move up to the nearest node with one; the
//! window's root is always present. A protected node's value is replaced here, before any adapter, inspector or
//! test sees the tree.

use crate::accessibility::{AccessibilityNode, AccessibleValue, Role};
use crate::cx::{Semantics, SemanticsCx};
use crate::route::Geometry;
use crate::tree::Tree;
use crate::NodeId;
use cupertino_geometry::Rect;
use std::collections::BTreeMap;

pub(crate) struct SemanticsPass {
    pub(crate) nodes: BTreeMap<NodeId, AccessibilityNode>,
    /// Tab stops, in reading order.
    pub(crate) focusable: Vec<NodeId>,
    /// Focus scopes, in reading order.
    pub(crate) scopes: Vec<NodeId>,
}

pub(crate) fn build<M: 'static>(
    tree: &Tree<M>,
    root: NodeId,
    geometry: &Geometry,
    focus: Option<NodeId>,
) -> SemanticsPass {
    let mut pass = SemanticsPass {
        nodes: BTreeMap::new(),
        focusable: Vec::new(),
        scopes: Vec::new(),
    };
    // Each entry is a node and its nearest ancestor in the accessibility tree.
    let mut stack = vec![(root, None::<NodeId>)];
    while let Some((id, owner)) = stack.pop() {
        let Some(node) = tree.get(id) else { continue };
        let mut semantics = Semantics::default();
        node.widget.semantics(
            &*node.state,
            &mut SemanticsCx {
                node: id,
                semantics: &mut semantics,
            },
        );
        if semantics.focusable {
            pass.focusable.push(id);
        }
        if semantics.focus_scope {
            pass.scopes.push(id);
        }
        let role = match semantics.role {
            Some(role) => Some(role),
            None if id == root => Some(Role::Window),
            None => None,
        };
        let next_owner = match role {
            Some(role) => {
                let value = if semantics.protected {
                    Some(AccessibleValue::Protected)
                } else {
                    semantics.value
                };
                let mut states = semantics.states;
                states.focusable |= semantics.focusable;
                states.focused = focus == Some(id);
                pass.nodes.insert(
                    id,
                    AccessibilityNode {
                        role,
                        name: semantics.name,
                        description: semantics.description,
                        value,
                        states,
                        bounds: geometry.get(&id).map_or(Rect::default(), |p| p.rect),
                        actions: semantics.actions,
                        relations: semantics.relations,
                        children: Vec::new(),
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
            None => owner,
        };
        stack.extend(node.children.iter().rev().map(|&c| (c, next_owner)));
    }
    pass
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
