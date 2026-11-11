mod capture;
mod ipc;
mod mcp;
mod mux;
use cupertino_control_model::MAX_RUNS_PER_CONTROLLER;
use cupertino_shell_client::{ControlEvent, ControlSession, DesktopControl};
use cupertino_wayland_client::{Connection, Event, OutputId, OutputInfo};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{self, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

/// Default deadline for compositor replies and first-frame capture.
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);
/// Maximum explicit state wait; a disconnected peer always cancels it earlier.
const MAX_WAIT_MS: u64 = 30_000;
/// The control protocol version from which the compositor serves several runs on one connection.
const CONCURRENT_RUNS: u32 = 5;
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case", deny_unknown_fields)]
enum Request {
    Hello {
        version: u32,
    },
    Status,
    #[serde(rename = "request")]
    Start {
        output: String,
        capabilities: u32,
    },
    Move {
        x: f64,
        y: f64,
        revision: Option<u32>,
    },
    Button {
        code: u32,
        pressed: bool,
        revision: Option<u32>,
    },
    Key {
        code: u32,
        pressed: bool,
    },
    Scroll {
        horizontal: f64,
        vertical: f64,
        revision: Option<u32>,
    },
    /// A frame of the granted output: the whole output at its logical size, or `region` at its device pixels.
    Capture {
        region: Option<capture::Region>,
    },
    Observe,
    Action {
        window: u64,
        revision: u64,
        node: u64,
        action: cupertino_semantics::Action,
        data: Option<cupertino_semantics::Data>,
    },
    Stop,
    Resume,
    Wait {
        phase: u32,
        timeout_ms: u64,
    },
}
/// What a refusal of `cupertino_control_session_v1` tells the local client, with what to do about it.
fn refusal(code: u32) -> String {
    match code {
        0 => "request refused (stale_epoch): this run ended; read status".into(),
        1 => "request refused (not_granted): the run is not active or lacks this capability".into(),
        2 => "request refused (denied): the person turned this off for the app the request comes from; they can turn it on in Settings › Privacy › Desktop Control or Screen Capture".into(),
        3 => "request refused (stale_geometry): the layout or the window's published snapshot changed since the revision given; observe again for the current one".into(),
        4 => "request refused (outside_scope): the target is outside the granted output".into(),
        5 => "request refused (unsupported): this capability set or operation is not available".into(),
        6 => "request refused (blocked): the session is locked, an authentication prompt is shown, or the target is protected".into(),
        7 => "request refused (no_authority): the person must be asked, and no consent panel is available".into(),
        8 => "request refused (busy): another run is in progress".into(),
        9 => "request refused (invalid_input)".into(),
        // unknown_outcome must never invite a blind mutation retry
        10 => "request outcome is unknown; observe before retrying".into(),
        11 => "request refused (unidentified): the app this request comes from cannot be named".into(),
        _ => format!("request refused ({code})"),
    }
}
fn base64(bytes: &[u8]) -> String {
    // Standard PNG response base64 alphabet, with RFC 4648 padding below.
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = *chunk.get(1).unwrap_or(&0);
        let c = *chunk.get(2).unwrap_or(&0);
        out.push(ALPHABET[(a >> 2) as usize] as char);
        out.push(ALPHABET[(((a & 3) << 4) | (b >> 4)) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(((b & 15) << 2) | (c >> 6)) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(c & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}
/// The compositor over the controller's one Wayland connection.
struct Wayland {
    conn: Connection,
    control: DesktopControl,
    capturer: capture::Capturer,
}
impl mux::Desktop for Wayland {
    type Session = ControlSession;
    type Frame = capture::Capture;
    fn capacity(&self) -> usize {
        if self.control.version() >= CONCURRENT_RUNS {
            MAX_RUNS_PER_CONTROLLER
        } else {
            1
        }
    }
    fn open(&mut self) -> ControlSession {
        self.control.session(&self.conn)
    }
    fn epoch(&self, session: &ControlSession) -> u64 {
        session.epoch()
    }
    fn identify(&mut self, session: &ControlSession, peer: Option<(u32, u64)>) {
        if let Some((pid, start)) = peer.filter(|_| session.names_apps()) {
            session.identify(pid, start);
        }
    }
    fn send(
        &mut self,
        session: &ControlSession,
        serial: u32,
        command: mux::Command,
    ) -> Result<(), String> {
        use mux::Command;
        match command {
            Command::Start {
                output,
                capabilities,
            } => session
                .start(&self.conn, serial, output, capabilities)
                .map_err(|_| "output disappeared")?,
            Command::Motion { revision, x, y } => session.motion(serial, revision, x, y),
            Command::Button {
                revision,
                code,
                pressed,
            } => session.button(serial, revision, code, pressed),
            Command::Key {
                revision,
                code,
                pressed,
            } => session.key(serial, revision, code, pressed),
            Command::Axis {
                revision,
                horizontal,
                vertical,
            } => session.axis(serial, revision, horizontal, vertical),
            Command::Observe => session.observe(serial),
            Command::Perform(invocation) => session
                .perform(serial, &invocation)
                .map_err(|_| "invalid semantic action")?,
            Command::Resume => session.resume(serial),
        }
        Ok(())
    }
    fn stop(&mut self, session: &ControlSession) {
        session.stop();
    }
    fn close(&mut self, session: ControlSession) {
        drop(session);
        let _ = self.conn.flush();
    }
    fn outputs(&self) -> Vec<OutputInfo> {
        self.conn.outputs()
    }
    fn capture(
        &mut self,
        session: &ControlSession,
        output: OutputId,
    ) -> Result<capture::Capture, String> {
        // the source the capture creates next belongs to this run, not to another the controller serves
        session.select_capture();
        self.capturer.start(&self.conn, output)
    }
    fn progress(&self, frame: &capture::Capture) -> (bool, bool, bool) {
        frame.progress()
    }
    fn prepare(&mut self, frame: &mut capture::Capture) -> Result<(), String> {
        self.capturer.prepare(&self.conn, frame)
    }
    fn pixels(&mut self, frame: capture::Capture) -> Result<capture::Pixels, String> {
        frame.pixels()
    }
    fn wait(&mut self, timeout: Option<Duration>) -> Result<Vec<ControlEvent>, String> {
        let events = self
            .conn
            .wait(timeout)
            .map_err(|_| "Wayland connection ended")?;
        Ok(events
            .into_iter()
            .filter_map(|event| match event {
                Event::Extension(event) => event.downcast_ref::<ControlEvent>().cloned(),
                _ => None,
            })
            .collect())
    }
}
fn serve() -> Result<(), String> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .ok_or("XDG_RUNTIME_DIR is missing")?;
    let display = std::env::var("WAYLAND_DISPLAY").map_err(|_| "WAYLAND_DISPLAY is missing")?;
    let listener = ipc::Listener::bind(&runtime, &display).map_err(|e| e.to_string())?;
    let mut conn = Connection::connect().map_err(|_| "cannot connect to Wayland")?;
    let control =
        DesktopControl::bind(&mut conn).ok_or("native desktop-control protocol is unavailable")?;
    let capturer =
        capture::Capturer::bind(&mut conn).ok_or("native capture protocol is unavailable")?;
    let mut desktop = Wayland {
        conn,
        control,
        capturer,
    };
    let capacity = mux::Desktop::capacity(&desktop);
    let accepting = listener.socket.try_clone().map_err(|e| e.to_string())?;
    // new peers wait here only until the loop takes them; one more than the loop serves is turned away at once
    let (pending, accepted) = std::sync::mpsc::sync_channel(capacity);
    let waker = desktop.conn.waker();
    let wake: ipc::Wake = std::sync::Arc::new(move || waker.wake());
    let accept_wake = wake.clone();
    std::thread::spawn(move || {
        for stream in accepting.incoming() {
            match pending.try_send(stream) {
                Ok(()) => accept_wake(),
                Err(std::sync::mpsc::TrySendError::Full(Ok(stream))) => {
                    ipc::refuse(stream, &mux::turned_away(capacity))
                }
                Err(std::sync::mpsc::TrySendError::Full(Err(_))) => {}
                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => break,
            }
        }
    });
    mux::run(&mut desktop, &accepted, &wake)
}
fn frontend(session: String) -> Result<(), String> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .ok_or("XDG_RUNTIME_DIR is missing")?;
    let mut stream =
        UnixStream::connect(ipc::socket_path(&runtime, &session).map_err(|e| e.to_string())?)
            .map_err(|_| "this session has no native controller")?;
    let mut responses = io::BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    ipc::write_json(&mut stream, &json!({"op":"hello","version":1})).map_err(|e| e.to_string())?;
    let greeting =
        ipc::read_line(&mut responses, ipc::MAX_RESPONSE_BYTES).map_err(|e| e.to_string())?;
    let greeting: Value =
        serde_json::from_slice(&greeting).map_err(|_| "invalid native handshake")?;
    if greeting["ok"] != true {
        // a controller that serves as many clients as it can says so instead of the greeting
        return Err(greeting["error"]
            .as_str()
            .unwrap_or("native handshake refused")
            .to_owned());
    }
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    loop {
        let line = ipc::read_line(&mut input, ipc::MAX_REQUEST_BYTES).map_err(|e| e.to_string())?;
        if line.is_empty() {
            break;
        }
        stream.write_all(&line).map_err(|e| e.to_string())?;
        let response =
            ipc::read_line(&mut responses, ipc::MAX_RESPONSE_BYTES).map_err(|e| e.to_string())?;
        if response.is_empty() {
            return Err("native controller disconnected".into());
        }
        output
            .write_all(&response)
            .and_then(|_| output.flush())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let result=match args.as_slice(){
    [serve_arg]if serve_arg=="--serve"=>serve(),
    [flag]if flag=="--mcp"=>std::env::var("WAYLAND_DISPLAY")
        .map_err(|_|"WAYLAND_DISPLAY is missing; use --mcp DISPLAY_NAME to select a session".to_owned())
        .and_then(|session|mcp::serve(&session)),
    [flag,session]if flag=="--mcp"=>mcp::serve(session),
    [flag,session]if flag=="--session"=>frontend(session.clone()),
    _=>Err("usage: cupertino-control --session WAYLAND_DISPLAY (JSON lines), --mcp [WAYLAND_DISPLAY] (MCP stdio; defaults to the host session), or --serve (compositor-owned)".into()),
};
    if let Err(error) = result {
        eprintln!("cupertino-control: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_base64_has_correct_padding() {
        for (bytes, text) in [
            (b"".as_slice(), ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg=="),
        ] {
            assert_eq!(base64(bytes), text)
        }
    }
    #[test]
    fn local_requests_reject_unknown_fields_and_arbitrary_commands() {
        assert!(serde_json::from_str::<Request>(
            r#"{"op":"key","code":30,"pressed":true,"shell":"bad"}"#
        )
        .is_err());
        assert!(serde_json::from_str::<Request>(r#"{"op":"exec","command":"bad"}"#).is_err());
    }
    #[test]
    fn refusals_name_their_reason_and_what_to_do() {
        assert!(refusal(2).contains("Settings › Privacy › Desktop Control or Screen Capture"));
        assert!(refusal(7).contains("no consent panel"));
        assert!(refusal(11).contains("(unidentified)"));
        assert!(
            !refusal(10).contains("refused"),
            "an unknown outcome is no refusal: the mutation may have happened"
        );
        assert_eq!(refusal(99), "request refused (99)");
        for code in 0..=11 {
            assert!(!refusal(code).is_empty());
        }
    }
}
