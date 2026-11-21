//! `kit::render::picture`: paint operations drawn into a picture on the CPU pool, which comes back to the UI thread as
//! a message.

use crate::support;

use cupertino_kit::core::{Application, Command, Environment, Window};
use cupertino_kit::geometry::{Rect, Size};
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::render::{self, ImageResource, Paint, PaintOp, PixelFormat, Shape};
use cupertino_kit::test::PlaceholderIcons;
use cupertino_kit::theme::{Appearance, Color, ThemePreferences, ThemeTokens};
use std::sync::Arc;
use std::thread::ThreadId;

/// The colour the picture's left half is filled with.
const FILL: Color = Color::new(0.0, 0.0, 1.0, 1.0);

/// The picture as it came back, and the thread it was turned into a message on.
type Painted = Option<(Option<ImageResource>, ThreadId)>;

struct Pictures;

impl Application for Pictures {
    type Model = Painted;
    type Message = (Option<ImageResource>, ThreadId);
    const RESIDENT: bool = true;

    fn init() -> (Painted, Vec<Command<Self::Message>>) {
        let env = Environment {
            theme: Arc::new(
                ThemeTokens::neutral(ThemePreferences {
                    appearance: Appearance::Light,
                    reduce_transparency: false,
                    reduce_motion: true,
                })
                .unwrap(),
            ),
            text: support::text_system(),
            direction: LayoutDirection::Ltr,
            scale: 1.0,
            icons: Arc::new(PlaceholderIcons::default()),
        };
        let ops = vec![PaintOp::Fill {
            shape: Shape::Rect(Rect::new(0.0, 0.0, 5.0, 3.0)),
            paint: Paint::Solid(FILL),
        }];
        let command = render::picture(&env, ops, Size::new(10.0, 3.0), 2.0, |picture| {
            (picture, std::thread::current().id())
        });
        (None, vec![command])
    }

    fn update(model: &mut Painted, message: Self::Message) -> Vec<Command<Self::Message>> {
        *model = Some(message);
        Vec::new()
    }

    fn view(_: &Painted) -> Vec<Window<Self::Message>> {
        Vec::new()
    }
}

#[test]
fn paint_operations_are_drawn_on_the_cpu_pool_and_the_picture_comes_back_to_the_ui_thread() {
    let mut driver = support::driver::<Pictures>(support::text_system());
    support::run_until(&mut driver, Option::is_some);
    let (picture, done_on) = driver.runtime().model().clone().unwrap();
    assert_eq!(done_on, std::thread::current().id());
    let picture = picture.expect("a picture of 20 x 6 pixels is drawn");
    assert_eq!((picture.width, picture.height), (20, 6));
    assert_eq!(picture.format, PixelFormat::Rgba8Premultiplied);
    let pixel = |x: usize, y: usize| {
        let i = y * picture.stride as usize + x * 4;
        &picture.pixels[i..i + 4]
    };
    assert_eq!(pixel(2, 3), FILL.to_rgba8());
    assert_eq!(pixel(17, 3), [0, 0, 0, 0]);
}
