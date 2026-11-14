//! The retained node tree. Each update's element tree is reconciled into it: a child is matched to an old node by
//! key, or by position among its unkeyed siblings, and kept only when the widget type is the same; everything else
//! unmounts, as do all but the first of siblings that share a key. Node ids carry their slot's generation, so an id
//! of an unmounted node never reaches its successor.

use crate::widget::{AnyWidget, Change, Element};
use crate::{Key, NodeId, UiError};
use cupertino_ui_layout::{Generation, LayoutSpec};
use std::any::Any;
use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::num::NonZeroU32;

pub(crate) struct Node<M> {
    pub(crate) parent: Option<NodeId>,
    pub(crate) children: Vec<NodeId>,
    pub(crate) key: Option<Key>,
    pub(crate) widget: Box<dyn AnyWidget<M>>,
    pub(crate) state: Box<dyn Any>,
    pub(crate) spec: LayoutSpec,
    /// Changes when this node's layout or any descendant's does.
    pub(crate) generation: Generation,
    /// Changes when this node's paint does.
    pub(crate) paint_revision: u64,
    /// Changes when this node's paint or layout, or what it holds, changes, or any descendant's does: a subtree of the
    /// same revision paints, measures and places as it did.
    pub(crate) subtree_revision: u64,
    /// The mark of the [`Element::kept`] that made the node, which an element of the same mark keeps as it is.
    pub(crate) kept: Option<u64>,
}

enum Slot<M> {
    Live(NonZeroU32, Box<Node<M>>),
    Free(NonZeroU32),
}

pub(crate) struct Tree<M> {
    slots: Vec<Slot<M>>,
    free: Vec<u32>,
    live: usize,
    limit: usize,
    revision: u64,
    removed: Vec<NodeId>,
    paint_dirty: BTreeSet<NodeId>,
    layout_dirty: bool,
    /// Keys already warned about, so a duplicate is reported once.
    duplicates: HashSet<Key>,
}

impl<M: 'static> Tree<M> {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            live: 0,
            limit,
            revision: 0,
            removed: Vec::new(),
            paint_dirty: BTreeSet::new(),
            layout_dirty: false,
            duplicates: HashSet::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.live
    }

    pub(crate) fn get(&self, id: NodeId) -> Option<&Node<M>> {
        match self.slots.get(id.index() as usize)? {
            Slot::Live(generation, node) if *generation == id.generation() => Some(node),
            _ => None,
        }
    }

    pub(crate) fn get_mut(&mut self, id: NodeId) -> Option<&mut Node<M>> {
        match self.slots.get_mut(id.index() as usize)? {
            Slot::Live(generation, node) if *generation == id.generation() => Some(node),
            _ => None,
        }
    }

    pub(crate) fn contains(&self, id: NodeId) -> bool {
        self.get(id).is_some()
    }

    fn node(&self, id: NodeId) -> &Node<M> {
        self.get(id).expect("the tree only links live nodes")
    }

    fn node_mut(&mut self, id: NodeId) -> &mut Node<M> {
        self.get_mut(id).expect("the tree only links live nodes")
    }

    /// Reconciles a window's root element with its current root node, returning the root afterwards.
    pub(crate) fn reconcile(
        &mut self,
        current: Option<NodeId>,
        element: Element<M>,
    ) -> Result<NodeId, UiError> {
        if let Some(root) = current.filter(|&root| self.keeps(root, &element)) {
            return Ok(root);
        }
        let (element, mark) = element.built();
        let root = match current {
            Some(root) if self.matches(root, &element) => {
                self.update(root, element)?;
                root
            }
            _ => {
                if let Some(root) = current {
                    self.unmount(root);
                }
                self.layout_dirty = true;
                self.mount(None, element)?
            }
        };
        self.node_mut(root).kept = mark;
        Ok(root)
    }

    /// Whether the node `id` stays as it is for `element`: both made by [`Element::kept`] with one key and one mark.
    fn keeps(&self, id: NodeId, element: &Element<M>) -> bool {
        let node = self.node(id);
        node.kept.is_some() && node.kept == element.kept_mark() && node.key == element.key
    }

    fn matches(&self, id: NodeId, element: &Element<M>) -> bool {
        let node = self.node(id);
        node.key == element.key && node.widget.widget_type() == element.widget.widget_type()
    }

    fn next_revision(&mut self) -> u64 {
        self.revision += 1;
        self.revision
    }

    fn mount(&mut self, parent: Option<NodeId>, element: Element<M>) -> Result<NodeId, UiError> {
        if self.live >= self.limit {
            return Err(UiError::NodeLimit(self.limit));
        }
        let (mut element, kept) = element.built();
        let children = element.widget.take_children();
        let revision = self.next_revision();
        let spec = element.layout();
        let node = Box::new(Node {
            parent,
            children: Vec::new(),
            key: element.key,
            state: element.widget.new_state(),
            spec,
            widget: element.widget,
            generation: Generation(revision),
            paint_revision: revision,
            subtree_revision: revision,
            kept,
        });
        let id = match self.free.pop() {
            Some(index) => {
                let slot = &mut self.slots[index as usize];
                let Slot::Free(generation) = *slot else {
                    unreachable!("the free list holds free slots only")
                };
                *slot = Slot::Live(generation, node);
                NodeId::new(index, generation)
            }
            None => {
                let index =
                    u32::try_from(self.slots.len()).map_err(|_| UiError::NodeLimit(self.limit))?;
                self.slots.push(Slot::Live(NonZeroU32::MIN, node));
                NodeId::new(index, NonZeroU32::MIN)
            }
        };
        self.live += 1;
        self.paint_dirty.insert(id);
        let mut ids = Vec::with_capacity(children.len());
        for child in children {
            ids.push(self.mount(Some(id), child)?);
        }
        self.node_mut(id).children = ids;
        Ok(id)
    }

    fn update(&mut self, id: NodeId, mut element: Element<M>) -> Result<(), UiError> {
        let children = element.widget.take_children();
        let spec = element.layout();
        let node = self.node_mut(id);
        let mut change = element.widget.update(&*node.widget, &mut *node.state);
        if spec != node.spec {
            change = Change::Layout;
        }
        node.widget = element.widget;
        node.spec = spec;
        self.mark(id, change);
        self.reconcile_children(id, children)
    }

    fn reconcile_children(
        &mut self,
        parent: NodeId,
        elements: Vec<Element<M>>,
    ) -> Result<(), UiError> {
        let old = std::mem::take(&mut self.node_mut(parent).children);
        // with each keyed child's place, so the ones left over are freed in order and slots are reused alike every run
        let mut keyed = HashMap::new();
        let mut unkeyed = VecDeque::new();
        for (at, &child) in old.iter().enumerate() {
            match self.node(child).key.clone() {
                Some(key) => match keyed.entry(key) {
                    Entry::Vacant(entry) => {
                        entry.insert((at, child));
                    }
                    Entry::Occupied(entry) => {
                        if self.duplicates.insert(entry.key().clone()) {
                            tracing::warn!(key = ?entry.key(), "siblings share a key; only the first keeps its node");
                        }
                        self.unmount(child);
                    }
                },
                None => unkeyed.push_back(child),
            }
        }
        let mut children = Vec::with_capacity(elements.len());
        let mut result = Ok(());
        for element in elements {
            let candidate = match &element.key {
                Some(key) => keyed.remove(key).map(|(_, id)| id),
                None => unkeyed.pop_front(),
            };
            // a node kept as it is: nothing of its subtree is made, compared or marked
            if let Some(old) = candidate.filter(|&old| self.keeps(old, &element)) {
                children.push(old);
                continue;
            }
            let id = match candidate {
                Some(old) => {
                    let (element, mark) = element.built();
                    let id = if self.matches(old, &element) {
                        self.update(old, element).map(|()| old)
                    } else {
                        self.unmount(old);
                        self.mount(Some(parent), element)
                    };
                    id.inspect(|&id| self.node_mut(id).kept = mark)
                }
                None => self.mount(Some(parent), element),
            };
            match id {
                Ok(id) => children.push(id),
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
        }
        let mut left: Vec<(usize, NodeId)> = keyed.into_values().collect();
        left.sort_unstable_by_key(|&(at, _)| at);
        for old in left.into_iter().map(|(_, id)| id).chain(unkeyed) {
            self.unmount(old);
        }
        // the parent places other children, but paints as it did: what it draws comes from its own props and state
        if children != old {
            self.mark_layout(parent);
        }
        self.node_mut(parent).children = children;
        result
    }

    /// Records a change of `id`: a layout change also changes the generation of every ancestor, whose size may
    /// depend on it.
    pub(crate) fn mark(&mut self, id: NodeId, change: Change) {
        if change == Change::None {
            return;
        }
        let revision = self.next_revision();
        self.paint_dirty.insert(id);
        let node = self.node_mut(id);
        node.paint_revision = revision;
        let layout = change == Change::Layout;
        self.layout_dirty |= layout;
        let mut next = Some(id);
        while let Some(id) = next {
            let node = self.node_mut(id);
            if layout {
                node.generation = Generation(revision);
            }
            node.subtree_revision = revision;
            next = node.parent;
        }
    }

    /// Records that `id` places its children differently while it paints as before: its generation and its
    /// ancestors' change, its paint revision does not.
    fn mark_layout(&mut self, id: NodeId) {
        let revision = self.next_revision();
        self.layout_dirty = true;
        let mut next = Some(id);
        while let Some(id) = next {
            let node = self.node_mut(id);
            node.generation = Generation(revision);
            node.subtree_revision = revision;
            next = node.parent;
        }
    }

    /// Frees `id` and its subtree. The caller unlinks `id` from its parent.
    pub(crate) fn unmount(&mut self, id: NodeId) {
        let mut stack = vec![id];
        while let Some(id) = stack.pop() {
            let index = id.index() as usize;
            let next = id.generation().checked_add(1);
            let freed = std::mem::replace(
                &mut self.slots[index],
                Slot::Free(next.unwrap_or(NonZeroU32::MAX)),
            );
            let Slot::Live(_, node) = freed else {
                unreachable!("only live nodes are unmounted")
            };
            // A slot whose generation would wrap is retired, so no id can ever repeat.
            if next.is_some() {
                self.free.push(id.index());
            }
            self.live -= 1;
            self.paint_dirty.remove(&id);
            self.removed.push(id);
            stack.extend(node.children);
        }
        self.layout_dirty = true;
    }

    /// The nodes unmounted since the last call, whose resources and accessibility objects go away.
    pub(crate) fn take_removed(&mut self) -> Vec<NodeId> {
        std::mem::take(&mut self.removed)
    }

    pub(crate) fn take_paint_dirty(&mut self) -> BTreeSet<NodeId> {
        std::mem::take(&mut self.paint_dirty)
    }

    pub(crate) fn take_layout_dirty(&mut self) -> bool {
        std::mem::take(&mut self.layout_dirty)
    }

    /// `root` and its descendants, parents before children, siblings in order.
    pub(crate) fn preorder(&self, root: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            out.push(id);
            stack.extend(self.node(id).children.iter().rev());
        }
        out
    }

    /// The nodes from the root down to `id`.
    pub(crate) fn path(&self, id: NodeId) -> Vec<NodeId> {
        let mut path = vec![id];
        let mut next = self.node(id).parent;
        while let Some(parent) = next {
            path.push(parent);
            next = self.node(parent).parent;
        }
        path.reverse();
        path
    }
}

#[cfg(test)]
mod tests;
