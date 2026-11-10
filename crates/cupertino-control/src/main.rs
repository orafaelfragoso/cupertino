mod capture;
mod ipc;
mod mcp;
use cupertino_shell_client::{ControlEvent, ControlSession, DesktopControl};
use cupertino_wayland_client::{Connection, Event, OutputId};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{self, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// Default deadline for compositor replies and first-frame capture.
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);
/// Maximum explicit state wait; a disconnected peer always cancels it earlier.
const MAX_WAIT_MS: u64 = 30_000;
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
    Capture,
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
#[derive(Default)]
struct Remote {
    epoch: u64,
    available: u32,
    phase: u32,
    revision: u32,
    output: Option<OutputId>,
    geometry: Value,
    reply: Option<(u32, Result<(), u32>)>,
    observation: Option<(u32, cupertino_semantics::Observation)>,
    action_reply: Option<(u32, cupertino_semantics::ActionReply)>,
}
impl Remote {
    fn observe(&mut self, events: Vec<Event>, run: &ControlSession) {
        for event in events {
            let Event::Extension(event) = event else {
                continue;
            };
            let Some(event) = event.downcast_ref::<ControlEvent>() else {
                continue;
            };
            match event {
                ControlEvent::Created { epoch, available } if *epoch == run.epoch() => {
                    self.epoch = *epoch;
                    self.available = *available;
                }
                ControlEvent::Status { epoch, phase, .. } if *epoch == run.epoch() => {
                    self.phase = *phase
                }
                ControlEvent::Geometry {
                    epoch,
                    revision,
                    x,
                    y,
                    width,
                    height,
                    scale,
                    name,
                } if *epoch == run.epoch() => {
                    self.revision = *revision;
                    self.geometry = json!({"revision":revision,"x":x,"y":y,"width":width,"height":height,"scale":scale,"name":name});
                }
                ControlEvent::Observation {
                    epoch,
                    serial,
                    data,
                } if *epoch == run.epoch() => self.observation = Some((*serial, data.clone())),
                ControlEvent::ActionFinished {
                    epoch,
                    serial,
                    reply,
                } if *epoch == run.epoch() => self.action_reply = Some((*serial, reply.clone())),
                ControlEvent::Completed { epoch, serial } if *epoch == run.epoch() => {
                    self.reply = Some((*serial, Ok(())))
                }
                ControlEvent::Refused {
                    epoch,
                    serial,
                    reason,
                } if *epoch == run.epoch() => self.reply = Some((*serial, Err(*reason))),
                _ => {}
            }
        }
    }
    fn snapshot(&self) -> Value {
        json!({"epoch":self.epoch,"available":self.available,"phase":self.phase,"geometry":self.geometry})
    }
}
struct ClientRun<'a> {
    conn: &'a mut Connection,
    session: ControlSession,
    inbox: ipc::Inbox,
    remote: Remote,
    next: u32,
}
impl ClientRun<'_> {
    fn pump(&mut self, timeout: Option<Duration>) -> Result<(), String> {
        if self.inbox.closed.load(Ordering::SeqCst) {
            return Err("local client disconnected".into());
        }
        let events = self
            .conn
            .wait(timeout)
            .map_err(|_| "Wayland connection ended")?;
        self.remote.observe(events, &self.session);
        if self.inbox.closed.load(Ordering::SeqCst) {
            return Err("local client disconnected".into());
        }
        Ok(())
    }
    fn until(
        &mut self,
        deadline: Instant,
        mut ready: impl FnMut(&Remote) -> bool,
    ) -> Result<(), String> {
        while !ready(&self.remote) {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or("operation timed out")?;
            self.pump(Some(remaining))?;
        }
        Ok(())
    }
    fn serial(&mut self) -> Result<u32, String> {
        let s = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or("request identifiers exhausted")?;
        Ok(s)
    }
    fn ack(&mut self, serial: u32) -> Result<(), String> {
        self.ack_timeout(serial, REPLY_TIMEOUT)
    }
    fn ack_timeout(&mut self, serial: u32, timeout: Duration) -> Result<(), String> {
        self.until(Instant::now() + timeout, |r| {
            r.reply.is_some_and(|(s, _)| s == serial)
        })?;
        self.remote.reply.unwrap().1.map_err(|r| {
            // cupertino_control_session_v1.refusal.unknown_outcome must never invite a blind mutation retry.
            if r == 10 {
                "request outcome is unknown; observe before retrying".into()
            } else {
                format!("request refused ({r})")
            }
        })
    }
    fn execute(&mut self, request: Request, capturer: &capture::Capturer) -> Result<Value, String> {
        self.pump(Some(Duration::ZERO))?;
        match request {
            Request::Hello { version: 1 } => Ok(
                json!({"schema":"cupertino.control.v1","protocol":3,"state":self.remote.snapshot()}),
            ),
            Request::Hello { .. } => Err("unsupported local protocol version".into()),
            Request::Status => {
                let mut state = self.remote.snapshot();
                state["outputs"] = json!(self
                    .conn
                    .outputs()
                    .iter()
                    .filter_map(|o| o.name.clone())
                    .collect::<Vec<_>>());
                Ok(state)
            }
            Request::Observe => {
                let serial = self.serial()?;
                self.remote.reply = None;
                self.session.observe(serial);
                self.ack(serial)?;
                self.remote
                    .observation
                    .as_ref()
                    .filter(|(s, _)| *s == serial)
                    .map(|(_, o)| serde_json::to_value(o).unwrap())
                    .ok_or("observation did not arrive".into())
            }
            Request::Action {
                window,
                revision,
                node,
                action,
                data,
            } => {
                let serial = self.serial()?;
                self.remote.reply = None;
                self.session
                    .perform(
                        serial,
                        &cupertino_semantics::Invocation {
                            window,
                            revision,
                            node,
                            action,
                            data,
                        },
                    )
                    .map_err(|_| "invalid semantic action")?;
                self.ack_timeout(serial, REPLY_TIMEOUT + Duration::from_secs(1))?;
                let reply = self
                    .remote
                    .action_reply
                    .as_ref()
                    .filter(|(s, _)| *s == serial)
                    .map(|(_, r)| r)
                    .ok_or("action outcome is unknown; observe before retrying")?;
                serde_json::to_value(reply).map_err(|_| "invalid semantic result".into())
            }
            Request::Start {
                output,
                capabilities,
            } => {
                let o = self
                    .conn
                    .outputs()
                    .into_iter()
                    .find(|o| o.name.as_deref() == Some(output.as_str()))
                    .ok_or("output is not present")?
                    .id;
                let s = self.serial()?;
                self.remote.reply = None;
                self.session
                    .start(self.conn, s, o, capabilities)
                    .map_err(|_| "output disappeared")?;
                self.ack(s)?;
                self.remote.output = Some(o);
                Ok(self.remote.snapshot())
            }
            Request::Wait { phase, timeout_ms } => {
                if timeout_ms > MAX_WAIT_MS {
                    return Err("state wait exceeds deadline limit".into());
                }
                self.until(Instant::now() + Duration::from_millis(timeout_ms), |r| {
                    r.phase == phase
                })?;
                Ok(self.remote.snapshot())
            }
            Request::Stop => {
                self.session.stop();
                self.until(Instant::now() + REPLY_TIMEOUT, |r| r.phase == 4)?;
                Ok(self.remote.snapshot())
            }
            Request::Capture => {
                if self.remote.phase != 2 {
                    return Err("desktop control is not active".into());
                }
                let revision = self.remote.revision;
                let mut frame =
                    capturer.start(self.conn, self.remote.output.ok_or("no granted output")?)?;
                let deadline = Instant::now() + REPLY_TIMEOUT;
                while !frame.progress().0 && !frame.progress().2 {
                    self.pump(Some(
                        deadline
                            .checked_duration_since(Instant::now())
                            .ok_or("capture timed out")?,
                    ))?;
                }
                if frame.progress().2 {
                    return Err("capture grant is unavailable".into());
                }
                capturer.prepare(self.conn, &mut frame)?;
                while !frame.progress().1 && !frame.progress().2 {
                    self.pump(Some(
                        deadline
                            .checked_duration_since(Instant::now())
                            .ok_or("capture timed out")?,
                    ))?;
                }
                if self.remote.phase != 2 || frame.progress().2 {
                    return Err("capture grant ended".into());
                }
                if self.remote.revision != revision {
                    return Err("output geometry changed during capture; observe again".into());
                }
                let (w, h, png) = frame.png()?;
                Ok(
                    json!({"mime_type":"image/png","width":w,"height":h,"geometry":self.remote.geometry,"data":base64(&png)}),
                )
            }
            Request::Resume => {
                let s = self.serial()?;
                self.remote.reply = None;
                self.session.resume(s);
                self.ack(s)?;
                Ok(self.remote.snapshot())
            }
            request => {
                let s = self.serial()?;
                self.remote.reply = None;
                let revision = self.remote.revision;
                match request {
                    Request::Move {
                        x,
                        y,
                        revision: expected,
                    } => self.session.motion(s, expected.unwrap_or(revision), x, y),
                    Request::Button {
                        code,
                        pressed,
                        revision: expected,
                    } => self
                        .session
                        .button(s, expected.unwrap_or(revision), code, pressed),
                    Request::Key { code, pressed } => self.session.key(s, revision, code, pressed),
                    Request::Scroll {
                        horizontal,
                        vertical,
                        revision: expected,
                    } => self
                        .session
                        .axis(s, expected.unwrap_or(revision), horizontal, vertical),
                    _ => unreachable!(),
                }
                self.ack(s)?;
                Ok(self.remote.snapshot())
            }
        }
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
fn serve() -> Result<(), String> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .ok_or("XDG_RUNTIME_DIR is missing")?;
    let display = std::env::var("WAYLAND_DISPLAY").map_err(|_| "WAYLAND_DISPLAY is missing")?;
    let listener = ipc::Listener::bind(&runtime, &display).map_err(|e| e.to_string())?;
    let mut conn = Connection::connect().map_err(|_| "cannot connect to Wayland")?;
    let control =
        DesktopControl::bind(&mut conn).ok_or("native desktop-control protocol is unavailable")?;
    let capture =
        capture::Capturer::bind(&mut conn).ok_or("native capture protocol is unavailable")?;
    let accepting = listener.socket.try_clone().map_err(|e| e.to_string())?;
    let (pending, accepted) = std::sync::mpsc::sync_channel(1);
    let wake = conn.waker();
    std::thread::spawn(move || {
        for stream in accepting.incoming() {
            match pending.try_send(stream) {
                Ok(()) => wake.wake(),
                Err(std::sync::mpsc::TrySendError::Full(_)) => {}
                Err(_) => break,
            }
        }
    });
    loop {
        let mut stream = match accepted.try_recv() {
            Ok(stream) => stream.map_err(|e| e.to_string())?,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                conn.wait(None).map_err(|_| "Wayland connection ended")?;
                continue;
            }
            Err(_) => return Err("local listener ended".into()),
        };
        let inbox = match ipc::read_peer(&stream, conn.waker()) {
            Ok(inbox) => inbox,
            Err(_) => continue,
        };
        let session = control.session(&conn);
        let mut run = ClientRun {
            conn: &mut conn,
            session,
            inbox,
            remote: Remote::default(),
            next: 1,
        };
        let result = (|| -> Result<(), String> {
            run.until(Instant::now() + REPLY_TIMEOUT, |r| r.epoch != 0)?;
            let mut hello = false;
            loop {
                if let Ok(line) = run.inbox.commands.try_recv() {
                    let request = serde_json::from_slice::<Request>(&line);
                    let result = match request {
                        Ok(Request::Hello { version: 1 }) => {
                            hello = true;
                            run.execute(Request::Hello { version: 1 }, &capture)
                        }
                        Ok(_) if !hello => Err("local protocol handshake required".into()),
                        Ok(request) => run.execute(request, &capture),
                        Err(_) => Err("invalid local command".into()),
                    };
                    let value = match result {
                        Ok(data) => json!({"ok":true,"data":data}),
                        Err(error) => json!({"ok":false,"error":error}),
                    };
                    run.inbox.pending.store(false, Ordering::SeqCst);
                    ipc::write_json(&mut stream, &value).map_err(|_| "local response failed")?;
                } else {
                    run.pump(None)?;
                }
            }
        })();
        let _ = result;
        run.session.stop();
        drop(run);
        let _ = conn.flush();
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }
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
        return Err("native handshake refused".into());
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
}
