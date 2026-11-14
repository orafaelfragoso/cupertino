use super::*;
use crate::test_support::{group, leaf};
use proptest::prelude::*;

type M = String;

fn keyed(keys: &[u64]) -> Element<M> {
    Element::new(group(
        keys.iter()
            .map(|&k| Element::new(leaf("x")).key(k))
            .collect(),
    ))
}

/// A group of rows made by `Element::kept`, each `(key, mark)`, counting in `built` the rows built.
fn kept_rows(rows: &[(u64, u64)], built: &std::rc::Rc<std::cell::Cell<u32>>) -> Element<M> {
    Element::new(group(
        rows.iter()
            .map(|&(key, mark)| {
                let built = std::rc::Rc::clone(built);
                Element::kept(key, mark, move || {
                    built.set(built.get() + 1);
                    Element::new(group(vec![Element::new(leaf("x"))]))
                })
            })
            .collect(),
    ))
}

#[test]
fn a_kept_element_of_the_same_mark_keeps_its_node_unbuilt_and_a_new_mark_builds_it_again() {
    let built = std::rc::Rc::new(std::cell::Cell::new(0));
    let mut tree = Tree::<M>::new(100);
    let root = tree
        .reconcile(None, kept_rows(&[(1, 10), (2, 20)], &built))
        .unwrap();
    assert_eq!(built.get(), 2);
    let (one, two) = (child_by_key(&tree, root, 1), child_by_key(&tree, root, 2));
    let leaf_of = |tree: &Tree<M>, id: NodeId| tree.get(id).unwrap().children[0];
    let (leaf_one, leaf_two) = (leaf_of(&tree, one), leaf_of(&tree, two));
    tree.take_layout_dirty();
    tree.take_paint_dirty();

    // the same marks: nothing is built, compared or marked
    tree.reconcile(Some(root), kept_rows(&[(1, 10), (2, 20)], &built))
        .unwrap();
    assert_eq!(built.get(), 2);
    assert_eq!(
        (child_by_key(&tree, root, 1), child_by_key(&tree, root, 2)),
        (one, two)
    );
    assert_eq!((updates(&tree, leaf_one), updates(&tree, leaf_two)), (0, 0));
    assert!(!tree.take_layout_dirty());
    assert!(tree.take_paint_dirty().is_empty());

    // a new mark builds its row again and takes its node over; a new row is made; the rest stays
    tree.reconcile(Some(root), kept_rows(&[(1, 10), (2, 21), (3, 30)], &built))
        .unwrap();
    assert_eq!(built.get(), 4);
    assert_eq!(
        (child_by_key(&tree, root, 1), child_by_key(&tree, root, 2)),
        (one, two)
    );
    assert_eq!((updates(&tree, leaf_one), updates(&tree, leaf_two)), (0, 1));
    assert_eq!(tree.get(root).unwrap().children.len(), 3);

    // a row made as any element in its place is no longer kept: the next kept one is built whatever its mark
    let plain = Element::new(group(vec![
        Element::new(group(vec![Element::new(leaf("x"))])).key(1u64),
        Element::new(group(vec![Element::new(leaf("x"))])).key(2u64),
    ]));
    tree.reconcile(Some(root), plain).unwrap();
    tree.reconcile(Some(root), kept_rows(&[(1, 10), (2, 21)], &built))
        .unwrap();
    assert_eq!(built.get(), 6);
    assert_eq!(child_by_key(&tree, root, 1), one);
    assert!(tree
        .take_removed()
        .iter()
        .all(|&gone| gone != one && gone != two));
}

fn child_by_key(tree: &Tree<M>, root: NodeId, key: u64) -> NodeId {
    *tree
        .get(root)
        .unwrap()
        .children
        .iter()
        .find(|&&c| tree.get(c).unwrap().key == Some(Key::Int(key)))
        .unwrap()
}

fn updates(tree: &Tree<M>, id: NodeId) -> u32 {
    *tree.get(id).unwrap().state.downcast_ref::<u32>().unwrap()
}

#[test]
fn reordered_keyed_children_keep_their_nodes_and_state() {
    let mut tree = Tree::<M>::new(100);
    let root = tree.reconcile(None, keyed(&[1, 2, 3])).unwrap();
    let before: Vec<_> = [1, 2, 3].map(|k| child_by_key(&tree, root, k)).into();
    tree.take_layout_dirty();

    assert_eq!(tree.reconcile(Some(root), keyed(&[3, 1, 2])).unwrap(), root);

    let after: Vec<_> = [1, 2, 3].map(|k| child_by_key(&tree, root, k)).into();
    assert_eq!(before, after);
    assert_eq!(
        tree.get(root).unwrap().children,
        vec![before[2], before[0], before[1]]
    );
    assert!(after.iter().all(|&id| updates(&tree, id) == 1));
    assert!(
        tree.take_layout_dirty(),
        "a new child order is a layout change"
    );
    assert!(tree.take_removed().is_empty());
}

#[test]
fn a_new_widget_type_under_the_same_key_remounts() {
    let mut tree = Tree::<M>::new(100);
    let root = tree.reconcile(None, keyed(&[1])).unwrap();
    let old = child_by_key(&tree, root, 1);

    let replaced = Element::new(group(vec![Element::new(group(vec![])).key(1u64)]));
    tree.reconcile(Some(root), replaced).unwrap();

    let new = child_by_key(&tree, root, 1);
    assert_ne!(old, new);
    assert!(!tree.contains(old));
    assert_eq!(tree.take_removed(), vec![old]);
}

#[test]
fn siblings_sharing_a_key_keep_a_stable_node_count_and_the_first_keeps_its_node() {
    let mut tree = Tree::<M>::new(100);
    let root = tree.reconcile(None, keyed(&[1, 1, 2])).unwrap();
    let first = tree.get(root).unwrap().children[0];
    for _ in 0..2 {
        tree.reconcile(Some(root), keyed(&[1, 1, 2])).unwrap();
        assert_eq!(tree.len(), 4);
        assert_eq!(tree.get(root).unwrap().children[0], first);
    }
}

#[test]
fn unkeyed_children_match_by_position_and_the_tail_unmounts_with_its_subtree() {
    let mut tree = Tree::<M>::new(100);
    let view = |n: usize| {
        Element::new(group(
            (0..n)
                .map(|_| Element::new(group(vec![Element::new(leaf("x"))])))
                .collect(),
        ))
    };
    let root = tree.reconcile(None, view(3)).unwrap();
    let first = tree.get(root).unwrap().children.clone();
    assert_eq!(tree.len(), 7);

    tree.reconcile(Some(root), view(1)).unwrap();

    assert_eq!(tree.get(root).unwrap().children, vec![first[0]]);
    assert_eq!(tree.len(), 3);
    let removed = tree.take_removed();
    assert_eq!(removed.len(), 4);
    assert!(removed.contains(&first[1]) && removed.contains(&first[2]));
}

#[test]
fn a_freed_slot_is_reused_under_a_new_generation() {
    let mut tree = Tree::<M>::new(100);
    let root = tree.reconcile(None, keyed(&[1])).unwrap();
    let old = child_by_key(&tree, root, 1);
    tree.reconcile(Some(root), keyed(&[])).unwrap();
    tree.reconcile(Some(root), keyed(&[2])).unwrap();

    let new = child_by_key(&tree, root, 2);
    assert_eq!(new.index(), old.index());
    assert_ne!(new.generation(), old.generation());
    assert!(tree.get(old).is_none());
}

#[test]
fn the_node_ceiling_is_a_typed_error() {
    let mut tree = Tree::<M>::new(3);
    assert_eq!(
        tree.reconcile(None, keyed(&[1, 2, 3])).err(),
        Some(UiError::NodeLimit(3))
    );
    assert_eq!(tree.len(), 3);
}

#[test]
fn a_layout_change_reaches_every_ancestor_and_a_paint_change_stays_put() {
    let mut tree = Tree::<M>::new(100);
    let view = |label| {
        Element::new(group(vec![Element::new(group(vec![Element::new(leaf(
            label,
        ))]))]))
    };
    let root = tree.reconcile(None, view("a")).unwrap();
    let middle = tree.get(root).unwrap().children[0];
    let leaf_id = tree.get(middle).unwrap().children[0];
    let generations =
        |tree: &Tree<M>| [root, middle, leaf_id].map(|id| tree.get(id).unwrap().generation);
    let before = generations(&tree);
    tree.take_paint_dirty();

    tree.reconcile(Some(root), view("b")).unwrap();
    assert_eq!(generations(&tree), before, "a label change is paint only");
    assert_eq!(
        tree.take_paint_dirty().into_iter().collect::<Vec<_>>(),
        vec![leaf_id]
    );

    tree.mark(leaf_id, Change::Layout);
    let after = generations(&tree);
    assert!(before.iter().zip(&after).all(|(b, a)| a > b));
}

proptest! {
    #[test]
    fn keyed_reconciliation_keeps_exactly_the_surviving_keys(
        old in proptest::collection::btree_set(0u64..40, 0..20),
        new in proptest::collection::btree_set(0u64..40, 0..20),
        order in any::<u64>(),
    ) {
        let old: Vec<u64> = old.into_iter().collect();
        let mut new: Vec<u64> = new.into_iter().collect();
        let shift = if new.is_empty() { 0 } else { (order % new.len() as u64) as usize };
        new.rotate_left(shift);

        let mut tree = Tree::<M>::new(1000);
        let root = tree.reconcile(None, keyed(&old)).unwrap();
        let ids: HashMap<u64, NodeId> = old.iter().map(|&k| (k, child_by_key(&tree, root, k))).collect();
        tree.take_removed();

        tree.reconcile(Some(root), keyed(&new)).unwrap();

        prop_assert_eq!(tree.len(), 1 + new.len());
        for &k in &new {
            let id = child_by_key(&tree, root, k);
            match ids.get(&k) {
                Some(&kept) => prop_assert_eq!(id, kept),
                None => prop_assert!(!ids.values().any(|&old| old == id)),
            }
        }
        let mut removed = tree.take_removed();
        removed.sort();
        let mut expected: Vec<NodeId> = old.iter().filter(|k| !new.contains(k)).map(|k| ids[k]).collect();
        expected.sort();
        prop_assert_eq!(removed, expected);
        for id in tree.preorder(root) {
            if let Some(parent) = tree.get(id).unwrap().parent {
                prop_assert!(tree.get(parent).unwrap().children.contains(&id));
            }
        }
    }
}
