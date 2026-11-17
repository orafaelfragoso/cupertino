//! Items are collected per layer and flattened front to back, so the topmost layer draws first in the list.

use cupertino_scene::{LayerStack, SceneLayer};

#[test]
fn layers_are_ordered_bottom_to_top() {
    use SceneLayer::*;
    let bottom_to_top = [
        Wallpaper,
        LayerBackground,
        LayerBottom,
        OverviewBackdrop,
        BackdropCapture,
        Windows,
        SnapPreview,
        LayerTop,
        Popups,
        Banners,
        InputHud,
        AccentPopup,
        LayerOverlay,
        DragIcon,
        DisplayFilter,
        Cursor,
        CompositorOverlay,
        SessionOverlay,
    ];
    assert!(bottom_to_top.windows(2).all(|w| w[0] < w[1]));
}

#[test]
fn flattening_lists_the_topmost_layer_first_whatever_the_push_order() {
    let mut stack = LayerStack::new();
    stack.push(SceneLayer::Windows, "window");
    stack.push(SceneLayer::Cursor, "cursor");
    stack.push(SceneLayer::Wallpaper, "wallpaper");
    stack.push(SceneLayer::Popups, "popup");
    assert_eq!(stack.flatten(), ["cursor", "popup", "window", "wallpaper"]);
}

#[test]
fn within_a_layer_the_callers_order_is_kept() {
    let mut stack = LayerStack::new();
    stack.push(SceneLayer::Windows, 1);
    stack.extend(SceneLayer::Windows, [2, 3]);
    stack.push(SceneLayer::Popups, 10);
    stack.push(SceneLayer::Windows, 4);
    assert_eq!(stack.flatten(), [10, 1, 2, 3, 4]);
}

#[test]
fn flattening_with_layers_names_each_items_layer() {
    let mut stack = LayerStack::new();
    stack.push(SceneLayer::Windows, 'a');
    stack.push(SceneLayer::SessionOverlay, 'b');
    stack.push(SceneLayer::Windows, 'c');
    assert_eq!(
        stack.flatten_with_layer(),
        [
            (SceneLayer::SessionOverlay, 'b'),
            (SceneLayer::Windows, 'a'),
            (SceneLayer::Windows, 'c'),
        ]
    );
}

#[test]
fn length_counts_items_across_layers() {
    let mut stack: LayerStack<u8> = LayerStack::default();
    assert!(stack.is_empty());
    assert_eq!(stack.len(), 0);
    stack.push(SceneLayer::Cursor, 1);
    stack.extend(SceneLayer::Windows, [2, 3]);
    stack.extend(SceneLayer::Popups, []);
    assert!(!stack.is_empty());
    assert_eq!(stack.len(), 3);
}

#[test]
fn an_empty_stack_flattens_to_nothing() {
    let stack: LayerStack<u8> = LayerStack::new();
    assert!(stack.flatten().is_empty());
}
