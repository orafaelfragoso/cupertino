//! The semantics pass. Every node declares its role, value, actions and whether it takes keyboard focus. A node
//! without a role stays out of the accessibility tree and its children move up to the nearest node with one; the
//! window's root is always present. A protected node's value is replaced here, before any adapter, inspector or
//! test sees the tree.

use crate::accessibility::{AccessibilityNode, AccessibleValue, Role};
use crate::cx::{KeyedRelation, Semantics, SemanticsCx};
use crate::route::Geometry;
use crate::scroll::can_scroll;
use crate::tree::Tree;
use crate::NodeId;
use cupertino_geometry::{Rect, Size};
use cupertino_ui_layout::{IdMap, LayoutDirection};
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

/// The pass of walking every node: [`build_kept`] with nothing kept.
#[cfg(test)]
pub(crate) fn build<M: 'static>(
    tree: &Tree<M>,
    root: NodeId,
    geometry: &Geometry,
    focus: Option<NodeId>,
    direction: LayoutDirection,
    scale: f64,
    raised: &[(NodeId, Rect)],
) -> SemanticsPass {
    build_kept(
        tree,
        root,
        geometry,
        focus,
        direction,
        scale,
        raised,
        &mut KeptSemantics::default(),
    )
}

/// What the walk did for one kept subtree ([`Element::kept`](crate::Element::kept)), so a pass where it did not change
/// lays the same in without asking its widgets again: what each node was asked with, the nodes it put in the tree as
/// they stood when the walk was done with them (before the relations to keyed nodes, the fallbacks and the focus,
/// worked out over the whole tree, touched them), what it handed the node above the subtree, and its entries in the
/// focus order, scopes, roving groups and items, hidden roots and keyed relations, each in order.
struct SemanticsRun {
    /// The kept node's subtree revision and mark, and the environment, when the run was made.
    revision: u64,
    mark: u64,
    direction: LayoutDirection,
    scale: f64,
    /// Every node the walk asked, in order, with the size it was asked at.
    asked: Vec<(NodeId, Size)>,
    /// The nodes put in the accessibility tree, in order, with the bounds each declared.
    accessible: Vec<(NodeId, Option<Rect>)>,
    /// Those nodes as the walk left them, in the same order.
    nodes: Vec<AccessibilityNode>,
    /// What the subtree handed the nearest node above it in the accessibility tree, in order.
    outward: Vec<Outward>,
    focusable: Vec<NodeId>,
    /// Each fallback, with how many of the run's Tab stops come before it.
    fallbacks: Vec<(NodeId, usize)>,
    scopes: Vec<NodeId>,
    roving_groups: Vec<NodeId>,
    roving_items: Vec<NodeId>,
    hidden: Vec<NodeId>,
    keyed: Vec<(NodeId, Vec<KeyedRelation>)>,
}

impl SemanticsRun {
    fn new(revision: u64, mark: u64, direction: LayoutDirection, scale: f64) -> Self {
        Self {
            revision,
            mark,
            direction,
            scale,
            asked: Vec::new(),
            accessible: Vec::new(),
            nodes: Vec::new(),
            outward: Vec::new(),
            focusable: Vec::new(),
            fallbacks: Vec::new(),
            scopes: Vec::new(),
            roving_groups: Vec::new(),
            roving_items: Vec::new(),
            hidden: Vec::new(),
            keyed: Vec::new(),
        }
    }
}

/// What a kept subtree hands the node above it in the accessibility tree.
enum Outward {
    /// A child of its own.
    Child(NodeId),
    /// A description, joined to its own.
    Description(String),
}

/// The runs of a window's kept subtrees between semantics passes: a kept node whose subtree revision and mark are
/// those of its run, whose nodes have the sizes they were asked at, that holds no focus and is handed no description
/// from above lays its run in, unwalked. Only subtrees with no scroll container are kept, since whether one can scroll
/// and its bounds under raised nodes follow its offset and what lies over it, which the subtree revision does not
/// cover.
#[derive(Default)]
pub(crate) struct KeptSemantics {
    runs: IdMap<NodeId, SemanticsRun>,
    /// How many nodes the last pass laid in from runs.
    pub(crate) laid: usize,
}

impl KeptSemantics {
    pub(crate) fn clear(&mut self) {
        self.runs.clear();
    }
}

/// A run being made by the walk: its kept node, the node above it in the accessibility tree, and whether it can still
/// be kept.
struct Recording {
    root: NodeId,
    outer: Option<NodeId>,
    run: SemanticsRun,
    keepable: bool,
}

/// A node's bounds: those it declared, in its own coordinates, or its whole rect.
fn declared_bounds(rect: Rect, declared: Option<Rect>) -> Rect {
    declared.map_or(rect, |b| {
        Rect::new(
            rect.origin.x + b.origin.x,
            rect.origin.y + b.origin.y,
            b.size.width,
            b.size.height,
        )
    })
}

/// [`build`], laying in the kept subtrees that did not change from `kept`'s runs and keeping a run for every other
/// kept subtree it walks. The pass is the one walking every node makes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_kept<M: 'static>(
    tree: &Tree<M>,
    root: NodeId,
    geometry: &Geometry,
    focus: Option<NodeId>,
    direction: LayoutDirection,
    scale: f64,
    raised: &[(NodeId, Rect)],
    kept: &mut KeptSemantics,
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
    let mut old = std::mem::take(&mut kept.runs);
    kept.laid = 0;
    let mut recordings: Vec<Recording> = Vec::new();
    // a subtree holding the focus says so on its nodes
    let focus_path = focus
        .filter(|&n| tree.contains(n))
        .map_or(Vec::new(), |n| tree.path(n));
    let rect_of = |id: NodeId| geometry.get(&id).map_or(Rect::default(), |p| p.rect);
    // Each fallback and where it would stand among the Tab stops, in reading order.
    let mut fallbacks = Vec::new();
    // Descriptions for the first node with a role under each node that gave one.
    let mut pending: Vec<(NodeId, String)> = Vec::new();
    // Each node with relations to keyed nodes, resolved once every node is in.
    let mut keyed = Vec::new();
    // Each entry is a node, its nearest ancestor in the accessibility tree, and the run it is recorded into.
    let mut stack = vec![(root, None::<NodeId>, None::<usize>)];
    while let Some((id, owner, recording)) = stack.pop() {
        let Some(node) = tree.get(id) else { continue };
        let scrolls = matches!(
            node.spec.kind,
            cupertino_ui_layout::ContainerKind::Scroll { .. }
        );
        let rect = rect_of(id);
        // a kept subtree lays in what it put in last time, or starts a run
        let mut recording = recording;
        if let (None, Some(mark)) = (recording, node.kept) {
            // a description handed down from above would go to its first node with a role
            let handed = !pending.is_empty() && {
                let path = tree.path(id);
                pending.iter().any(|(giver, _)| path.contains(giver))
            };
            if id != root && !scrolls && !handed && !focus_path.contains(&id) {
                if let Some(run) = old.remove(&id).filter(|run| {
                    run.revision == node.subtree_revision
                        && run.mark == mark
                        && run.direction == direction
                        && run.scale == scale
                        && run.asked.iter().all(|&(n, size)| rect_of(n).size == size)
                }) {
                    lay_run(&run, owner, &rect_of, &mut pass, &mut fallbacks, &mut keyed);
                    kept.laid += run.asked.len();
                    kept.runs.insert(id, run);
                    continue;
                }
                recordings.push(Recording {
                    root: id,
                    outer: owner,
                    run: SemanticsRun::new(node.subtree_revision, mark, direction, scale),
                    keepable: true,
                });
                recording = Some(recordings.len() - 1);
            }
        }
        let mut rec = recording.map(|r| &mut recordings[r]);
        if let Some(rec) = rec.as_deref_mut() {
            rec.keepable &= !scrolls;
            rec.run.asked.push((id, rect.size));
        }
        let mut semantics = Semantics::default();
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
            if let Some(rec) = rec {
                rec.run.hidden.push(id);
            }
            continue;
        }
        if semantics.focusable {
            pass.focusable.push(id);
            if let Some(rec) = rec.as_deref_mut() {
                rec.run.focusable.push(id);
            }
        } else if semantics.focus_fallback {
            fallbacks.push((id, pass.focusable.len()));
            if let Some(rec) = rec.as_deref_mut() {
                rec.run.fallbacks.push((id, rec.run.focusable.len()));
            }
        }
        if semantics.focus_scope {
            pass.scopes.push(id);
            if let Some(rec) = rec.as_deref_mut() {
                rec.run.scopes.push(id);
            }
        }
        if semantics.roving_group {
            pass.roving_groups.push(id);
            if let Some(rec) = rec.as_deref_mut() {
                rec.run.roving_groups.push(id);
            }
        }
        if semantics.roving_item {
            pass.roving_items.push(id);
            if let Some(rec) = rec.as_deref_mut() {
                rec.run.roving_items.push(id);
            }
        }
        let role = match semantics.role {
            Some(role) => Some(role),
            None if id == root => Some(Role::Window),
            None => None,
        };
        // what this node hands its owner leaves the run being made: its owner is the one above the kept node
        let leaves = rec
            .as_deref()
            .is_some_and(|rec| owner.is_some() && owner == rec.outer);
        let next_owner = match role {
            Some(role) => {
                if !semantics.keyed_relations.is_empty() {
                    let relations = std::mem::take(&mut semantics.keyed_relations);
                    if let Some(rec) = rec.as_deref_mut() {
                        rec.run.keyed.push((id, relations.clone()));
                    }
                    keyed.push((id, relations));
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
                let bounds = declared_bounds(rect, semantics.bounds);
                // a scroll view under raised nodes shows its content only below them; what it scrolls keeps its
                // place, so assistive technology can still find and scroll to it
                let under = scrolls
                    && geometry
                        .get(&id)
                        .is_some_and(|p| p.raised != crate::route::Raised::In);
                let bounds = if under && !raised.is_empty() {
                    crate::paint::unobscured(bounds, raised)
                } else {
                    bounds
                };
                pass.nodes.insert(
                    id,
                    AccessibilityNode {
                        role,
                        name: semantics.name,
                        description,
                        value,
                        states,
                        bounds,
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
                if let Some(rec) = rec.as_deref_mut() {
                    rec.run.accessible.push((id, semantics.bounds));
                    if leaves {
                        rec.run.outward.push(Outward::Child(id));
                    }
                }
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
                    if let Some(rec) = rec.filter(|_| leaves) {
                        rec.run
                            .outward
                            .push(Outward::Description(description.clone()));
                    }
                    let owner = pass
                        .nodes
                        .get_mut(&owner)
                        .expect("an owner is inserted before its descendants");
                    owner.description = Some(join(owner.description.take(), description));
                }
                owner
            }
        };
        // a description handed down inside a kept subtree goes to one of its descendants, all of them in its run
        if let Some(text) = semantics.content_description {
            pending.push((id, text));
        }
        stack.extend(
            node.children
                .iter()
                .rev()
                .map(|&c| (c, next_owner, recording)),
        );
    }
    // the runs take their nodes as the walk left them, before what is worked out over the whole tree
    for mut rec in recordings.into_iter().filter(|r| r.keepable) {
        rec.run.nodes = rec
            .run
            .accessible
            .iter()
            .map(|(id, _)| pass.nodes[id].clone())
            .collect();
        kept.runs.insert(rec.root, rec.run);
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

/// Lays `run` in under `owner` as the walk would have: its nodes with their bounds from where they lie now, what it
/// hands `owner`, and its entries in each list.
fn lay_run(
    run: &SemanticsRun,
    owner: Option<NodeId>,
    rect_of: &impl Fn(NodeId) -> Rect,
    pass: &mut SemanticsPass,
    fallbacks: &mut Vec<(NodeId, usize)>,
    keyed: &mut Vec<(NodeId, Vec<KeyedRelation>)>,
) {
    let base = pass.focusable.len();
    pass.focusable.extend_from_slice(&run.focusable);
    fallbacks.extend(run.fallbacks.iter().map(|&(id, at)| (id, base + at)));
    pass.scopes.extend_from_slice(&run.scopes);
    pass.roving_groups.extend_from_slice(&run.roving_groups);
    pass.roving_items.extend_from_slice(&run.roving_items);
    pass.hidden.extend_from_slice(&run.hidden);
    keyed.extend(run.keyed.iter().cloned());
    for (&(id, declared), node) in run.accessible.iter().zip(&run.nodes) {
        let mut node = node.clone();
        node.bounds = declared_bounds(rect_of(id), declared);
        pass.nodes.insert(id, node);
    }
    if let Some(owner) = owner.and_then(|o| pass.nodes.get_mut(&o)) {
        for out in &run.outward {
            match out {
                Outward::Child(id) => owner.children.push(*id),
                Outward::Description(text) => {
                    owner.description = Some(join(owner.description.take(), text.clone()));
                }
            }
        }
    }
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
