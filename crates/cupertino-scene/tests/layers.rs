//! Items are collected per layer and flattened front to back, so the topmost layer draws first in the list.

use cupertino_scene::{LayerStack, SceneLayer};

#[test]
fn flattening_lists_layers_top_to_bottom_and_keeps_the_callers_order_within_a_layer() {
    use SceneLayer::*;
    let top_to_bottom = [
        SessionOverlay,
        CompositorOverlay,
        Cursor,
        DisplayFilter,
        DragIcon,
        LayerOverlay,
        Banners,
        Popups,
        LayerTop,
        SnapPreview,
        Windows,
        BackdropCapture,
        OverviewBackdrop,
        LayerBottom,
        LayerBackground,
        Wallpaper,
    ];
    let mut stack = LayerStack::new();
    for layer in top_to_bottom.iter().rev().step_by(2) {
        stack.push(*layer, *layer);
    }
    for layer in top_to_bottom.iter().step_by(2) {
        stack.push(*layer, *layer);
    }
    assert_eq!(stack.flatten(), top_to_bottom, "push order does not matter");

    let mut stack = LayerStack::new();
    stack.push(Windows, 1);
    stack.extend(Windows, [2, 3]);
    stack.push(Popups, 10);
    stack.push(Windows, 4);
    assert_eq!(
        stack.flatten_with_layer(),
        [
            (Popups, 10),
            (Windows, 1),
            (Windows, 2),
            (Windows, 3),
            (Windows, 4)
        ]
    );
}

#[test]
fn length_counts_items_across_layers() {
    let mut stack: LayerStack<u8> = LayerStack::default();
    assert!(stack.is_empty());
    stack.push(SceneLayer::Cursor, 1);
    stack.extend(SceneLayer::Windows, [2, 3]);
    stack.extend(SceneLayer::Popups, []);
    assert!(!stack.is_empty());
    assert_eq!(stack.len(), 3);
}
