use crate::support;

use cupertino_geometry::{Insets, Rect, Size};
use cupertino_ui_controls::{Image, ImageFit, ImageSource, Stack};
use cupertino_ui_core::{Application, Command, Element, Window};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_platform::WindowEvent;
use cupertino_ui_render::{ImageResource, PaintOp, PixelFormat, ResourceId, Shape};
use support::{window, Harness};

fn source(id: u64, width: u32, height: u32) -> ImageSource {
    ImageSource {
        id: ResourceId(id),
        image: ImageResource {
            revision: 1,
            width,
            height,
            stride: width * 4,
            format: PixelFormat::Rgba8Premultiplied,
            pixels: vec![255; (width * height * 4) as usize].into(),
        },
    }
}

struct Pictures<const COVER: bool, const RADIUS: u8>;

impl<const COVER: bool, const RADIUS: u8> Application for Pictures<COVER, RADIUS> {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Start,
            children: vec![Element::new(Image {
                sources: vec![source(2, 80, 80), source(1, 40, 40)],
                size: Size::new(80.0, 40.0),
                fit: if COVER {
                    ImageFit::Cover
                } else {
                    ImageFit::Contain
                },
                radius: f64::from(RADIUS),
                label: Some("Photo".into()),
            })
            .key("image")],
        }))
    }
}

/// What the image node drew: each image's resource and box, and each clip.
fn drawn<A: Application>(h: &Harness<A>) -> Vec<(Option<u64>, Rect)> {
    h.ops("image")
        .into_iter()
        .filter_map(|op| match op {
            PaintOp::Image { image, dst, .. } => Some((Some(image.id.0), dst)),
            PaintOp::PushClip(Shape::Rect(rect)) => Some((None, rect)),
            PaintOp::PushClip(Shape::RoundedRect { rect, .. }) => Some((None, rect)),
            _ => None,
        })
        .collect()
}

#[test]
fn contain_shows_the_whole_image_centred_without_a_clip() {
    let h = Harness::<Pictures<false, 0>>::new();
    assert_eq!(drawn(&h), [(Some(1), Rect::new(20.0, 0.0, 40.0, 40.0))]);
}

#[test]
fn cover_fills_the_box_and_clips_what_falls_outside() {
    let h = Harness::<Pictures<true, 0>>::new();
    assert_eq!(
        drawn(&h),
        [
            (None, Rect::new(0.0, 0.0, 80.0, 40.0)),
            (Some(2), Rect::new(0.0, -20.0, 80.0, 80.0)),
        ]
    );
}

#[test]
fn a_radius_rounds_the_image_it_shows() {
    let h = Harness::<Pictures<false, 6>>::new();
    let clip = h.ops("image").into_iter().find_map(|op| match op {
        PaintOp::PushClip(shape) => Some(shape),
        _ => None,
    });
    assert_eq!(
        clip,
        Some(Shape::RoundedRect {
            rect: Rect::new(20.0, 0.0, 40.0, 40.0),
            radius: 6.0,
        })
    );
}

#[test]
fn the_rendition_drawn_is_the_smallest_that_covers_the_device_pixels() {
    let mut h = Harness::<Pictures<false, 0>>::new();
    assert_eq!(drawn(&h)[0].0, Some(1));
    h.send(WindowEvent::Scale(2.0));
    h.settle();
    assert_eq!(drawn(&h)[0].0, Some(2));
    h.send(WindowEvent::Scale(3.0));
    h.settle();
    assert_eq!(drawn(&h)[0].0, Some(2), "past the largest, the largest");
}

#[test]
fn a_labelled_image_is_an_image_to_assistive_technology() {
    let h = Harness::<Pictures<false, 0>>::assistive();
    let node = h.accessible("image");
    assert_eq!(
        (node.role, node.name.as_deref()),
        (cupertino_ui_core::Role::Image, Some("Photo"))
    );
}
