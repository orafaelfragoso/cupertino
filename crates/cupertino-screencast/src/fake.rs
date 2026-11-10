//! Stand-ins for the tests of screen sharing: a private PipeWire daemon and a video consumer, as a browser would
//! connect to a shared stream.

use std::cell::RefCell;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use pipewire as pw;
use pw::spa;
use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use spa::param::video::VideoFormat;
use spa::pod::{object, property, Pod, Value};
use spa::utils::{Fraction, Rectangle, SpaTypes};

use crate::format::DAMAGE_REGIONS;
use crate::Region;

const NAME: &str = "fake-consumer";

/// `pipewire` in a temporary runtime directory of its own, never the user's; killed on drop, or when the thread that
/// started it ends.
pub struct Daemon {
    child: Child,
    runtime: tempfile::TempDir,
}

impl Daemon {
    /// `None` when `pipewire` is not installed or makes no socket within `wait`.
    pub fn start(wait: Duration) -> Option<Daemon> {
        let runtime = tempfile::tempdir().ok()?;
        let mut command = Command::new("pipewire");
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", runtime.path())
            .env("XDG_RUNTIME_DIR", runtime.path())
            .env("PIPEWIRE_RUNTIME_DIR", runtime.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: only an async-signal-safe call between fork and exec
        unsafe {
            command.pre_exec(|| {
                rustix::process::set_parent_process_death_signal(Some(
                    rustix::process::Signal::KILL,
                ))
                .map_err(Into::into)
            });
        }
        let daemon = Daemon {
            child: command.spawn().ok()?,
            runtime,
        };
        let until = Instant::now() + wait;
        while !daemon.remote().exists() {
            if Instant::now() > until {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Some(daemon)
    }

    /// The daemon's socket, for `PIPEWIRE_REMOTE`.
    pub fn remote(&self) -> PathBuf {
        self.runtime.path().join("pipewire-0")
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// What the consumer saw.
#[derive(Debug, PartialEq, Eq)]
pub enum Seen {
    /// The format it fixed.
    Format { width: u32, height: u32 },
    Frame {
        size: u32,
        stride: i32,
        pixels: Vec<u8>,
        /// The header's timestamp in nanoseconds, or -1 without a header.
        pts: i64,
        damage: Vec<Region>,
    },
}

/// A video input stream on a connection of its own, linked to a stream's node. Without a session manager nothing
/// links streams, so it links its own port once the registry lists it. It asks for the header and damage metas, as
/// screen sharing consumers do.
pub struct Consumer {
    seen: mpsc::Receiver<Seen>,
    quit: pw::channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl Consumer {
    /// Connect to the daemon at `remote` and link to `node`.
    pub fn link(remote: &Path, node: u32) -> Consumer {
        let (sender, seen) = mpsc::channel();
        let (quit, quitting) = pw::channel::channel();
        let remote = remote.to_owned();
        let thread = std::thread::spawn(move || consume(&remote, node, sender, quitting));
        Consumer {
            seen,
            quit,
            thread: Some(thread),
        }
    }

    /// The next thing seen that `pick` keeps, within `wait`.
    pub fn next(&self, wait: Duration, mut pick: impl FnMut(&Seen) -> bool) -> Option<Seen> {
        let until = Instant::now() + wait;
        loop {
            let seen = self
                .seen
                .recv_timeout(until.saturating_duration_since(Instant::now()))
                .ok()?;
            if pick(&seen) {
                return Some(seen);
            }
        }
    }
}

impl Drop for Consumer {
    fn drop(&mut self) {
        let _ = self.quit.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn consume(
    remote: &Path,
    producer: u32,
    seen: mpsc::Sender<Seen>,
    quitting: pw::channel::Receiver<()>,
) {
    let main_loop = pw::main_loop::MainLoopRc::new(None).expect("a main loop");
    let context = pw::context::ContextRc::new(&main_loop, None).expect("a context");
    let core = context
        .connect_rc(Some(pw::properties::properties! {
            *pw::keys::REMOTE_NAME => remote.to_string_lossy().into_owned(),
        }))
        .expect("the daemon answers");
    let _quit = quitting.attach(main_loop.loop_(), {
        let main_loop = main_loop.clone();
        move |()| main_loop.quit()
    });
    let stream = pw::stream::StreamRc::new(
        core.clone(),
        NAME,
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::NODE_NAME => NAME,
        },
    )
    .expect("a stream");
    let registry = core.get_registry_rc().expect("the registry");
    let link = Rc::new(RefCell::new(None::<pw::link::Link>));
    let _ports = registry
        .add_listener_local()
        .global({
            let (core, link) = (core.clone(), Rc::clone(&link));
            move |global| {
                let Some(props) = global.props else { return };
                if global.type_ != pw::types::ObjectType::Port
                    || props.get("port.direction") != Some("in")
                    || !props
                        .get("object.path")
                        .is_some_and(|p| p.starts_with(&format!("{NAME}:")))
                    || link.borrow().is_some()
                {
                    return;
                }
                let Some(node) = props.get("node.id") else {
                    return;
                };
                *link.borrow_mut() = core
                    .create_object(
                        "link-factory",
                        &pw::properties::properties! {
                            *pw::keys::LINK_OUTPUT_NODE => producer.to_string(),
                            *pw::keys::LINK_INPUT_NODE => node.to_owned(),
                        },
                    )
                    .ok();
            }
        })
        .register();
    let _listener = stream
        .add_local_listener_with_user_data(seen)
        .param_changed(|stream, seen, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let mut info = spa::param::video::VideoInfoRaw::new();
            if info.parse(param).is_err() {
                return;
            }
            let _ = seen.send(Seen::Format {
                width: info.size().width,
                height: info.size().height,
            });
            let metas = [
                // OBS accepts any of these memory types in its shared-memory fallback.
                // A producer must intersect the mask rather than compare it as a fixed integer.
                serialize(spa::pod::Object {
                    type_: SpaTypes::ObjectParamBuffers.as_raw(),
                    id: spa::param::ParamType::Buffers.as_raw(),
                    properties: vec![spa::pod::Property::new(
                        spa::sys::SPA_PARAM_BUFFERS_dataType,
                        Value::Int(
                            (1 << spa::sys::SPA_DATA_MemPtr)
                                | (1 << spa::sys::SPA_DATA_MemFd)
                                | (1 << spa::sys::SPA_DATA_DmaBuf),
                        ),
                    )],
                }),
                meta(
                    spa::sys::SPA_META_Header,
                    size_of::<spa::sys::spa_meta_header>(),
                ),
                meta(
                    spa::sys::SPA_META_VideoDamage,
                    size_of::<spa::sys::spa_meta_region>() * DAMAGE_REGIONS,
                ),
            ];
            let _ = stream.update_params(
                &mut metas
                    .each_ref()
                    .map(|m| Pod::from_bytes(m).expect("a serialized pod")),
            );
        })
        .process(|stream, seen| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let pts = buffer
                .find_meta::<spa::buffer::meta::MetaHeader>()
                .map_or(-1, |h| h.pts());
            let damage = buffer
                .find_meta::<spa::buffer::meta::MetaVideoDamage>()
                .map(|d| {
                    d.iter()
                        .filter(|r| r.is_valid())
                        .map(|r| Region {
                            x: r.position().x,
                            y: r.position().y,
                            width: r.size().width,
                            height: r.size().height,
                        })
                        .collect()
                })
                .unwrap_or_default();
            let data = &mut buffer.datas_mut()[0];
            let (size, stride) = (data.chunk().size(), data.chunk().stride());
            let pixels = data
                .data()
                .map(|d| d[..size as usize].to_vec())
                .unwrap_or_default();
            let _ = seen.send(Seen::Frame {
                size,
                stride,
                pixels,
                pts,
                damage,
            });
        })
        .register()
        .expect("the stream's listener");
    let format = serialize(object!(
        SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        property!(FormatProperties::MediaType, Id, MediaType::Video),
        property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA
        ),
        property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            Rectangle {
                width: 64,
                height: 48
            },
            Rectangle {
                width: 1,
                height: 1
            },
            Rectangle {
                width: 8192,
                height: 8192
            }
        ),
        property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            Fraction { num: 30, denom: 1 },
            Fraction { num: 0, denom: 1 },
            Fraction { num: 120, denom: 1 }
        ),
    ));
    stream
        .connect(
            spa::utils::Direction::Input,
            None,
            pw::stream::StreamFlags::MAP_BUFFERS,
            &mut [Pod::from_bytes(&format).expect("a serialized pod")],
        )
        .expect("the stream connects");
    main_loop.run();
}

fn serialize(object: spa::pod::Object) -> Vec<u8> {
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &Value::Object(object),
    )
    .expect("a pod serializes")
    .0
    .into_inner()
}

fn meta(kind: u32, size: usize) -> Vec<u8> {
    serialize(spa::pod::Object {
        type_: SpaTypes::ObjectParamMeta.as_raw(),
        id: spa::param::ParamType::Meta.as_raw(),
        properties: vec![
            spa::pod::Property::new(
                spa::sys::SPA_PARAM_META_type,
                Value::Id(spa::utils::Id(kind)),
            ),
            spa::pod::Property::new(spa::sys::SPA_PARAM_META_size, Value::Int(size as i32)),
        ],
    })
}
