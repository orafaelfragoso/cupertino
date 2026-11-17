//! A scene frame flattens its layers front to back and refuses a material composite without its capture below it.

use std::time::Duration;

use cupertino_geometry::{CornerStyle, Point, Rect};
use cupertino_scene::{
    ContentDamage, FrameId, GlobalRect, LayerStack, MaterialPass, MaterialStage, Output, OutputId,
    OutputInvalidation, OutputRect, PhysicalSize, Placement, RecipeId, ResourceKey, SceneFrame,
    SceneId, SceneItem, SceneKind, SceneLayer, Source, Transform,
};

fn output() -> Output {
    Output {
        id: OutputId(1),
        rect: GlobalRect(Rect::new(0.0, 0.0, 1920.0, 1080.0)),
        physical_size: PhysicalSize {
            width: 3840,
            height: 2160,
        },
        scale: 2.0,
        transform: Transform::Normal,
    }
}

fn rect() -> OutputRect {
    OutputRect(Rect::new(0.0, 0.0, 100.0, 50.0))
}

fn pass(stage: MaterialStage) -> Source {
    Source::Material(MaterialPass {
        recipe: RecipeId(ResourceKey {
            slot: 1,
            generation: 1,
        }),
        rect: rect(),
        radius: 8.0,
        style: CornerStyle::Circular,
        stage,
        tint: [1.0; 3],
    })
}

fn item(kind: SceneKind, index: u64, source: Source) -> SceneItem {
    SceneItem {
        id: SceneId { kind, index },
        revision: 1,
        source,
        placement: Placement::IDENTITY,
        opacity: 1.0,
        clip: cupertino_scene::Clip::None,
        damage: ContentDamage::Unchanged,
    }
}

fn frame(items: LayerStack<SceneItem>) -> SceneFrame {
    SceneFrame::new(
        output(),
        FrameId(9),
        Duration::from_millis(16),
        [0.0, 0.0, 0.0, 1.0],
        OutputInvalidation::Incremental,
        items,
    )
}

#[test]
fn a_frame_carries_its_header_and_lists_items_front_to_back() {
    let mut stack = LayerStack::new();
    stack.push(
        SceneLayer::Wallpaper,
        item(SceneKind::Wallpaper, 0, pass(MaterialStage::Full)),
    );
    stack.push(
        SceneLayer::Windows,
        item(SceneKind::WindowContent, 1, pass(MaterialStage::Full)),
    );
    stack.push(
        SceneLayer::Cursor,
        item(SceneKind::CursorImage, 2, pass(MaterialStage::Full)),
    );
    let frame = frame(stack);
    assert_eq!(frame.id, FrameId(9));
    assert_eq!(frame.timestamp, Duration::from_millis(16));
    assert_eq!(frame.invalidation, OutputInvalidation::Incremental);
    assert_eq!(frame.output, output());
    let order: Vec<(SceneLayer, u64)> = frame
        .items()
        .iter()
        .map(|(layer, item)| (*layer, item.id.index))
        .collect();
    assert_eq!(
        order,
        [
            (SceneLayer::Cursor, 2),
            (SceneLayer::Windows, 1),
            (SceneLayer::Wallpaper, 0),
        ]
    );
}

#[test]
fn a_composite_with_its_capture_below_it_is_accepted() {
    let mut stack = LayerStack::new();
    stack.push(
        SceneLayer::BackdropCapture,
        item(
            SceneKind::BackdropCapture,
            0,
            pass(MaterialStage::Capture { owner: 7 }),
        ),
    );
    stack.push(
        SceneLayer::Popups,
        item(
            SceneKind::Backdrop,
            1,
            pass(MaterialStage::Composite { owner: 7 }),
        ),
    );
    assert_eq!(frame(stack).items().len(), 2);
}

#[test]
#[cfg_attr(debug_assertions, should_panic(expected = "capture below it"))]
fn a_composite_with_the_capture_of_another_owner_is_refused() {
    let mut stack = LayerStack::new();
    stack.push(
        SceneLayer::BackdropCapture,
        item(
            SceneKind::BackdropCapture,
            0,
            pass(MaterialStage::Capture { owner: 8 }),
        ),
    );
    stack.push(
        SceneLayer::Popups,
        item(
            SceneKind::Backdrop,
            1,
            pass(MaterialStage::Composite { owner: 7 }),
        ),
    );
    frame(stack);
}

#[test]
#[cfg_attr(debug_assertions, should_panic(expected = "capture below it"))]
fn a_capture_above_its_composite_is_refused() {
    let mut stack = LayerStack::new();
    stack.push(
        SceneLayer::Popups,
        item(
            SceneKind::BackdropCapture,
            0,
            pass(MaterialStage::Capture { owner: 7 }),
        ),
    );
    stack.push(
        SceneLayer::BackdropCapture,
        item(
            SceneKind::Backdrop,
            1,
            pass(MaterialStage::Composite { owner: 7 }),
        ),
    );
    frame(stack);
}

#[test]
fn scene_kinds_have_distinct_stable_keys() {
    use std::collections::HashSet;
    let kinds = [
        SceneKind::Wallpaper,
        SceneKind::WindowContent,
        SceneKind::TitleBar,
        SceneKind::MinimizeMesh,
        SceneKind::DisplayFilter,
        SceneKind::AccentShadow,
        SceneKind::InputHudMaterial,
    ];
    let keys: HashSet<&str> = kinds.iter().map(|k| k.key()).collect();
    assert_eq!(keys.len(), kinds.len());
    assert_eq!(SceneKind::WindowContent.key(), "window-content");
    assert_eq!(SceneKind::BackdropCapture.key(), "backdrop-capture");
    assert!(kinds.iter().all(|k| {
        let key = k.key();
        !key.is_empty() && key.chars().all(|c| c.is_ascii_lowercase() || c == '-')
    }));
}

#[test]
fn the_identity_placement_does_not_move_or_scale() {
    let p = Placement::IDENTITY;
    assert_eq!(p.origin.0, Point::ZERO);
    assert_eq!(p.pivot.0, Point::ZERO);
    assert_eq!((p.scale_x, p.scale_y), (1.0, 1.0));
}
