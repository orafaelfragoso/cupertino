//! The PipeWire thread: its main loop, the commands of the owner, and one output stream for each share. The stream
//! drives its graph and allocates its own memfd buffers, which the owner has the compositor write frames into.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::os::fd::{AsRawFd, OwnedFd};
use std::rc::Rc;
use std::sync::mpsc;

use pipewire as pw;
use pw::spa;
use pw::spa::pod::Pod;
use pw::stream::{StreamFlags, StreamListener, StreamRc, StreamState};
use rustix::fs::{memfd_create, MemfdFlags};

use crate::format::{self, Fixed, Offer, DAMAGE_REGIONS};
use crate::{BufferId, Command, Format, Frame, Notice, Region, Size, State, StreamId};

type Notices = Box<dyn Fn(Notice) + Send>;

/// A buffer the stream added, and whether the owner holds it to fill.
struct Slot {
    id: BufferId,
    raw: *mut pw::sys::pw_buffer,
    _memory: OwnedFd,
    dequeued: bool,
}

/// What the stream's callbacks and the commands share about one stream.
struct Share {
    id: StreamId,
    offer: Offer,
    format: Option<Format>,
    /// The format of the buffers last asked for, which a resize leaves until the buffers of the new size are asked for.
    allocated: Option<Format>,
    /// Whether PipeWire runs the stream: a consumer takes frames.
    running: bool,
    /// The state last told to the owner.
    told: State,
    node_known: bool,
    slots: Vec<Slot>,
    seq: u64,
}

impl Share {
    /// The state to tell the owner, when it changed since it was last told.
    fn news(&mut self) -> Option<Notice> {
        let state = match (self.format, self.running) {
            (None, _) => State::Connecting,
            (Some(format), false) => State::Negotiated(format),
            (Some(format), true) => State::Streaming(format),
        };
        (state != self.told).then(|| {
            self.told = state.clone();
            Notice::State {
                stream: self.id,
                state,
            }
        })
    }
}

struct Open {
    // dropped before the stream, so destroying the stream calls back nothing
    _listener: StreamListener<Rc<RefCell<Share>>>,
    stream: StreamRc,
    share: Rc<RefCell<Share>>,
}

struct Thread {
    main_loop: pw::main_loop::MainLoopRc,
    core: pw::core::CoreRc,
    notices: Notices,
    streams: RefCell<HashMap<StreamId, Open>>,
    next_buffer: std::cell::Cell<u64>,
}

pub(crate) fn run(
    commands: pw::channel::Receiver<Command>,
    notices: Notices,
    ready: mpsc::Sender<Result<(), String>>,
) {
    pw::init();
    let connected = (|| {
        let main_loop = pw::main_loop::MainLoopRc::new(None)?;
        let context = pw::context::ContextRc::new(&main_loop, None)?;
        let core = context.connect_rc(None)?;
        Ok::<_, pw::Error>((main_loop, context, core))
    })();
    let (main_loop, _context, core) = match connected {
        Ok(connected) => connected,
        Err(e) => {
            let _ = ready.send(Err(format!("connecting to PipeWire: {e}")));
            return;
        }
    };
    let thread = Rc::new(Thread {
        main_loop: main_loop.clone(),
        core: core.clone(),
        notices,
        streams: RefCell::default(),
        next_buffer: std::cell::Cell::new(0),
    });
    let gone = Rc::new(RefCell::new(None::<String>));
    let _core_listener = core
        .add_listener_local()
        .error({
            let main_loop = main_loop.clone();
            let gone = Rc::clone(&gone);
            move |id, _, _, message| {
                if id == pw::core::PW_ID_CORE {
                    *gone.borrow_mut() = Some(format!("PipeWire went away: {message}"));
                    main_loop.quit();
                }
            }
        })
        .register();
    let _commands = commands.attach(main_loop.loop_(), {
        let thread = Rc::clone(&thread);
        move |command| thread.command(command)
    });
    let _ = ready.send(Ok(()));
    main_loop.run();
    let reason = gone.borrow_mut().take();
    let streams: Vec<StreamId> = thread
        .streams
        .borrow_mut()
        .drain()
        .map(|(id, _)| id)
        .collect();
    if let Some(reason) = reason {
        for stream in streams {
            (thread.notices)(Notice::State {
                stream,
                state: State::Closed(reason.clone()),
            });
        }
    }
}

impl Thread {
    fn notice(&self, notice: Notice) {
        (self.notices)(notice);
    }

    fn command(self: &Rc<Self>, command: Command) {
        match command {
            Command::Open {
                stream,
                name,
                offer,
            } => {
                if let Err(e) = self.open(stream, &name, offer) {
                    self.notice(Notice::State {
                        stream,
                        state: State::Closed(format!("creating the stream: {e}")),
                    });
                }
            }
            Command::Resize { stream, size } => self.resize(stream, size),
            Command::Dequeue(stream) => {
                let buffer = self.dequeue(stream);
                self.notice(Notice::Dequeued { stream, buffer });
            }
            Command::Queue {
                stream,
                buffer,
                frame,
            } => self.queue(stream, buffer, &frame),
            Command::Close(stream) => {
                self.streams.borrow_mut().remove(&stream);
            }
            Command::Quit => self.main_loop.quit(),
        }
    }

    fn open(self: &Rc<Self>, id: StreamId, name: &str, offer: Offer) -> Result<(), pw::Error> {
        let stream = StreamRc::new(
            self.core.clone(),
            name,
            pw::properties::properties! {
                *pw::keys::MEDIA_TYPE => "Video",
                *pw::keys::MEDIA_CATEGORY => "Capture",
                *pw::keys::MEDIA_CLASS => "Video/Source",
                *pw::keys::MEDIA_ROLE => "Screen",
                *pw::keys::NODE_NAME => name,
            },
        )?;
        let enum_format = format::enum_format(&offer);
        let share = Rc::new(RefCell::new(Share {
            id,
            offer,
            format: None,
            allocated: None,
            running: false,
            told: State::Connecting,
            node_known: false,
            slots: Vec::new(),
            seq: 0,
        }));
        let listener = stream
            .add_local_listener_with_user_data(Rc::clone(&share))
            .state_changed({
                let thread = Rc::downgrade(self);
                move |stream, share, _, state| {
                    if let Some(thread) = thread.upgrade() {
                        thread.state_changed(stream, share, state);
                    }
                }
            })
            .param_changed({
                let thread = Rc::downgrade(self);
                move |stream, share, id, param| {
                    if let (Some(thread), Some(param)) = (thread.upgrade(), param) {
                        if id == spa::param::ParamType::Format.as_raw() {
                            thread.format_changed(stream, share, param);
                        }
                    }
                }
            })
            .add_buffer({
                let thread = Rc::downgrade(self);
                move |_, share, raw| {
                    if let Some(thread) = thread.upgrade() {
                        thread.add_buffer(share, raw);
                    }
                }
            })
            .remove_buffer({
                let thread = Rc::downgrade(self);
                move |_, share, raw| {
                    if let Some(thread) = thread.upgrade() {
                        let mut s = share.borrow_mut();
                        if let Some(i) = s.slots.iter().position(|slot| slot.raw == raw) {
                            let slot = s.slots.remove(i);
                            let stream = s.id;
                            drop(s);
                            thread.notice(Notice::Removed {
                                stream,
                                buffer: slot.id,
                            });
                        }
                    }
                }
            })
            .register()?;
        stream.connect(
            spa::utils::Direction::Output,
            None,
            StreamFlags::DRIVER | StreamFlags::ALLOC_BUFFERS,
            &mut [Pod::from_bytes(&enum_format).expect("a serialized pod")],
        )?;
        self.streams.borrow_mut().insert(
            id,
            Open {
                _listener: listener,
                stream,
                share,
            },
        );
        Ok(())
    }

    fn state_changed(
        &self,
        stream: &pw::stream::Stream,
        share: &Rc<RefCell<Share>>,
        state: StreamState,
    ) {
        let (id, node, news) = {
            let mut s = share.borrow_mut();
            let node = stream.node_id();
            let report = !s.node_known && node != spa::sys::SPA_ID_INVALID;
            s.node_known |= report;
            let news = match state {
                StreamState::Error(reason) => Some(State::Closed(reason)),
                StreamState::Unconnected => {
                    Some(State::Closed("the stream was disconnected".to_owned()))
                }
                StreamState::Connecting => None,
                StreamState::Paused | StreamState::Streaming => {
                    s.running = matches!(state, StreamState::Streaming);
                    if s.running {
                        // a driver that has not triggered yet makes its first queued buffer drive a cycle itself, and the
                        // trigger after it runs a second cycle that drops the frame: triggering once here keeps every
                        // frame on `trigger_process`
                        let _ = stream.trigger_process();
                    }
                    None
                }
            }
            .map(|state| {
                s.told = state.clone();
                Notice::State {
                    stream: s.id,
                    state,
                }
            })
            .or_else(|| s.news());
            (s.id, report.then_some(node), news)
        };
        if let Some(node) = node {
            self.notice(Notice::Node { stream: id, node });
        }
        if let Some(news) = news {
            self.notice(news);
        }
    }

    fn format_changed(&self, stream: &pw::stream::Stream, share: &Rc<RefCell<Share>>, param: &Pod) {
        let (id, negotiated, news) = {
            let mut s = share.borrow_mut();
            let negotiated =
                Fixed::parse(param).and_then(|fixed| format::negotiate(&s.offer, fixed).ok());
            s.format = negotiated;
            s.allocated = negotiated.or(s.allocated);
            (s.id, negotiated, s.news())
        };
        // the owner knows the format before the buffers of it arrive, which updating the params may add at once
        if let Some(news) = news {
            self.notice(news);
        }
        // a format fixed against an older offer waits for the consumer to fix one of the current offer
        let Some(format) = negotiated else {
            return;
        };
        let buffers = format::buffers(&format);
        let metas = format::metas();
        let mut params = [
            Pod::from_bytes(&buffers).expect("a serialized pod"),
            Pod::from_bytes(&metas[0]).expect("a serialized pod"),
            Pod::from_bytes(&metas[1]).expect("a serialized pod"),
        ];
        if let Err(e) = stream.update_params(&mut params) {
            self.notice(Notice::State {
                stream: id,
                state: State::Closed(format!("setting the buffers: {e}")),
            });
        }
    }

    fn add_buffer(&self, share: &Rc<RefCell<Share>>, raw: *mut pw::sys::pw_buffer) {
        let mut s = share.borrow_mut();
        let Some(format) = s.allocated else {
            return;
        };
        let memory = match frame_memory(&format) {
            Ok(memory) => memory,
            Err(e) => {
                let stream = s.id;
                drop(s);
                self.notice(Notice::State {
                    stream,
                    state: State::Closed(format!("allocating a buffer: {e}")),
                });
                return;
            }
        };
        // SAFETY: PipeWire hands the buffer it allocated for this stream; with ALLOC_BUFFERS its one data block is
        // the stream's to fill in, and stays valid until `remove_buffer`.
        let filled = unsafe {
            let buffer = (*raw).buffer;
            if buffer.is_null() || (*buffer).n_datas == 0 {
                false
            } else {
                let data = &mut *(*buffer).datas;
                data.type_ = spa::sys::SPA_DATA_MemFd;
                data.flags = spa::sys::SPA_DATA_FLAG_READWRITE | spa::sys::SPA_DATA_FLAG_MAPPABLE;
                data.fd = i64::from(memory.as_raw_fd());
                data.mapoffset = 0;
                data.maxsize = format.frame_len();
                data.data = std::ptr::null_mut();
                true
            }
        };
        if !filled {
            return;
        }
        let Ok(fd) = memory.try_clone() else {
            return;
        };
        let id = BufferId(self.next_buffer.get());
        self.next_buffer.set(id.0 + 1);
        s.slots.push(Slot {
            id,
            raw,
            _memory: memory,
            dequeued: false,
        });
        let stream = s.id;
        drop(s);
        self.notice(Notice::Added {
            stream,
            buffer: id,
            fd,
        });
    }

    fn resize(&self, id: StreamId, size: Size) {
        let streams = self.streams.borrow();
        let Some(open) = streams.get(&id) else {
            return;
        };
        let (enum_format, news) = {
            let mut s = open.share.borrow_mut();
            s.offer.size = size;
            s.format = None;
            (format::enum_format(&s.offer), s.news())
        };
        if let Some(news) = news {
            self.notice(news);
        }
        let result = open
            .stream
            .update_params(&mut [Pod::from_bytes(&enum_format).expect("a serialized pod")]);
        drop(streams);
        if let Err(e) = result {
            self.notice(Notice::State {
                stream: id,
                state: State::Closed(format!("offering the new size: {e}")),
            });
        }
    }

    fn dequeue(&self, id: StreamId) -> Option<BufferId> {
        let streams = self.streams.borrow();
        let open = streams.get(&id)?;
        open.share.borrow().format?;
        // SAFETY: the stream is connected; a null pointer means no buffer is free.
        let raw = unsafe { open.stream.dequeue_raw_buffer() };
        if raw.is_null() {
            return None;
        }
        let mut s = open.share.borrow_mut();
        match s.slots.iter_mut().find(|slot| slot.raw == raw) {
            Some(slot) => {
                slot.dequeued = true;
                Some(slot.id)
            }
            None => {
                drop(s);
                // SAFETY: the buffer was dequeued from this stream just now.
                unsafe { open.stream.queue_raw_buffer(raw) };
                None
            }
        }
    }

    fn queue(&self, id: StreamId, buffer: BufferId, frame: &Frame) {
        let streams = self.streams.borrow();
        let Some(open) = streams.get(&id) else {
            return;
        };
        let raw = {
            let mut s = open.share.borrow_mut();
            let Some(format) = s.format else {
                return;
            };
            s.seq += 1;
            let seq = s.seq;
            let Some(slot) = s
                .slots
                .iter_mut()
                .find(|slot| slot.id == buffer && slot.dequeued)
            else {
                return;
            };
            slot.dequeued = false;
            // SAFETY: the slot is a buffer of this stream that the owner holds; nothing else writes it until it is
            // queued below.
            unsafe { describe(slot.raw, &format, frame, seq) };
            slot.raw
        };
        // SAFETY: the buffer was dequeued from this stream and is queued once.
        unsafe { open.stream.queue_raw_buffer(raw) };
        let _ = open.stream.trigger_process();
    }
}

/// A memfd the size of one frame of `format`.
fn frame_memory(format: &Format) -> std::io::Result<OwnedFd> {
    let fd = memfd_create("cupertino-screencast", MemfdFlags::CLOEXEC)?;
    let file = File::from(fd);
    file.set_len(u64::from(format.frame_len()))?;
    Ok(OwnedFd::from(file))
}

/// The regions a buffer's damage meta holds for `damage` of a frame of `size`, with room for `capacity`: the damage
/// itself, cut to the frame, or the whole frame when it does not fit.
pub(crate) fn damage_regions(damage: &[Region], size: Size, capacity: usize) -> Vec<Region> {
    let whole = Region {
        x: 0,
        y: 0,
        width: size.width,
        height: size.height,
    };
    let cut: Vec<Region> = damage
        .iter()
        .filter_map(|r| {
            let x0 = r.x.clamp(0, size.width as i32);
            let y0 = r.y.clamp(0, size.height as i32);
            let x1 = (i64::from(r.x) + i64::from(r.width)).clamp(0, i64::from(size.width)) as i32;
            let y1 = (i64::from(r.y) + i64::from(r.height)).clamp(0, i64::from(size.height)) as i32;
            (x1 > x0 && y1 > y0).then_some(Region {
                x: x0,
                y: y0,
                width: (x1 - x0) as u32,
                height: (y1 - y0) as u32,
            })
        })
        .collect();
    if cut.len() > capacity {
        vec![whole]
    } else {
        cut
    }
}

/// Fill in the chunk, the header and the damage of a buffer that holds a frame of `format`.
///
/// # Safety
///
/// `raw` is a buffer of a stream with `format`, with one data block, that nothing else writes meanwhile.
unsafe fn describe(raw: *mut pw::sys::pw_buffer, format: &Format, frame: &Frame, seq: u64) {
    let buffer = (*raw).buffer;
    let data = &mut *(*buffer).datas;
    let chunk = &mut *data.chunk;
    chunk.offset = 0;
    chunk.size = format.frame_len();
    chunk.stride = format.stride() as i32;
    chunk.flags = 0;
    let header = spa::sys::spa_buffer_find_meta_data(
        buffer,
        spa::sys::SPA_META_Header,
        std::mem::size_of::<spa::sys::spa_meta_header>(),
    ) as *mut spa::sys::spa_meta_header;
    if !header.is_null() {
        (*header).flags = 0;
        (*header).offset = 0;
        (*header).pts = frame.pts.as_nanos() as i64;
        (*header).dts_offset = 0;
        (*header).seq = seq;
    }
    let meta = spa::sys::spa_buffer_find_meta(buffer, spa::sys::SPA_META_VideoDamage);
    if meta.is_null() {
        return;
    }
    let capacity = (*meta).size as usize / std::mem::size_of::<spa::sys::spa_meta_region>();
    let regions = std::slice::from_raw_parts_mut(
        (*meta).data as *mut spa::sys::spa_meta_region,
        capacity.min(DAMAGE_REGIONS),
    );
    let size = Size {
        width: format.width,
        height: format.height,
    };
    let damage = damage_regions(&frame.damage, size, regions.len());
    for (i, slot) in regions.iter_mut().enumerate() {
        let r = damage.get(i).copied().unwrap_or(Region {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        });
        slot.region.position.x = r.x;
        slot.region.position.y = r.y;
        slot.region.size.width = r.width;
        slot.region.size.height = r.height;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: Size = Size {
        width: 800,
        height: 600,
    };

    fn region(x: i32, y: i32, width: u32, height: u32) -> Region {
        Region {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn damage_is_cut_to_the_frame_and_empty_rectangles_go() {
        assert_eq!(
            damage_regions(
                &[
                    region(-10, -10, 20, 20),
                    region(790, 590, 50, 50),
                    region(900, 0, 10, 10),
                    region(5, 5, 0, 3),
                ],
                SIZE,
                16
            ),
            [region(0, 0, 10, 10), region(790, 590, 10, 10)]
        );
    }

    #[test]
    fn more_damage_than_fits_is_the_whole_frame() {
        let many: Vec<Region> = (0..17).map(|i| region(i * 10, 0, 5, 5)).collect();
        assert_eq!(damage_regions(&many, SIZE, 16), [region(0, 0, 800, 600)]);
        assert_eq!(damage_regions(&many[..16], SIZE, 16).len(), 16);
    }
}
