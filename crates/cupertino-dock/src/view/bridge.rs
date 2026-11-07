//! The application switcher's panel and the overview's chrome, which the process still paints itself: each is a Kit
//! window whose content is one image the host painted at the output's scale, and which reports the pointer (and the
//! overview the keys) to the model. They move to Kit views of their own with the window-manager surfaces.

use cupertino_kit::controls::ImageSource;
use cupertino_kit::core::{
    CommandRegistry, Element, Key, Role, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Rect, Size};
use cupertino_kit::platform::{DecorationMode, WindowSpec};
use cupertino_kit::render::Sampling;
use cupertino_kit::theme::Surface;

use super::paint::Root;
use super::{Input, OVERVIEW_APP_ID, SWITCHER_APP_ID};

/// What assistive technology calls the two windows.
pub const SWITCHER_TITLE: &str = "Application Switcher";
pub const OVERVIEW_TITLE: &str = "Overview";

pub fn switcher_key() -> WindowKey {
    WindowKey(Key::from("switcher"))
}

pub fn overview_key() -> WindowKey {
    WindowKey(Key::from("overview"))
}

/// A painted surface: its size in points, the image (none before its first paint) and the opacity of its material.
pub struct Painted {
    pub output: u32,
    pub size: Size,
    pub image: Option<ImageSource>,
    pub alpha: f64,
    /// What the panel shows, for assistive technology.
    pub name: String,
}

fn spec(title: &str, app_id: &str, output: u32, size: Size, input: bool) -> WindowSpec {
    WindowSpec {
        title: title.to_owned(),
        app_id: format!("{app_id}@{output}"),
        size,
        min_size: None,
        max_size: None,
        decorations: DecorationMode::None,
        foreign_parent: None,
        input_region: (!input).then(Vec::new),
    }
}

fn under(image: Option<ImageSource>, size: Size) -> super::paint::Underlay {
    Box::new(move |cx| {
        if let Some(image) = &image {
            let dst = Rect::new(0.0, 0.0, size.width, size.height);
            cx.image(image.id, &image.image, dst, Sampling::Linear);
        }
    })
}

/// The switcher's panel, the server's material under it, centred on its output.
pub fn switcher<M: Clone + 'static>(
    painted: Painted,
    radius: f64,
    input: fn(Input) -> M,
) -> Window<M> {
    let revision = painted.image.as_ref().map_or(0, |i| i.id.0);
    Window {
        key: switcher_key(),
        spec: spec(
            SWITCHER_TITLE,
            SWITCHER_APP_ID,
            painted.output,
            painted.size,
            true,
        ),
        shortcuts: CommandRegistry::new(),
        on_close: Some(input(Input::Closed(switcher_key()))),
        background: if painted.image.is_some() {
            WindowBackground::Surface {
                surface: Surface::Switcher,
                radius,
                alpha: painted.alpha,
            }
        } else {
            WindowBackground::None
        },
        content: Element::new(Root {
            role: Role::List,
            name: painted.name,
            pointer: Some(Box::new(move |p| input(Input::Switcher(p)))),
            key: None,
            roving: false,
            on_blur: None,
            alpha: 1.0,
            revision,
            under: Some(under(painted.image, painted.size)),
            children: Vec::new(),
        }),
        popup: None,
        menus: None,
    }
}

/// The overview's chrome over the whole output, with the keyboard while it is open.
pub fn overview<M: Clone + 'static>(painted: Painted, input: fn(Input) -> M) -> Window<M> {
    let revision = painted.image.as_ref().map_or(0, |i| i.id.0);
    Window {
        key: overview_key(),
        spec: spec(
            OVERVIEW_TITLE,
            OVERVIEW_APP_ID,
            painted.output,
            painted.size,
            true,
        ),
        shortcuts: CommandRegistry::new(),
        on_close: Some(input(Input::Closed(overview_key()))),
        background: WindowBackground::None,
        content: Element::new(Root {
            role: Role::Group,
            name: painted.name,
            pointer: Some(Box::new(move |p| input(Input::Overview(p)))),
            key: Some(Box::new(move |k| Some(input(Input::OverviewKey(k))))),
            roving: false,
            on_blur: None,
            alpha: 1.0,
            revision,
            under: Some(under(painted.image, painted.size)),
            children: Vec::new(),
        }),
        popup: None,
        menus: None,
    }
}
