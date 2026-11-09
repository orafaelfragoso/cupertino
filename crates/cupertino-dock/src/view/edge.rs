//! The edge strips: on every output the dock is not on, a strip one point deep along the dock's edge that draws
//! nothing and reports the pointer coming onto it and leaving it. The pointer resting against that edge moves the dock
//! to that output.

use cupertino_kit::core::{
    CommandRegistry, Element, Key, Role, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::Size;
use cupertino_kit::platform::{DecorationMode, WindowSpec};

use super::paint::Root;
use super::{Input, Pointer, EDGE_APP_ID};
use crate::config::Orientation;

/// Depth of a strip in points.
pub const DEPTH: f64 = 1.0;

/// What assistive technology calls a strip.
pub const TITLE: &str = "Dock edge";

pub fn key(output: u32) -> WindowKey {
    WindowKey(Key::from(format!("edge-{output}")))
}

/// The strip of the output with key `output`, `size` points large, along the dock's edge.
pub fn window<M: Clone + 'static>(
    output: u32,
    size: Size,
    orientation: Orientation,
    input: fn(Input) -> M,
) -> Window<M> {
    let size = if orientation.is_horizontal() {
        Size::new(size.width, DEPTH)
    } else {
        Size::new(DEPTH, size.height)
    };
    Window {
        key: key(output),
        spec: WindowSpec {
            title: TITLE.to_owned(),
            app_id: format!("{EDGE_APP_ID}@{output}"),
            size,
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: CommandRegistry::new(),
        on_close: Some(input(Input::Closed(key(output)))),
        // shown without a frame of the output's width: it draws nothing
        background: WindowBackground::INPUT_ONLY,
        content: Element::new(Root {
            role: Role::Group,
            name: TITLE.to_owned(),
            pointer: Some(Box::new(move |p| match p {
                Pointer::Leave => input(Input::Edge(output, false)),
                _ => input(Input::Edge(output, true)),
            })),
            key: None,
            roving: false,
            on_blur: None,
            alpha: 1.0,
            revision: 0,
            under: None,
            frame: None,
            children: Vec::new(),
        }),
        popup: None,
        menus: None,
    }
}
