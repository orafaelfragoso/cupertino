//! Captures of the output by a program with the `capture` role (`capture::once`), against the stand-in server's
//! capture globals: a frame of a rotated or flipped output arrives in its buffer turned, as the server sends it, and
//! the program gets the upright picture, a region of it cut where the region is on the upright output.

mod support;

use std::sync::Mutex;

use cupertino_kit::core::{Application, Command, Window, WindowKey};
use cupertino_kit::geometry::{Rect, Size};
use cupertino_kit::render::ImageResource;
use cupertino_kit_shell::capture::{self, Options, Source};
use cupertino_kit_shell::hosting::Kind;
use cupertino_kit_shell::{
    outputs, run, Anchor, Error, Event, Keyboard, Layer, LayerSpec, Margin, Program, Role, Surface,
};
use support::capture::place_of;
use support::display::{Offer, OUTPUT_SIZE};
use support::{environment_lock, Server};
use wayland_server::protocol::wl_output::Transform;

/// Every transform an output can have.
const TRANSFORMS: [Transform; 8] = [
    Transform::Normal,
    Transform::_90,
    Transform::_180,
    Transform::_270,
    Transform::Flipped,
    Transform::Flipped90,
    Transform::Flipped180,
    Transform::Flipped270,
];

/// The region the capturing program asks for, in the output's logical points.
static REGION: Mutex<Option<Rect>> = Mutex::new(None);
/// What the capturing program's capture gave.
static CAPTURED: Mutex<Option<Result<ImageResource, Error>>> = Mutex::new(None);

/// A program with no window that captures the first output once, in the region [`REGION`] names, and exits.
struct Capturer;

#[derive(Clone, Debug)]
struct Captured;

impl Application for Capturer {
    type Model = ();
    type Message = Captured;
    const RESIDENT: bool = true;

    fn init() -> ((), Vec<Command<Captured>>) {
        let output = outputs::all()
            .into_iter()
            .next()
            .expect("the stand-in's output");
        let options = Options {
            cursor: false,
            region: *REGION.lock().unwrap(),
        };
        let capture = capture::once(Source::Output(output.id), options, |result| {
            *CAPTURED.lock().unwrap() = Some(result);
            Captured
        });
        ((), vec![capture])
    }

    fn update(_: &mut (), Captured: Captured) -> Vec<Command<Captured>> {
        vec![Command::Exit]
    }

    fn view(_: &()) -> Vec<Window<Captured>> {
        Vec::new()
    }
}

impl Program for Capturer {
    fn surface(_: &(), _: &WindowKey) -> Surface {
        Surface {
            role: Role::Layer(LayerSpec {
                namespace: "test-capturer",
                output: None,
                layer: Layer::Overlay,
                anchor: Anchor::default(),
                size: Size::new(1.0, 1.0),
                margin: Margin::default(),
                exclusive_zone: 0,
                keyboard: Keyboard::None,
            }),
            persistent: None,
            holes: Vec::new(),
        }
    }

    fn event(_: Event) -> Option<Captured> {
        None
    }
}

/// The image the program captures of `region` of the stand-in's output, whose frames come turned by `transform`.
fn capture(transform: Transform, region: Option<Rect>) -> ImageResource {
    use std::os::fd::IntoRawFd;

    let offer = Offer {
        capture: Some(transform),
        ..Offer::CURRENT
    };
    let (server, _host_end) = Server::start_offering(Kind::Shared, &[], offer);
    let client = server.client();
    let environment = cupertino_kit::app::environment().expect("toolkit environment");
    let _lock = environment_lock();
    *REGION.lock().unwrap() = region;
    std::env::set_var("WAYLAND_SOCKET", client.into_raw_fd().to_string());
    run::<Capturer>(environment).expect("the program runs and exits");
    std::env::remove_var("WAYLAND_SOCKET");
    CAPTURED
        .lock()
        .unwrap()
        .take()
        .expect("the capture finished")
        .unwrap_or_else(|e| panic!("{transform:?}: the capture failed: {e}"))
}

/// Where in the output's upright picture the pixel at `(x, y)` of `image` was.
fn place(image: &ImageResource, (x, y): (u32, u32)) -> (u32, u32) {
    let at = (y * image.stride + x * 4) as usize;
    let pixel = &image.pixels[at..at + 4];
    place_of([pixel[0], pixel[1], pixel[2], pixel[3]])
}

#[test]
fn a_region_of_a_rotated_or_flipped_output_is_cut_from_its_upright_picture() {
    let region = Rect::new(100.0, 50.0, 300.0, 200.0);
    for transform in TRANSFORMS {
        let image = capture(transform, Some(region));
        assert_eq!((image.width, image.height), (300, 200), "{transform:?}");
        for y in 0..image.height {
            for x in 0..image.width {
                assert_eq!(
                    place(&image, (x, y)),
                    (100 + x, 50 + y),
                    "{transform:?}: pixel {x}, {y}"
                );
            }
        }
    }
}

#[test]
fn a_whole_rotated_output_is_captured_upright() {
    let image = capture(Transform::_90, None);
    assert_eq!((image.width, image.height), OUTPUT_SIZE);
    let (right, bottom) = (OUTPUT_SIZE.0 - 1, OUTPUT_SIZE.1 - 1);
    for corner in [(0, 0), (right, 0), (0, bottom), (right, bottom), (960, 540)] {
        assert_eq!(place(&image, corner), corner);
    }
}

#[test]
fn a_region_off_the_output_ends_the_capture_with_why() {
    use std::os::fd::IntoRawFd;

    let offer = Offer {
        capture: Some(Transform::_270),
        ..Offer::CURRENT
    };
    let (server, _host_end) = Server::start_offering(Kind::Shared, &[], offer);
    let client = server.client();
    let environment = cupertino_kit::app::environment().expect("toolkit environment");
    let _lock = environment_lock();
    *REGION.lock().unwrap() = Some(Rect::new(4000.0, 4000.0, 10.0, 10.0));
    std::env::set_var("WAYLAND_SOCKET", client.into_raw_fd().to_string());
    run::<Capturer>(environment).expect("the program runs and exits");
    std::env::remove_var("WAYLAND_SOCKET");
    let captured = CAPTURED
        .lock()
        .unwrap()
        .take()
        .expect("the capture finished");
    assert!(captured.is_err(), "nothing of the output is in the region");
}
