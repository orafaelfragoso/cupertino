//! The `cupertino-testclient` process: a first-party application window for the QEMU harness.
//! It maps one window with the given application id and title, and appends what is typed into it to a file, so a
//! check can tell which window has the keyboard focus. It is not part of the desktop.

use std::error::Error;
use std::fs::File;
use std::io::Write;
use std::process::ExitCode;

use cupertino_client::{key, Client, Event, TitlebarStyle, ToplevelConfig};
use tiny_skia::Color;

const USAGE: &str = "usage: cupertino-testclient --app-id ID --title TITLE --output FILE";

/// Logical size of the window; the test checks never depend on it.
const WINDOW_SIZE: (u32, u32) = (480, 320);
/// Content grey, distinct from the desktop picture so a mapped window changes a screendump.
const FILL_GREY: u8 = 236;

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
    if code == key::ENTER {
        Some('\n')
    } else {
        character
    }
}

/// Map the window and serve it until the server asks to close it. The output file is created empty once the first
/// frame is committed, which is how a harness knows the window is up.
fn run(options: &Options) -> Result<(), Box<dyn Error>> {
    let mut client = Client::connect()?;
    client.toplevel(&ToplevelConfig {
        app_id: options.app_id.clone(),
        title: options.title.clone(),
        size: WINDOW_SIZE,
        min_size: (1, 1),
        titlebar: TitlebarStyle::Standard,
    })?;
    let mut output: Option<File> = None;
    loop {
        for event in client.wait(None)? {
            match event {
                // the client queues a draw after every configure and scale change
                Event::Draw(surface) => {
                    client.draw(surface, |pixmap, _scale| {
                        pixmap.fill(Color::from_rgba8(FILL_GREY, FILL_GREY, FILL_GREY, 255))
                    })?;
                    if output.is_none() {
                        output = Some(File::create(&options.output)?);
                    }
                }
                Event::Key {
                    key,
                    pressed: true,
                    character,
                    ..
                } => {
                    if let (Some(file), Some(c)) = (output.as_mut(), typed_text(key, character)) {
                        write!(file, "{c}")?;
                    }
                }
                Event::Closed(_) => return Ok(()),
                _ => {}
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
        assert_eq!(typed_text(key::ENTER, None), Some('\n'));
        assert_eq!(typed_text(30, Some('A')), Some('A'));
        assert_eq!(typed_text(key::ESCAPE, None), None);
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
