//! The conversation with polkit's setuid helper, the only part that sees PAM. The agent names the user and hands over
//! polkitd's cookie; the helper then sends one line per PAM message and, last, `SUCCESS` or `FAILURE`, which it has
//! already told polkitd itself. The agent only relays what the person types.
//!
//! Newer polkit runs the helper as a socket-activated service at [`SOCKET`]; otherwise, or when nothing answers there
//! (a distribution that ships the socket disabled), the agent starts the helper binary from a fixed path. Either way
//! the cookie travels on the stream, never on a command line.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use zeroize::Zeroizing;

/// Where polkit's socket-activated helper listens.
pub const SOCKET: &str = "/run/polkit/agent-helper.socket";
/// Where distributions install the helper binary, tried in order when there is no socket.
pub const BINARIES: [&str; 2] = [
    "/usr/lib/polkit-1/polkit-agent-helper-1",
    "/usr/libexec/polkit-agent-helper-1",
];
/// The longest line the helper may send, newline included; PAM's own messages are far shorter.
pub const MAX_LINE: usize = 4096;

/// How to reach the helper.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    Socket(PathBuf),
    Binary(PathBuf),
}

impl Route {
    /// The routes to try in order: the socket when it exists, then the first helper binary installed. Empty when
    /// polkit has neither.
    pub fn find() -> Vec<Route> {
        Self::find_in(Path::new(SOCKET), &BINARIES.map(Path::new))
    }

    fn find_in(socket: &Path, binaries: &[&Path]) -> Vec<Route> {
        let socket = socket.exists().then(|| Route::Socket(socket.to_owned()));
        let binary = binaries
            .iter()
            .find(|path| path.is_file())
            .map(|path| Route::Binary(path.to_path_buf()));
        socket.into_iter().chain(binary).collect()
    }
}

/// One line from the helper.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    /// PAM asks for something; `echo` is false for a secret such as a password.
    Prompt {
        text: String,
        echo: bool,
    },
    /// PAM's error message, such as an expired password.
    Error(String),
    /// PAM's information, such as a fingerprint reader's hint.
    Info(String),
    Success,
    Failure,
}

#[derive(Debug)]
pub enum HelperError {
    Io(io::Error),
    /// The helper closed the stream before its last line.
    Closed,
    /// A line longer than [`MAX_LINE`].
    TooLong,
    /// A line the protocol has no meaning for.
    Unknown,
}

impl From<io::Error> for HelperError {
    fn from(error: io::Error) -> Self {
        HelperError::Io(error)
    }
}

impl std::fmt::Display for HelperError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HelperError::Io(error) => write!(f, "talking to polkit's helper: {error}"),
            HelperError::Closed => f.write_str("polkit's helper stopped before it answered"),
            HelperError::TooLong => write!(f, "polkit's helper sent a line over {MAX_LINE} bytes"),
            HelperError::Unknown => f.write_str("polkit's helper sent a line of no known kind"),
        }
    }
}

impl std::error::Error for HelperError {}

/// The meaning of one line without its newline. The helper escapes each message as GLib's `g_strescape` does.
pub fn parse(line: &[u8]) -> Result<Line, HelperError> {
    let line = unescape(line);
    let message = |prefix: &str| {
        line.strip_prefix(prefix.as_bytes())
            .map(|rest| String::from_utf8_lossy(rest).into_owned())
    };
    if let Some(text) = message("PAM_PROMPT_ECHO_OFF ") {
        Ok(Line::Prompt { text, echo: false })
    } else if let Some(text) = message("PAM_PROMPT_ECHO_ON ") {
        Ok(Line::Prompt { text, echo: true })
    } else if let Some(text) = message("PAM_ERROR_MSG ") {
        Ok(Line::Error(text))
    } else if let Some(text) = message("PAM_TEXT_INFO ") {
        Ok(Line::Info(text))
    } else if line.starts_with(b"SUCCESS") {
        Ok(Line::Success)
    } else if line.starts_with(b"FAILURE") {
        Ok(Line::Failure)
    } else {
        Err(HelperError::Unknown)
    }
}

/// GLib's `g_strcompress`: C escapes and up to three octal digits; any other escaped byte stands for itself.
fn unescape(line: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(line.len());
    let mut bytes = line.iter().copied().peekable();
    while let Some(b) = bytes.next() {
        if b != b'\\' {
            out.push(b);
            continue;
        }
        let Some(escaped) = bytes.next() else { break };
        out.push(match escaped {
            b'b' => 0x08,
            b'f' => 0x0c,
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'v' => 0x0b,
            b'0'..=b'7' => {
                let mut value = u32::from(escaped - b'0');
                for _ in 0..2 {
                    match bytes.peek() {
                        Some(&d @ b'0'..=b'7') => {
                            value = value * 8 + u32::from(d - b'0');
                            bytes.next();
                        }
                        _ => break,
                    }
                }
                value as u8
            }
            other => other,
        });
    }
    out
}

/// The helper's side of one authentication: read its lines with [`Helper::read_line`], answer its prompts with
/// [`Helper::answer`], or [`Helper::split`] it to read on one thread and answer from another. Dropping the [`Answers`]
/// ends the conversation: a helper the agent started is stopped, and a read waiting on the socket returns.
pub struct Helper {
    lines: Lines,
    answers: Answers,
}

/// The reading half of a [`Helper`].
pub struct Lines(BufReader<Box<dyn Read + Send>>);

/// The answering half of a [`Helper`]; dropping it ends the conversation.
pub struct Answers {
    writer: Box<dyn Write + Send>,
    child: Option<Child>,
    socket: Option<UnixStream>,
}

impl Helper {
    /// Starts authenticating `user` for polkitd's `cookie` through the first of `routes` that answers. A started
    /// helper gets an empty environment, no descriptors but its pipes and stderr discarded; only the user is on its
    /// command line.
    pub fn start(routes: &[Route], user: &str, cookie: &str) -> io::Result<Helper> {
        let mut failed = io::Error::new(io::ErrorKind::NotFound, "polkit has no agent helper");
        for route in routes {
            match Self::start_on(route, user, cookie) {
                Ok(helper) => return Ok(helper),
                Err(error) => failed = error,
            }
        }
        Err(failed)
    }

    fn start_on(route: &Route, user: &str, cookie: &str) -> io::Result<Helper> {
        let mut helper = match route {
            Route::Socket(path) => {
                let stream = UnixStream::connect(path)?;
                let mut helper = Helper {
                    lines: Lines(BufReader::new(Box::new(stream.try_clone()?))),
                    answers: Answers {
                        writer: Box::new(stream.try_clone()?),
                        child: None,
                        socket: Some(stream),
                    },
                };
                helper.answers.send(user.as_bytes())?;
                helper
            }
            Route::Binary(path) => {
                let mut child = Command::new(path)
                    .arg(user)
                    .env_clear()
                    .current_dir("/")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()?;
                let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
                    unreachable!("both pipes were asked for");
                };
                Helper {
                    lines: Lines(BufReader::new(Box::new(stdout))),
                    answers: Answers {
                        writer: Box::new(stdin),
                        child: Some(child),
                        socket: None,
                    },
                }
            }
        };
        helper.answers.send(cookie.as_bytes())?;
        Ok(helper)
    }

    /// The helper's next line; blocks until it comes.
    pub fn read_line(&mut self) -> Result<Line, HelperError> {
        self.lines.read_line()
    }

    /// See [`Answers::answer`].
    pub fn answer(&mut self, answer: &[u8]) -> io::Result<()> {
        self.answers.answer(answer)
    }

    pub fn split(self) -> (Lines, Answers) {
        (self.lines, self.answers)
    }
}

impl Lines {
    /// The helper's next line; blocks until it comes, or until the [`Answers`] are dropped.
    pub fn read_line(&mut self) -> Result<Line, HelperError> {
        let mut line = Vec::new();
        let read = (&mut self.0)
            .take(MAX_LINE as u64)
            .read_until(b'\n', &mut line)?;
        match line.pop() {
            None if read == 0 => Err(HelperError::Closed),
            Some(b'\n') => parse(&line),
            _ if read == MAX_LINE => Err(HelperError::TooLong),
            _ => Err(HelperError::Closed),
        }
    }
}

impl Answers {
    /// Answers the last prompt. The answer and the buffer it is written from are wiped once sent; a line break in
    /// the answer would end it early, so it is refused.
    pub fn answer(&mut self, answer: &[u8]) -> io::Result<()> {
        if answer.contains(&b'\n') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "an answer can't hold a line break",
            ));
        }
        self.send(answer)
    }

    fn send(&mut self, text: &[u8]) -> io::Result<()> {
        let mut line = Zeroizing::new(Vec::with_capacity(text.len() + 1));
        line.extend_from_slice(text);
        line.push(b'\n');
        self.writer.write_all(&line)?;
        self.writer.flush()
    }
}

impl Drop for Answers {
    fn drop(&mut self) {
        // a helper still reading an answer stops at the end of its input
        self.writer = Box::new(io::sink());
        if let Some(socket) = &self.socket {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests;
