//! The `cupertino-testclient` process: a first-party application window for the QEMU harness.
//! It maps one window with the given application id and title, and appends what is typed into it to a file, so a
//! check can tell which window has the keyboard focus. With `--frames FILE` it draws a new frame at every frame callback
//! and replaces FILE with how many it presented (never seen empty), so a check can tell that the server keeps releasing its buffers. With
//! `--intervals FILE` it animates the same way and writes the time between consecutive presented frames, in
//! microseconds, one line each, so a soak can follow frame pacing. With `--copy TEXT` its first key press sets the
//! clipboard to TEXT, and with `--clipboard FILE` it writes every text selection it is offered to FILE, so a check can
//! copy and paste between it and other programs. It is not part of the desktop.

use std::error::Error;
use std::fs::File;
use std::io::{LineWriter, Write};
use std::process::ExitCode;
use std::time::Duration;

use cupertino_wayland_client::{
    BufferFormat, Connection, Decorations, Error as TransportError, Event, KeyboardEvent,
    Presented, Toplevel, ToplevelConfig,
};

const USAGE: &str =
    "usage: cupertino-testclient --app-id ID --title TITLE --output FILE [--frames FILE] \
                     [--intervals FILE] [--copy TEXT] [--clipboard FILE]";

/// Logical size of the window when the server leaves it to the client; the test checks never depend on it.
const WINDOW_SIZE: (u32, u32) = (480, 320);
/// Content grey, distinct from the desktop picture so a mapped window changes a screendump.
const FILL_GREY: u8 = 236;
/// How long an animating window waits for the server to release a buffer before it tries again.
const RETRY: Duration = Duration::from_millis(16);
/// The text types a selection is offered and read in, the preferred first.
const TEXT_TYPES: [&str; 3] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"];
/// Evdev code of the Return key.
const KEY_ENTER: u32 = 28;

struct Options {
    app_id: String,
    title: String,
    output: String,
    frames: Option<String>,
    intervals: Option<String>,
    copy: Option<String>,
    clipboard: Option<String>,
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Options, String> {
        let (mut app_id, mut title, mut output, mut frames, mut intervals, mut copy, mut clipboard) =
            Default::default();
        let mut args = args;
        while let Some(flag) = args.next() {
            let slot = match flag.as_str() {
                "--app-id" => &mut app_id,
                "--title" => &mut title,
                "--output" => &mut output,
                "--frames" => &mut frames,
                "--intervals" => &mut intervals,
                "--copy" => &mut copy,
                "--clipboard" => &mut clipboard,
                other => return Err(format!("unknown argument {other}")),
            };
            *slot = Some(args.next().ok_or_else(|| format!("{flag} needs a value"))?);
        }
        Ok(Options {
            app_id: app_id.ok_or("--app-id is required")?,
            title: title.ok_or("--title is required")?,
            output: output.ok_or("--output is required")?,
            frames,
            intervals,
            copy,
            clipboard,
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

/// Fill the window with `grey` and present it; `false` when the server still holds every buffer, so the next frame
/// callback has to retry.
fn draw(conn: &mut Connection, toplevel: Toplevel, grey: u8) -> Result<bool, TransportError> {
    let mut lease = match conn.lease(toplevel.surface(), WINDOW_SIZE, BufferFormat::Xrgb8888) {
        Ok(lease) => lease,
        Err(TransportError::NoFreeBuffer) => return Ok(false),
        Err(e) => return Err(e),
    };
    lease.pixels_mut().fill(grey);
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
        foreign_parent: None,
    })?;
    let mut output: Option<File> = None;
    let (mut configured, mut dirty, mut blocked) = (false, false, false);
    let mut presented: u64 = 0;
    let animating = options.frames.is_some() || options.intervals.is_some();
    let mut intervals = match &options.intervals {
        Some(path) => Some(LineWriter::new(File::create(path)?)),
        None => None,
    };
    let mut last_presented: Option<u64> = None;
    let mut copy = options.copy.clone();
    loop {
        for event in conn.wait(blocked.then_some(RETRY))? {
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
                    if let Some(text) = copy.take() {
                        conn.set_selection(
                            TEXT_TYPES
                                .iter()
                                .map(|mime| (mime.to_string(), text.clone().into_bytes()))
                                .collect(),
                        )?;
                    }
                }
                Event::Selection(Some(offer)) => {
                    let mime = TEXT_TYPES
                        .iter()
                        .find(|mime| offer.mime_types().iter().any(|m| m == *mime));
                    if let (Some(path), Some(mime)) = (&options.clipboard, mime) {
                        std::fs::write(path, conn.receive(&offer, mime)?)?;
                    }
                }
                Event::Frame(_) if animating => dirty = true,
                Event::Presented(Presented { time_ns, .. }) => {
                    if let (Some(file), Some(last)) = (intervals.as_mut(), last_presented) {
                        writeln!(file, "{}", time_ns.saturating_sub(last) / 1000)?;
                    }
                    last_presented = Some(time_ns);
                }
                Event::Close(_) => return Ok(()),
                _ => {}
            }
        }
        if configured && dirty {
            let grey = if animating {
                FILL_GREY - (presented % 16) as u8
            } else {
                FILL_GREY
            };
            blocked = !draw(&mut conn, toplevel, grey)?;
            if !blocked {
                dirty = false;
                presented += 1;
                if output.is_none() {
                    output = Some(File::create(&options.output)?);
                }
                if let Some(frames) = &options.frames {
                    // a reader between the truncate and the write of the file itself would read no count
                    let partial = format!("{frames}.partial");
                    std::fs::write(&partial, presented.to_string())?;
                    std::fs::rename(&partial, frames)?;
                }
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
        assert_eq!((parsed.frames, parsed.intervals), (None, None));
        assert_eq!((parsed.copy, parsed.clipboard), (None, None));
        let animating = Options::parse(args(
            "--app-id one --title Two --output /tmp/x --frames /tmp/f",
        ))
        .unwrap();
        assert_eq!(animating.frames.as_deref(), Some("/tmp/f"));
        let pacing = Options::parse(args(
            "--app-id one --title Two --output /tmp/x --intervals /tmp/i",
        ))
        .unwrap();
        assert_eq!(
            (pacing.frames.as_deref(), pacing.intervals.as_deref()),
            (None, Some("/tmp/i"))
        );
        let clipboard = Options::parse(args(
            "--app-id one --title Two --output /tmp/x --copy hello --clipboard /tmp/c",
        ))
        .unwrap();
        assert_eq!(
            (clipboard.copy.as_deref(), clipboard.clipboard.as_deref()),
            (Some("hello"), Some("/tmp/c"))
        );
        assert_eq!(
            Options::parse(args("--app-id one --title Two"))
                .err()
                .as_deref(),
            Some("--output is required")
        );
    }
}
