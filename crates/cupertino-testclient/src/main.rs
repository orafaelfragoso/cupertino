//! The `cupertino-testclient` process: a first-party application window for the QEMU harness.
//! It maps one window with the given application id and title, and appends what is typed into it to a file, so a
//! check can tell which window has the keyboard focus. It is not part of the desktop.

use std::error::Error;
use std::fs::File;
use std::io::Write;
use std::process::ExitCode;

use cupertino_wayland_client::{
    BufferFormat, Connection, Decorations, Error as TransportError, Event, KeyboardEvent, Toplevel,
    ToplevelConfig,
};

const USAGE: &str = "usage: cupertino-testclient --app-id ID --title TITLE --output FILE";

/// Logical size of the window when the server leaves it to the client; the test checks never depend on it.
const WINDOW_SIZE: (u32, u32) = (480, 320);
/// Content grey, distinct from the desktop picture so a mapped window changes a screendump.
const FILL_GREY: u8 = 236;
/// Evdev code of the Return key.
const KEY_ENTER: u32 = 28;

struct Options {
    app_id: String,
    title: String,
    output: String,
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Options, String> {
        let (mut app_id, mut title, mut output) = (None, None, None);
        let mut args = args;
        while let Some(flag) = args.next() {
            let slot = match flag.as_str() {
                "--app-id" => &mut app_id,
                "--title" => &mut title,
                "--output" => &mut output,
                other => return Err(format!("unknown argument {other}")),
            };
            *slot = Some(args.next().ok_or_else(|| format!("{flag} needs a value"))?);
        }
        Ok(Options {
            app_id: app_id.ok_or("--app-id is required")?,
            title: title.ok_or("--title is required")?,
            output: output.ok_or("--output is required")?,
        })
    }
}

/// What a key press adds to the output file: the character it types, or a newline for Return.
fn typed_text(code: u32, character: Option<char>) -> Option<char> {
    if code == KEY_ENTER {
        Some('\n')
    } else {
        character
    }
}

/// Fill the window grey and present it; `false` when the server still holds every buffer, so the next frame callback
/// has to retry.
fn draw(conn: &mut Connection, toplevel: Toplevel) -> Result<bool, TransportError> {
    let mut lease = match conn.lease(toplevel.surface(), WINDOW_SIZE, BufferFormat::Xrgb8888) {
        Ok(lease) => lease,
        Err(TransportError::NoFreeBuffer) => return Ok(false),
        Err(e) => return Err(e),
    };
    lease.pixels_mut().fill(FILL_GREY);
    lease.present()?;
    Ok(true)
}

/// Map the window and serve it until the server asks to close it. The output file is created empty once the first
/// frame is committed, which is how a harness knows the window is up.
fn run(options: &Options) -> Result<(), Box<dyn Error>> {
    let mut conn = Connection::connect()?;
    let toplevel = conn.toplevel(&ToplevelConfig {
        app_id: options.app_id.clone(),
        title: options.title.clone(),
        min_size: Some((1, 1)),
        max_size: None,
        decorations: Decorations::Server,
    })?;
    let mut output: Option<File> = None;
    let (mut configured, mut dirty) = (false, false);
    loop {
        for event in conn.wait(None)? {
            match event {
                Event::Configure { .. } => (configured, dirty) = (true, true),
                Event::ScaleChanged { .. } => dirty = true,
                Event::Keyboard {
                    event:
                        KeyboardEvent::Key {
                            code,
                            pressed: true,
                            character,
                            ..
                        },
                    ..
                } => {
                    if let (Some(file), Some(c)) = (output.as_mut(), typed_text(code, character)) {
                        write!(file, "{c}")?;
                    }
                }
                Event::Close(_) => return Ok(()),
                _ => {}
            }
        }
        if configured && dirty && draw(&mut conn, toplevel)? {
            dirty = false;
            if output.is_none() {
                output = Some(File::create(&options.output)?);
            }
        }
    }
}

fn main() -> ExitCode {
    let options = match Options::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(problem) => {
            eprintln!("cupertino-testclient: {problem}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match run(&options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cupertino-testclient: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_types_a_newline_and_other_keys_their_character() {
        assert_eq!(typed_text(KEY_ENTER, None), Some('\n'));
        assert_eq!(typed_text(30, Some('A')), Some('A'));
        const KEY_ESC: u32 = 1;
        assert_eq!(typed_text(KEY_ESC, None), None);
    }

    #[test]
    fn options_need_all_three_flags() {
        let args = |s: &str| {
            s.split(' ')
                .map(String::from)
                .collect::<Vec<_>>()
                .into_iter()
        };
        let parsed = Options::parse(args("--app-id one --title Two --output /tmp/x")).unwrap();
        assert_eq!(
            (
                parsed.app_id.as_str(),
                parsed.title.as_str(),
                parsed.output.as_str()
            ),
            ("one", "Two", "/tmp/x")
        );
        assert_eq!(
            Options::parse(args("--app-id one --title Two"))
                .err()
                .as_deref(),
            Some("--output is required")
        );
    }
}
