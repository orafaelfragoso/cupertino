//! A scene frame flattens its layers front to back and refuses a material composite without its capture below it.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Duration;

use cupertino_geometry::{CornerStyle, Rect};
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

fn pass(stage: MaterialStage) -> Source {
    Source::Material(MaterialPass {
        recipe: RecipeId(ResourceKey {
            slot: 1,
            generation: 1,
        }),
        rect: OutputRect(Rect::new(0.0, 0.0, 100.0, 50.0)),
        radius: 8.0,
        style: CornerStyle::Circular,
        stage,
        tint: [1.0; 3],
        blur_ramp: [1.0, 1.0],
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
fn a_frame_lists_its_items_front_to_back() {
    let mut stack = LayerStack::new();
    stack.push(
        SceneLayer::Wallpaper,
        item(SceneKind::Wallpaper, 0, pass(MaterialStage::Full)),
    );
    stack.push(
        SceneLayer::Cursor,
        item(SceneKind::CursorImage, 2, pass(MaterialStage::Full)),
    );
    stack.push(
        SceneLayer::Windows,
        item(SceneKind::WindowContent, 1, pass(MaterialStage::Full)),
    );
    let frame = frame(stack);
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
fn a_material_composite_needs_its_own_capture_below_it() {
    for (name, capture_owner, capture_layer, composite_layer, accepted) in [
        (
            "its capture below",
            7,
            SceneLayer::BackdropCapture,
            SceneLayer::Popups,
            true,
        ),
        (
            "another owner's capture",
            8,
            SceneLayer::BackdropCapture,
            SceneLayer::Popups,
            false,
        ),
        (
            "its capture above",
            7,
            SceneLayer::Popups,
            SceneLayer::BackdropCapture,
            false,
        ),
    ] {
        let mut stack = LayerStack::new();
        stack.push(
            capture_layer,
            item(
                SceneKind::BackdropCapture,
                0,
                pass(MaterialStage::Capture {
                    owner: capture_owner,
                }),
            ),
        );
        stack.push(
            composite_layer,
            item(
                SceneKind::Backdrop,
                1,
                pass(MaterialStage::Composite { owner: 7 }),
            ),
        );
        let built = catch_unwind(AssertUnwindSafe(|| frame(stack)));
        // The check is a debug assertion: a release build draws what it is given.
        assert_eq!(built.is_ok(), accepted || !cfg!(debug_assertions), "{name}");
    }
}
