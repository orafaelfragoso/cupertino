//! A privileged program of its own, as `cupertino-screenshot` is: it takes one frame of the first output with
//! [`capture::once`] and exits. The server gives it a connection with the capture role; no component of the table has
//! that role.

use std::sync::Mutex;

use cupertino_kit::core::{Application, Command, RendererClass, Window, WindowKey};
use cupertino_kit::render::ImageResource;
use cupertino_kit_shell::capture::{self, Options, Source};
use cupertino_kit_shell::{outputs, Error, Event, Program, Role, Surface};

/// The frame the program took, or why it took none.
pub static TAKEN: Mutex<Option<Result<ImageResource, Error>>> = Mutex::new(None);

pub struct Shot;

impl Application for Shot {
    type Model = ();
    type Message = Result<ImageResource, Error>;
    const RESIDENT: bool = true;
    const RENDERER: RendererClass = RendererClass::Cpu;

    fn init() -> ((), Vec<Command<Self::Message>>) {
        let first = outputs::all().first().map(|o| o.id);
        let command = match first {
            Some(output) => {
                capture::once(Source::Output(output), Options::default(), |frame| frame)
            }
            None => Command::Perform(Box::pin(async {
                Err(Error::Protocol("the program sees no output".into()))
            })),
        };
        ((), vec![command])
    }

    fn update(_: &mut (), frame: Self::Message) -> Vec<Command<Self::Message>> {
        *TAKEN.lock().unwrap() = Some(frame);
        vec![Command::Exit]
    }

    fn view(_: &()) -> Vec<Window<Self::Message>> {
        Vec::new()
    }
}

impl Program for Shot {
    fn surface(_: &(), _: &WindowKey) -> Surface {
        Surface {
            role: Role::Window,
            persistent: None,
            holes: Vec::new(),
        }
    }

    fn event(_: Event) -> Option<Self::Message> {
        None
    }
}
