//! The slice's window: GLES draws into dmabufs on the device the compositor's feedback names, attached through
//! `zwp_linux_dmabuf_v1` version 4 and synchronised explicitly through `linux-drm-syncobj-v1` when it is offered (or by
//! waiting for its own fence before each commit when it is not). Presentation feedback counts missed refreshes.

use std::fs::File;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{delegate_noop, Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1, zwp_linux_dmabuf_feedback_v1, zwp_linux_dmabuf_v1,
};
use wayland_protocols::wp::linux_drm_syncobj::v1::client::{
    wp_linux_drm_syncobj_manager_v1, wp_linux_drm_syncobj_surface_v1,
    wp_linux_drm_syncobj_timeline_v1,
};
use wayland_protocols::wp::presentation_time::client::{wp_presentation, wp_presentation_feedback};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

use crate::gles::{Gpu, Image, FORMAT_ARGB8888, MODIFIER_INVALID};
use crate::model::{self, Gaps, Reuse};
use crate::syncobj::{self, Node, Timeline};

/// What a run does.
#[derive(Clone, Debug)]
pub struct Options {
    pub size: (i32, i32),
    /// Stop after this many frames.
    pub frames: Option<u64>,
    /// Stop after this long.
    pub duration: Option<Duration>,
    /// Draw every this-many-th frame from shared memory, switching away from dmabuf and back.
    pub shm_every: Option<u64>,
    /// Allocate by texture export even where there is a GBM device.
    pub prefer_export: bool,
    /// Draw only shared-memory frames: the control window of a comparison.
    pub shm_only: bool,
    pub title: String,
}

/// What a run saw.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub frames: u64,
    pub dmabuf_frames: u64,
    pub shm_frames: u64,
    pub gaps: Gaps,
    pub explicit_sync: bool,
    pub syncobj_surfaces_created: u64,
    pub syncobj_surfaces_destroyed: u64,
    pub release_waits: u64,
    pub fence_waits: u64,
    pub buffers: usize,
    pub modifier: Option<u64>,
    pub offered_modifiers: Vec<u64>,
    pub allocator: String,
    pub renderer: String,
    pub software: bool,
    pub device: Option<PathBuf>,
    /// Why the window drew from shared memory instead of dmabufs, if it did.
    pub fallback: Option<String>,
}

impl Report {
    /// The report as `key value` lines.
    pub fn lines(&self) -> String {
        let hex = |m: u64| format!("{m:#018x}");
        let mut out = String::new();
        let mut put = |k: &str, v: String| out.push_str(&format!("{k} {v}\n"));
        put("frames", self.frames.to_string());
        put("dmabuf_frames", self.dmabuf_frames.to_string());
        put("shm_frames", self.shm_frames.to_string());
        put("presented", self.gaps.presented.to_string());
        put("discarded", self.gaps.discarded.to_string());
        put("missed", self.gaps.missed.to_string());
        put("explicit_sync", u8::from(self.explicit_sync).to_string());
        put(
            "syncobj_surfaces_created",
            self.syncobj_surfaces_created.to_string(),
        );
        put(
            "syncobj_surfaces_destroyed",
            self.syncobj_surfaces_destroyed.to_string(),
        );
        put("release_waits", self.release_waits.to_string());
        put("fence_waits", self.fence_waits.to_string());
        put("buffers", self.buffers.to_string());
        put("modifier", self.modifier.map_or("none".into(), hex));
        put(
            "offered_modifiers",
            self.offered_modifiers
                .iter()
                .map(|&m| hex(m))
                .collect::<Vec<_>>()
                .join(","),
        );
        put("allocator", self.allocator.clone());
        put("renderer", self.renderer.replace(' ', "_"));
        put("software", u8::from(self.software).to_string());
        put(
            "device",
            self.device
                .as_ref()
                .map_or("none".into(), |d| d.display().to_string()),
        );
        put(
            "fallback",
            self.fallback
                .clone()
                .unwrap_or_else(|| "none".into())
                .replace(' ', "_"),
        );
        out
    }
}

#[derive(Default)]
struct Feedback {
    table: Vec<(u32, u64)>,
    main_device: Option<u64>,
    tranche_device: Option<u64>,
    tranche: Vec<u16>,
    /// The (device, format table indices) of each tranche.
    tranches: Vec<(Option<u64>, Vec<u16>)>,
    done: bool,
}

#[derive(Clone, Copy, Debug)]
enum BufferTag {
    Dmabuf(usize),
    Shm,
}

struct Slot {
    image: Image,
    buffer: wl_buffer::WlBuffer,
    timeline: Option<(
        Timeline,
        wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1,
    )>,
    uses: u64,
    reuse: Reuse,
    release: Option<OwnedFd>,
}

struct State {
    feedback: Feedback,
    /// Modifiers of ARGB8888 announced the version 3 way (no feedback).
    v3_modifiers: Vec<u64>,
    configured: bool,
    closed: bool,
    frame_ready: bool,
    failed_import: bool,
    slots: Vec<Slot>,
    commits: u64,
    gaps: Gaps,
    clock_monotonic: bool,
}

/// Run a window on `connection` as `options` says.
pub fn run(connection: Connection, options: &Options) -> Result<Report, String> {
    let (globals, mut queue): (_, EventQueue<State>) =
        registry_queue_init(&connection).map_err(|e| format!("registry: {e}"))?;
    let qh = queue.handle();
    let compositor: wl_compositor::WlCompositor = globals
        .bind(&qh, 4..=6, ())
        .map_err(|e| format!("wl_compositor: {e}"))?;
    let wm: xdg_wm_base::XdgWmBase = globals
        .bind(&qh, 1..=6, ())
        .map_err(|e| format!("xdg_wm_base: {e}"))?;
    let shm: wl_shm::WlShm = globals
        .bind(&qh, 1..=1, ())
        .map_err(|e| format!("wl_shm: {e}"))?;
    let presentation: Option<wp_presentation::WpPresentation> = globals.bind(&qh, 1..=1, ()).ok();
    let dmabuf: Option<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1> = if options.shm_only {
        None
    } else {
        globals.bind(&qh, 3..=4, ()).ok()
    };
    let syncobj_manager: Option<wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1> =
        if options.shm_only {
            None
        } else {
            globals.bind(&qh, 1..=1, ()).ok()
        };

    let mut state = State {
        feedback: Feedback::default(),
        v3_modifiers: Vec::new(),
        configured: false,
        closed: false,
        frame_ready: true,
        failed_import: false,
        slots: Vec::new(),
        commits: 0,
        gaps: Gaps::default(),
        clock_monotonic: false,
    };
    let surface = compositor.create_surface(&qh, ());
    let xdg = wm.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg.get_toplevel(&qh, ());
    toplevel.set_title(options.title.clone());
    toplevel.set_app_id("cupertino-gpu-slice".into());
    surface.commit();
    let mut report = Report::default();

    // the device and modifiers the compositor names
    if let Some(dmabuf) = &dmabuf {
        if dmabuf.version() >= 4 {
            dmabuf.get_default_feedback(&qh, ());
            while !state.feedback.done {
                queue
                    .blocking_dispatch(&mut state)
                    .map_err(|e| format!("feedback: {e}"))?;
            }
        } else {
            queue
                .roundtrip(&mut state)
                .map_err(|e| format!("dmabuf formats: {e}"))?;
        }
    }
    while !state.configured {
        queue
            .blocking_dispatch(&mut state)
            .map_err(|e| format!("configure: {e}"))?;
    }

    let (offered, device) = offered_modifiers(&state);
    report.offered_modifiers = offered.clone();
    report.device = device.clone();
    let node = device.as_ref().and_then(|path| {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .ok()
            .map(Node)
    });
    let mut gpu = None;
    if dmabuf.is_some() {
        match Gpu::new(
            node.as_ref().map(|n| n.0.as_raw_fd()),
            options.prefer_export,
        ) {
            Ok(g) => gpu = Some(g),
            Err(e) => report.fallback = Some(format!("no GPU: {e}")),
        }
    } else if !options.shm_only {
        report.fallback = Some("no zwp_linux_dmabuf_v1".into());
    }
    let explicit =
        matches!((&syncobj_manager, &node), (Some(_), Some(node)) if node.supports_eventfd());
    report.explicit_sync = explicit && gpu.is_some();
    report.buffers = if report.explicit_sync { 2 } else { 3 };

    if let (Some(gpu), Some(dmabuf)) = (&gpu, &dmabuf) {
        report.renderer = gpu.renderer.clone();
        report.software = gpu.software();
        report.allocator = gpu.allocator.name().into();
        let renderable = gpu.renderable_modifiers(FORMAT_ARGB8888);
        let mut usable = model::usable_modifiers(&offered, &renderable, gpu.software());
        if gpu.allocator == crate::gles::Allocator::Export {
            // the export allocator takes the driver's layout; it is usable if the compositor offers it
            usable = offered.clone();
        }
        for i in 0..report.buffers {
            match make_slot(
                gpu,
                dmabuf,
                &qh,
                &mut queue,
                &mut state,
                i,
                options.size,
                &usable,
            ) {
                Ok(slot) => {
                    report.modifier = Some(slot.image.modifier);
                    state.slots.push(slot);
                }
                Err(e) => {
                    report.fallback = Some(e);
                    break;
                }
            }
        }
        if report.fallback.is_some() {
            for slot in state.slots.drain(..) {
                slot.buffer.destroy();
                gpu.free(slot.image);
            }
        }
    }
    if report.explicit_sync && report.fallback.is_none() {
        let (manager, node) = (syncobj_manager.as_ref().unwrap(), node.as_ref().unwrap());
        for slot in &mut state.slots {
            let timeline = node.timeline().map_err(|e| format!("syncobj: {e}"))?;
            let fd = node
                .export(&timeline)
                .map_err(|e| format!("syncobj export: {e}"))?;
            let object = manager.import_timeline(fd.as_fd(), &qh, ());
            slot.timeline = Some((timeline, object));
        }
    } else {
        report.explicit_sync = false;
    }
    let use_dmabuf = report.fallback.is_none() && !options.shm_only && gpu.is_some();

    let start = Instant::now();
    let mut syncobj_surface: Option<wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1> =
        None;
    let mut n = 0u64;
    let refresh_wait = Duration::from_millis(1000);
    loop {
        let done = options.frames.is_some_and(|f| n >= f)
            || options.duration.is_some_and(|d| start.elapsed() >= d)
            || state.closed;
        if done {
            break;
        }
        pump(&connection, &mut queue, &mut state, refresh_wait)?;
        let node_ref = node.as_ref();
        for slot in &mut state.slots {
            if let (Some(fd), Some((timeline, _))) = (&slot.release, &slot.timeline) {
                if syncobj::wait_readable(fd.as_fd(), 0)
                    || node_ref.is_some_and(|node| node.signalled(timeline, 2 * slot.uses))
                {
                    slot.release = None;
                    slot.reuse = Reuse::Free;
                    report.release_waits += 1;
                }
            }
        }
        if !state.frame_ready {
            continue;
        }
        let shm_frame = !use_dmabuf
            || options
                .shm_every
                .is_some_and(|every| every > 0 && n % every == every - 1);
        if shm_frame {
            if let Some(object) = syncobj_surface.take() {
                // a commit without points on a surface with a syncobj object is a protocol error
                object.destroy();
                report.syncobj_surfaces_destroyed += 1;
            }
            let buffer = shm_buffer(&shm, &qh, options.size)?;
            surface.attach(Some(&buffer), 0, 0);
            report.shm_frames += 1;
        } else {
            let commits = state.commits;
            let Some(index) = state.slots.iter().position(|s| s.reuse.free(commits)) else {
                continue;
            };
            let gpu = gpu.as_ref().unwrap();
            if report.explicit_sync && syncobj_surface.is_none() {
                syncobj_surface = Some(syncobj_manager.as_ref().unwrap().get_surface(
                    &surface,
                    &qh,
                    (),
                ));
                report.syncobj_surfaces_created += 1;
            }
            let slot = &mut state.slots[index];
            gpu.draw(&slot.image, &model::frame_rects(n, slot.image.size));
            let fence = gpu.fence()?;
            if let (Some((timeline, object)), Some(sync_surface), Some(node)) =
                (&slot.timeline, &syncobj_surface, &node)
            {
                slot.uses += 1;
                let (acquire, release) = (2 * slot.uses - 1, 2 * slot.uses);
                node.signal_with(timeline, fence.as_fd(), acquire)
                    .map_err(|e| format!("acquire point: {e}"))?;
                sync_surface.set_acquire_point(object, (acquire >> 32) as u32, acquire as u32);
                sync_surface.set_release_point(object, (release >> 32) as u32, release as u32);
                slot.release = Some(
                    node.wait_fd(timeline, release)
                        .map_err(|e| format!("release wait: {e}"))?,
                );
                slot.reuse = Reuse::ReleasePoint;
            } else {
                // without explicit sync the compositor must not sample a buffer still being drawn
                syncobj::wait_readable(fence.as_fd(), -1);
                report.fence_waits += 1;
                slot.reuse = Reuse::Release;
            }
            surface.attach(Some(&slot.buffer), 0, 0);
            report.dmabuf_frames += 1;
        }
        surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
        surface.frame(&qh, ());
        if let Some(p) = &presentation {
            p.feedback(&surface, &qh, ());
        }
        surface.commit();
        state.commits += 1;
        state.frame_ready = false;
        n += 1;
        connection.flush().map_err(|e| format!("flush: {e}"))?;
    }
    // the last frames' presentation
    let _ = queue.roundtrip(&mut state);
    report.frames = n;
    report.gaps = state.gaps;
    let _ = state.clock_monotonic;
    if let Some(object) = syncobj_surface.take() {
        object.destroy();
    }
    surface.attach(None, 0, 0);
    surface.commit();
    let _ = queue.roundtrip(&mut state);
    if let Some(gpu) = &gpu {
        for slot in state.slots.drain(..) {
            slot.buffer.destroy();
            if let (Some((timeline, object)), Some(node)) = (slot.timeline, &node) {
                object.destroy();
                node.destroy(timeline);
            }
            gpu.free(slot.image);
        }
    }
    toplevel.destroy();
    xdg.destroy();
    surface.destroy();
    let _ = connection.flush();
    Ok(report)
}

/// Dispatch what has arrived, waiting for events or a release at most `timeout`.
fn pump(
    connection: &Connection,
    queue: &mut EventQueue<State>,
    state: &mut State,
    timeout: Duration,
) -> Result<(), String> {
    queue
        .dispatch_pending(state)
        .map_err(|e| format!("dispatch: {e}"))?;
    connection.flush().map_err(|e| format!("flush: {e}"))?;
    if let Some(guard) = queue.prepare_read() {
        let mut fds = vec![libc::pollfd {
            fd: guard.connection_fd().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        }];
        for slot in &state.slots {
            if let Some(fd) = &slot.release {
                fds.push(libc::pollfd {
                    fd: fd.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                });
            }
        }
        // Safety: `fds` holds valid descriptors for the call.
        let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, timeout.as_millis() as i32) };
        if n > 0 && fds[0].revents & libc::POLLIN != 0 {
            match guard.read() {
                Ok(_) => {}
                Err(wayland_client::backend::WaylandError::Io(e))
                    if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(format!("read: {e}")),
            }
        }
    }
    queue
        .dispatch_pending(state)
        .map_err(|e| format!("dispatch: {e}"))?;
    Ok(())
}

/// The modifiers of ARGB8888 the compositor offers on its main device (its first tranche for that device), and the
/// device's render node.
fn offered_modifiers(state: &State) -> (Vec<u64>, Option<PathBuf>) {
    let fb = &state.feedback;
    if !fb.done {
        return (state.v3_modifiers.clone(), None);
    }
    let main = fb.main_device;
    let tranche = fb
        .tranches
        .iter()
        .find(|(device, _)| device.is_some() && *device == main)
        .or(fb.tranches.first());
    let mut modifiers = Vec::new();
    if let Some((_, indices)) = tranche {
        for &i in indices {
            if let Some(&(format, modifier)) = fb.table.get(i as usize) {
                if format == FORMAT_ARGB8888 && !modifiers.contains(&modifier) {
                    modifiers.push(modifier);
                }
            }
        }
    }
    (modifiers, main.and_then(render_node_of))
}

/// The render node of the DRM device numbered `dev`: the node itself, or the render node beside a primary node.
pub fn render_node_of(dev: u64) -> Option<PathBuf> {
    let nodes: Vec<PathBuf> = std::fs::read_dir("/dev/dri")
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("renderD"))
        })
        .collect();
    if let Some(node) = nodes.iter().find(|p| {
        std::fs::metadata(p).is_ok_and(|m| m.file_type().is_char_device() && m.rdev() == dev)
    }) {
        return Some(node.clone());
    }
    let (major, minor) = (libc::major(dev), libc::minor(dev));
    let dir = format!("/sys/dev/char/{major}:{minor}/device/drm");
    std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let name = e.file_name().to_string_lossy().into_owned();
        name.starts_with("renderD")
            .then(|| PathBuf::from("/dev/dri").join(name))
    })
}

#[allow(clippy::too_many_arguments)]
fn make_slot(
    gpu: &Gpu,
    dmabuf: &zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
    qh: &QueueHandle<State>,
    queue: &mut EventQueue<State>,
    state: &mut State,
    index: usize,
    size: (i32, i32),
    usable: &[u64],
) -> Result<Slot, String> {
    let image = gpu.allocate(size, FORMAT_ARGB8888, usable)?;
    let params = dmabuf.create_params(qh, ());
    for (i, plane) in image.planes.iter().enumerate() {
        let modifier = if image.modifier == MODIFIER_INVALID {
            MODIFIER_INVALID
        } else {
            image.modifier
        };
        params.add(
            plane.fd.as_fd(),
            i as u32,
            plane.offset,
            plane.stride,
            (modifier >> 32) as u32,
            modifier as u32,
        );
    }
    let buffer = params.create_immed(
        size.0,
        size.1,
        FORMAT_ARGB8888,
        zwp_linux_buffer_params_v1::Flags::empty(),
        qh,
        BufferTag::Dmabuf(index),
    );
    params.destroy();
    queue
        .roundtrip(state)
        .map_err(|e| format!("the compositor refused the dmabuf: {e}"))?;
    if state.failed_import {
        gpu.free(image);
        return Err("the compositor could not import the dmabuf".into());
    }
    Ok(Slot {
        image,
        buffer,
        timeline: None,
        uses: 0,
        reuse: Reuse::Free,
        release: None,
    })
}

/// A shared-memory buffer of `size` filled with [`model::SHM_COLOUR`], destroyed when released.
fn shm_buffer(
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<State>,
    size: (i32, i32),
) -> Result<wl_buffer::WlBuffer, String> {
    let (w, h) = size;
    let stride = w * 4;
    let len = (stride * h) as usize;
    // Safety: memfd_create returns a new descriptor or -1.
    let fd = unsafe { libc::memfd_create(c"gpu-slice-shm".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err("memfd_create failed".into());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let [r, g, b, a] = model::SHM_COLOUR.map(|c| (c * 255.0).round() as u8);
    let pixels = [b, g, r, a].repeat(len / 4);
    std::io::Write::write_all(
        &mut File::from(fd.try_clone().map_err(|e| e.to_string())?),
        &pixels,
    )
    .map_err(|e| format!("shm: {e}"))?;
    let pool = shm.create_pool(fd.as_fd(), len as i32, qh, ());
    let buffer = pool.create_buffer(
        0,
        w,
        h,
        stride,
        wl_shm::Format::Argb8888,
        qh,
        BufferTag::Shm,
    );
    pool.destroy();
    Ok(buffer)
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

delegate_noop!(State: wl_compositor::WlCompositor);
delegate_noop!(State: ignore wl_surface::WlSurface);
delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: wl_shm_pool::WlShmPool);
delegate_noop!(State: wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1);
delegate_noop!(State: wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1);
delegate_noop!(State: wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1);

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for State {
    fn event(
        _: &mut Self,
        wm: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for State {
    fn event(
        state: &mut Self,
        xdg: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg.ack_configure(serial);
            state.configured = true;
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for State {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Close = event {
            state.closed = true;
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.frame_ready = true;
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, BufferTag> for State {
    fn event(
        state: &mut Self,
        buffer: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        tag: &BufferTag,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            match *tag {
                BufferTag::Shm => buffer.destroy(),
                BufferTag::Dmabuf(i) => {
                    let commits = state.commits;
                    if let Some(slot) = state.slots.get_mut(i) {
                        // with explicit sync the release point says when, not this event
                        if slot.reuse == Reuse::Release {
                            slot.reuse = Reuse::ReleasedAt(commits);
                        }
                    }
                }
            }
        }
    }
}

impl Dispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
        event: zwp_linux_dmabuf_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_linux_dmabuf_v1::Event::Modifier {
            format,
            modifier_hi,
            modifier_lo,
        } = event
        {
            let modifier = (u64::from(modifier_hi) << 32) | u64::from(modifier_lo);
            if format == FORMAT_ARGB8888 && !state.v3_modifiers.contains(&modifier) {
                state.v3_modifiers.push(modifier);
            }
        }
    }
}

impl Dispatch<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
        event: zwp_linux_buffer_params_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_linux_buffer_params_v1::Event::Failed = event {
            state.failed_import = true;
        }
    }
}

fn dev_of(bytes: &[u8]) -> Option<u64> {
    Some(u64::from_ne_bytes(bytes.get(..8)?.try_into().ok()?))
}

impl Dispatch<zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
        event: zwp_linux_dmabuf_feedback_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwp_linux_dmabuf_feedback_v1::Event;
        let fb = &mut state.feedback;
        match event {
            Event::FormatTable { fd, size } => fb.table = read_table(fd, size as usize),
            Event::MainDevice { device } => fb.main_device = dev_of(&device),
            Event::TrancheTargetDevice { device } => fb.tranche_device = dev_of(&device),
            Event::TrancheFormats { indices } => fb.tranche.extend(
                indices
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_ne_bytes([c[0], c[1]])),
            ),
            Event::TrancheDone => {
                let indices = std::mem::take(&mut fb.tranche);
                fb.tranches.push((fb.tranche_device.take(), indices));
            }
            Event::Done => fb.done = true,
            _ => {}
        }
    }
}

/// The format table: 16-byte entries of a format, padding and a modifier.
fn read_table(fd: OwnedFd, size: usize) -> Vec<(u32, u64)> {
    if size == 0 {
        return Vec::new();
    }
    // Safety: a read-only private mapping of the table the compositor sent, unmapped before returning.
    unsafe {
        let p = libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ,
            libc::MAP_PRIVATE,
            fd.as_raw_fd(),
            0,
        );
        if p == libc::MAP_FAILED {
            return Vec::new();
        }
        let bytes = std::slice::from_raw_parts(p.cast::<u8>(), size);
        let table = bytes
            .as_chunks::<16>()
            .0
            .iter()
            .map(|e| {
                (
                    u32::from_ne_bytes(e[0..4].try_into().unwrap()),
                    u64::from_ne_bytes(e[8..16].try_into().unwrap()),
                )
            })
            .collect();
        libc::munmap(p, size);
        table
    }
}

impl Dispatch<wp_presentation::WpPresentation, ()> for State {
    fn event(
        state: &mut Self,
        _: &wp_presentation::WpPresentation,
        event: wp_presentation::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_presentation::Event::ClockId { clk_id } = event {
            state.clock_monotonic = clk_id == libc::CLOCK_MONOTONIC as u32;
        }
    }
}

impl Dispatch<wp_presentation_feedback::WpPresentationFeedback, ()> for State {
    fn event(
        state: &mut Self,
        _: &wp_presentation_feedback::WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wp_presentation_feedback::Event::Presented {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
                refresh,
                seq_hi,
                seq_lo,
                ..
            } => {
                let secs = (u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo);
                let time = secs * 1_000_000_000 + u64::from(tv_nsec);
                let seq = (u64::from(seq_hi) << 32) | u64::from(seq_lo);
                state.gaps.presented(seq, time, u64::from(refresh));
            }
            wp_presentation_feedback::Event::Discarded => state.gaps.discarded(),
            _ => {}
        }
    }
}
