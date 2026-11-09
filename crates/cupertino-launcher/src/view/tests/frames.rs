//! What each frame of the panel's interactions costs at whole and fractional backing scales, and how often the glass
//! under it has to be reshaped.
//!
//! The panel runs on a clock that moves one 60 Hz frame at a time ([`FrameMeter`]), with its animation frames
//! answered at each step, so the opening, unfolding and folding transitions run frame by frame as they do in the
//! session. Each interaction keeps its frames ([`FrameSample`]: wall time, repainted pixels, whole repaints) and the
//! glass region changes it sent the compositor ([`WindowRequest::Materials`]): each change makes the compositor
//! sample and blur what is under the panel again, so a change on a frame that only moves a selection or a hover is a
//! cost the panel should never pay.
//!
//! `glass_follows_only_the_panel_s_shape_at_fractional_scales` checks that at every scale of [`SCALES`]: the arrows
//! and hover inside a settled panel reshape no glass and repaint no whole window, and a transition reshapes it at
//! most once a frame. `frame_cost_report` is `#[ignore]`d: it measures wall time, which depends on the machine, and
//! is run by hand in the release profile (see the crate's README); the tables go to standard output and, when
//! `CUPERTINO_REPAINT_DIR` names a folder, to `launcher-<scale>x.md` and `.csv` there.

use super::*;
use cupertino_kit::platform::WindowRequest;
use cupertino_kit::render::{
    CpuFrame, DamageRegion, PixelFormat, PixelRect, RenderError, RenderFrameId, RenderRequest,
    UiRenderer,
};
use cupertino_ui_test::repaint::{csv, FrameMeter, FrameSample, Summary};
use cupertino_ui_test::HeadlessWindowSystem;
use std::collections::BTreeMap;

/// The backing scales measured.
const SCALES: [f64; 5] = [1.0, 1.25, 1.5, 1.75, 2.0];

/// The fractional ones, where a region off the device pixel grid shows.
const FRACTIONAL: [f64; 3] = [1.25, 1.5, 1.75];

/// The renderer the panel's process runs.
const RENDERER: CpuConfig = CpuConfig {
    max_pixels: 1 << 24,
    glyph_cache_bytes: 4 << 20,
    image_cache_bytes: 8 << 20,
    font_smoothing: true,
};

/// A renderer that draws nothing: it hands back a blank frame of the size asked for with the damage it was asked to
/// repaint, for the checks of what a frame repaints and reshapes, which do not need its pixels and run in a fraction
/// of the time.
struct Blank {
    frame: CpuFrame,
}

impl Default for Blank {
    fn default() -> Self {
        Self {
            frame: CpuFrame {
                id: RenderFrameId(0),
                width: 0,
                height: 0,
                stride: 0,
                format: PixelFormat::Rgba8Premultiplied,
                pixels: Vec::new(),
                damage: Vec::new(),
            },
        }
    }
}

impl UiRenderer for Blank {
    type Frame = CpuFrame;

    fn render(&mut self, request: &RenderRequest<'_>) -> Result<RenderFrameId, RenderError> {
        let (w, h) = (request.width, request.height);
        let whole = PixelRect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        };
        let scale = request.scale;
        self.frame.damage = match request.damage {
            DamageRegion::Full => vec![whole],
            DamageRegion::Rects(rects) => rects
                .iter()
                .map(|r| {
                    let px = |v: f64, most: u32| (v * scale).clamp(0.0, f64::from(most));
                    let (x0, y0) = (px(r.min_x(), w).floor(), px(r.min_y(), h).floor());
                    let (x1, y1) = (px(r.max_x(), w).ceil(), px(r.max_y(), h).ceil());
                    PixelRect {
                        x: x0 as u32,
                        y: y0 as u32,
                        width: (x1 - x0) as u32,
                        height: (y1 - y0) as u32,
                    }
                })
                .filter(|r| r.width > 0 && r.height > 0)
                .collect(),
        };
        self.frame.id = RenderFrameId(self.frame.id.0 + 1);
        self.frame.width = w;
        self.frame.height = h;
        self.frame.stride = w * 4;
        self.frame.pixels.resize(w as usize * h as usize * 4, 0);
        Ok(self.frame.id)
    }

    fn frame(&self, id: RenderFrameId) -> Result<&CpuFrame, RenderError> {
        if id == self.frame.id {
            Ok(&self.frame)
        } else {
            Err(RenderError::UnknownFrame(id))
        }
    }
}

/// An icon the metered host's icon thread is asked for, and where its renditions go.
type IconLoad = (String, async_channel::Sender<Vec<ImageSource>>);

thread_local! {
    /// The animation frame the model waits for, delivered at the next step.
    static PENDING: Cell<Option<u64>> = const { Cell::new(None) };
    /// The session's monotonic clock, in seconds: the meter's.
    static CLOCK: Cell<f64> = const { Cell::new(0.0) };
    /// How many applications the next [`Metered`] launcher lists; the test entries when `None`.
    static APPS: Cell<Option<usize>> = const { Cell::new(None) };
    /// The metered host's icon thread, started at the first icon asked.
    static ICON_THREAD: RefCell<Option<std::sync::mpsc::Sender<IconLoad>>> = const { RefCell::new(None) };
    /// The icons asked of the host so far, and the most waiting at once.
    static ICONS_ASKED: Cell<usize> = const { Cell::new(0) };
    static MOST_LOADING: Cell<usize> = const { Cell::new(0) };
    /// The wall time the loop spent asking the host for icons.
    static LOOP_ICON_NANOS: Cell<u128> = const { Cell::new(0) };
}

/// What loading and decoding one application icon costs the metered host's icon thread: about what a large theme icon
/// takes to read and scale in this profile.
const ICON_DECODE: std::time::Duration = std::time::Duration::from_millis(3);

/// A launcher listing `count` applications, each with an icon of its own.
fn launcher_of(count: usize) -> crate::Launcher {
    use crate::engine::tests_support::app;
    let entries = (0..count)
        .map(|i| app(&format!("app{i}"), &format!("Application {i}"), "", "true"))
        .collect();
    crate::Launcher::new(
        crate::LauncherConfig::default(),
        entries,
        crate::Frecency::in_memory(),
    )
}

/// The test host, with a clock that moves and animation frames answered one per step.
struct MeteredHost;

impl Host for MeteredHost {
    fn run(&mut self, _: CommandSpec, _: Option<String>) {}
    fn open(&mut self, _: &str) {}
    fn index(&mut self, _: FileSearchConfig) -> Vec<Command<Message>> {
        Vec::new()
    }
    fn resize(&mut self, size: Size) {
        EXTENT.set(Some(size));
    }
    fn frame(&mut self, frame: u64) -> Vec<Command<Message>> {
        PENDING.set(Some(frame));
        Vec::new()
    }
    fn icon(&mut self, _: &ResultItem) -> Vec<ImageSource> {
        Vec::new()
    }
    fn preview(&mut self, _: &ResultItem) -> Vec<Command<Message>> {
        Vec::new()
    }
    /// As the session's host does: the icon is loaded and decoded on a thread of the host's, which spends
    /// [`ICON_DECODE`] on it, and arrives as a message.
    fn app_icon(&mut self, app: &crate::browse::App) -> Vec<Command<Message>> {
        let start = std::time::Instant::now();
        ICONS_ASKED.set(ICONS_ASKED.get() + 1);
        let loads = ICON_THREAD.with_borrow_mut(|thread| {
            thread
                .get_or_insert_with(|| {
                    let (loads, queue) = std::sync::mpsc::channel::<IconLoad>();
                    std::thread::spawn(move || {
                        for (_, reply) in queue {
                            std::thread::sleep(ICON_DECODE);
                            let _ = reply.send_blocking(Vec::new());
                        }
                    });
                    loads
                })
                .clone()
        });
        let (reply, loaded) = async_channel::bounded(1);
        loads.send((app.id.clone(), reply)).unwrap();
        let id = app.id.clone();
        let commands = vec![Command::Perform(Box::pin(async move {
            Message::AppIcon(id, loaded.recv().await.unwrap_or_default())
        }))];
        LOOP_ICON_NANOS.set(LOOP_ICON_NANOS.get() + start.elapsed().as_nanos());
        commands
    }
    fn now(&self) -> Now {
        Now {
            mono: CLOCK.get(),
            unix: 1_000_000.0,
        }
    }
}

/// The launcher on the test entries, hidden, with its transitions running.
struct MeteredApp;

impl Application for MeteredApp {
    type Model = Model;
    type Message = Message;
    const RESIDENT: bool = true;

    fn init() -> (Model, Vec<Command<Message>>) {
        EXTENT.set(None);
        INBOXED.set(false);
        PENDING.set(None);
        CLOCK.set(0.0);
        INBOX.set(Some(async_channel::unbounded()));
        let launcher = APPS.get().map_or_else(test_launcher, launcher_of);
        let model = Model::new(launcher, Box::new(MeteredHost));
        (model, vec![listen()])
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        let inbox = INBOXED.replace(false);
        let mut commands = kit::update(model, message);
        if inbox {
            commands.push(listen());
        }
        MOST_LOADING.set(MOST_LOADING.get().max(model.app_icons_loading()));
        commands
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        kit::view(model)
    }
}

fn send(message: Message) {
    INBOX.with_borrow(|inbox| inbox.as_ref().unwrap().0.try_send(message).unwrap());
}

/// What one interaction cost.
struct Measured {
    name: &'static str,
    frames: Vec<FrameSample>,
    /// The glass region changes it sent.
    reshapes: usize,
}

/// The launcher at one backing scale.
struct Metered {
    driver: TestDriver<MeteredApp>,
    scale: f64,
    /// Each window configured, with the size it was configured at.
    configured: BTreeMap<WindowId, Option<Size>>,
    /// The meter of the panel window shown, with that window.
    meter: Option<(FrameMeter, WindowId)>,
    now: Timestamp,
    /// The panel's glass region changes counted so far.
    reshapes: usize,
}

impl Metered {
    /// The launcher at `scale`, drawing its frames when `draw`, or with the [`Blank`] renderer.
    fn new(scale: f64, draw: bool) -> Metered {
        let mut fonts = FontIndex::new();
        fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
        let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
        let glyphs = Arc::clone(&text);
        let options = RuntimeOptions {
            theme: Arc::new(theme()),
            text,
            icons: Arc::new(PlaceholderIcons::default()),
            direction: LayoutDirection::Ltr,
            node_limit: 4000,
            renderers: Box::new(move || -> Box<dyn UiRenderer<Frame = CpuFrame>> {
                if draw {
                    Box::new(CpuRenderer::new(
                        RENDERER,
                        GlyphRaster::new(Arc::clone(&glyphs)),
                    ))
                } else {
                    Box::new(Blank::default())
                }
            }),
            accessibility: None,
            clock: Arc::new(ManualClock::new(Timestamp(0))),
            on_window_error: None,
        };
        let capabilities = Capabilities {
            fractional_scale: true,
            app_effects: true,
            ..Capabilities::default()
        };
        let driver = TestDriver::new(capabilities, options, ManualClock::new(Timestamp(0)));
        let mut metered = Metered {
            driver,
            scale,
            configured: BTreeMap::new(),
            meter: None,
            now: Timestamp(0),
            reshapes: 0,
        };
        metered.driver.window_system_mut().set_keep_pixels(false);
        metered.driver.run_until_idle(budget()).unwrap();
        metered.configure();
        metered.driver.run_until_idle(budget()).unwrap();
        metered
    }

    fn panel(&self) -> Option<WindowId> {
        self.driver
            .runtime()
            .windows()
            .into_iter()
            .find(|(k, _)| *k == WindowKey(Key::from("panel")))
            .map(|(_, id)| id)
    }

    /// Configures each new window as the server would, at the backing scale, and the panel again at each new size.
    fn configure(&mut self) {
        for (key, id) in self.driver.runtime().windows() {
            let size = if key == WindowKey(Key::from("panel")) {
                Some(EXTENT.with_borrow(|e| e.unwrap_or(window_extent(compact_shape()))))
            } else if key == WindowKey(Key::from("catcher")) {
                Some(OUTPUT)
            } else {
                None
            };
            let known = self.configured.get(&id);
            if known.is_some_and(|known| *known == size) {
                continue;
            }
            let system = self.driver.window_system_mut();
            if known.is_none() {
                system
                    .enqueue_window(id, WindowEvent::Scale(self.scale))
                    .unwrap();
            }
            system.configure(id, size, active()).unwrap();
            self.configured.insert(id, size);
        }
    }

    /// The panel's glass region changes so far.
    fn count_reshapes(&self, panel: WindowId) -> usize {
        self.driver
            .runtime()
            .window_system()
            .requests(panel)
            .map_or(0, |requests| {
                requests
                    .iter()
                    .filter(|r| matches!(r, WindowRequest::Materials(_)))
                    .count()
            })
    }

    /// One frame period on: `input` queues the period's input and the animation frame the model waits for is
    /// answered. A panel that just opened is metered from its first configure.
    fn step(&mut self, input: impl FnOnce(&mut HeadlessWindowSystem, WindowId)) -> bool {
        self.configure();
        let Some(panel) = self.panel() else {
            input(self.driver.window_system_mut(), WindowId(0));
            self.driver.run_until_idle(budget()).unwrap();
            return false;
        };
        if self.meter.as_ref().is_none_or(|(_, w)| *w != panel) {
            self.meter = Some((FrameMeter::new(panel, self.now), panel));
            self.reshapes = self.count_reshapes(panel);
        }
        let (meter, _) = self.meter.as_mut().unwrap();
        let presented = meter
            .step(&mut self.driver, |system, now| {
                CLOCK.set(now.0 as f64 / 1e9);
                if let Some(frame) = PENDING.take() {
                    send(Message::Frame(frame));
                }
                input(system, panel);
            })
            .is_some();
        self.now = meter.now();
        presented
    }

    /// Steps until nothing presented for `quiet` steps in a row, at most `limit` steps.
    fn until_quiet(&mut self, quiet: usize, limit: usize) {
        let mut still = 0;
        for _ in 0..limit {
            let presented = self.step(|_, _| {});
            let animating = PENDING.with(|p| p.get().is_some());
            still = if presented || animating { 0 } else { still + 1 };
            if still >= quiet {
                return;
            }
        }
        panic!("the panel keeps presenting frames");
    }

    /// The frames and glass changes since the last call.
    fn take(&mut self, name: &'static str) -> Measured {
        let frames = self
            .meter
            .as_mut()
            .map(|(m, _)| m.take())
            .unwrap_or_default();
        let panel = self.meter.as_ref().map(|(_, w)| *w);
        let now = panel.map_or(0, |panel| self.count_reshapes(panel));
        let reshapes = now.saturating_sub(self.reshapes);
        self.reshapes = now;
        Measured {
            name,
            frames,
            reshapes,
        }
    }

    /// The panel's model.
    fn model(&self) -> &Model {
        self.driver.runtime().model()
    }

    /// The panel's size in points.
    fn panel_size(&self) -> Size {
        let panel = self.panel().expect("the panel is shown");
        self.configured[&panel].expect("the panel was configured at a size")
    }
}

fn key(
    system: &mut HeadlessWindowSystem,
    panel: WindowId,
    keysym: u32,
    text: Option<char>,
    mods: Mods,
) {
    for pressed in [true, false] {
        let event = KeyEvent {
            code: 0,
            keysym: Keysym(keysym),
            text: text.filter(|_| pressed),
            pressed,
            repeat: false,
            mods,
        };
        system
            .enqueue_window(panel, WindowEvent::Key(event, Timestamp(0)))
            .unwrap();
    }
}

/// Each interaction at `scale`, in the order measured: the search field opens and takes a query, unfolds into the
/// browser, the arrows and the pointer move through the applications, the browser folds back and the field closes;
/// then the Dock opens the browser and Escape folds and closes it.
fn interactions(scale: f64, draw: bool) -> Vec<Measured> {
    let mut m = Metered::new(scale, draw);
    let mut out = Vec::new();

    send(Message::Toggle);
    m.until_quiet(10, 200);
    assert_eq!(m.model().nav.view, View::Search);
    out.push(m.take("search opens"));

    for c in "te".chars() {
        m.step(|s, w| key(s, w, c as u32, Some(c), Mods::NONE));
        m.until_quiet(3, 200);
    }
    out.push(m.take("typing"));

    m.step(|s, w| key(s, w, '1' as u32, Some('1'), Mods::PRIMARY));
    m.until_quiet(10, 200);
    assert_eq!(m.model().nav.view, View::Browse);
    out.push(m.take("unfold into browser"));

    // clear the query so every application shows
    m.step(|s, w| key(s, w, ESCAPE, None, Mods::NONE));
    m.until_quiet(10, 200);
    m.take("clear");
    for _ in 0..8 {
        m.step(|s, w| key(s, w, RIGHT, None, Mods::NONE));
    }
    m.until_quiet(10, 200);
    out.push(m.take("browser arrows"));

    let size = m.panel_size();
    for i in 0..40 {
        let t = f64::from(i) / 40.0;
        let position = Point::new(size.width * (0.1 + 0.8 * t), size.height * (0.2 + 0.6 * t));
        m.step(|s, w| {
            let event = WindowEvent::Pointer(PointerEvent::Motion { position }, Timestamp(0));
            s.enqueue_window(w, event).unwrap();
        });
    }
    m.until_quiet(10, 200);
    out.push(m.take("browser hover"));

    m.step(|s, w| key(s, w, ESCAPE, None, Mods::NONE));
    m.until_quiet(10, 200);
    assert_eq!(m.model().nav.view, View::Search);
    out.push(m.take("fold into search field"));

    m.step(|s, w| key(s, w, ESCAPE, None, Mods::NONE));
    m.until_quiet(10, 200);
    out.push(m.take("search closes"));
    while m.panel().is_some() {
        m.step(|_, _| {});
    }

    send(Message::Browse);
    m.step(|_, _| {});
    m.until_quiet(10, 200);
    assert_eq!(m.model().nav.view, View::Browse);
    out.push(m.take("dock opens browser"));

    m.step(|s, w| key(s, w, ESCAPE, None, Mods::NONE));
    m.until_quiet(10, 200);
    out.push(m.take("browser folds"));
    out
}

/// Whether `rect`'s edges fall on whole device pixels at `scale`.
fn on_device_pixels(rect: Rect, scale: f64) -> bool {
    let whole = |v: f64| ((v * scale) - (v * scale).round()).abs() < 1e-6;
    whole(rect.min_x()) && whole(rect.min_y()) && whole(rect.max_x()) && whole(rect.max_y())
}

#[test]
fn glass_follows_only_the_panel_s_shape_at_fractional_scales() {
    for scale in FRACTIONAL {
        let runs = interactions(scale, false);
        let of = |name: &str| runs.iter().find(|r| r.name == name).unwrap();
        for name in ["browser arrows", "browser hover"] {
            let run = of(name);
            assert!(!run.frames.is_empty(), "{name} at {scale}x presents");
            assert_eq!(run.reshapes, 0, "{name} at {scale}x reshapes no glass");
            assert!(
                run.frames.iter().all(|f| !f.full),
                "{name} at {scale}x repaints only what changed"
            );
        }
        // each key changes the results, and so the panel's height
        for name in [
            "search opens",
            "typing",
            "unfold into browser",
            "fold into search field",
            "dock opens browser",
        ] {
            let run = of(name);
            assert!(run.reshapes > 0, "{name} at {scale}x reshapes the glass");
            assert!(
                run.reshapes <= run.frames.len(),
                "{name} at {scale}x reshapes the glass at most once a frame: {} in {} frames",
                run.reshapes,
                run.frames.len()
            );
        }
    }
}

impl Metered {
    /// The glass regions the panel stands on now.
    fn glass(&self) -> Vec<Rect> {
        let panel = self.panel().expect("the panel is shown");
        let requests = self
            .driver
            .runtime()
            .window_system()
            .requests(panel)
            .unwrap();
        let last = requests.iter().rev().find_map(|r| match r {
            WindowRequest::Materials(regions) => Some(regions),
            _ => None,
        });
        last.expect("the panel stands on glass")
            .iter()
            .map(|region| region.rect)
            .collect()
    }

    /// Delivers `message` and lets what it starts settle.
    fn settle_on(&mut self, message: Message) {
        send(message);
        self.step(|_, _| {});
        self.until_quiet(4, 200);
    }
}

#[test]
fn the_settled_panel_s_glass_sits_on_whole_device_pixels() {
    for scale in SCALES {
        let mut m = Metered::new(scale, false);
        m.settle_on(Message::ReduceMotion(true));
        let check = |m: &Metered, what: &str| {
            for rect in m.glass() {
                assert!(
                    on_device_pixels(rect, scale),
                    "{what}: the glass at {rect:?} falls between device pixels at {scale}x"
                );
            }
        };
        m.settle_on(Message::Toggle);
        check(&m, "the search field");
        for c in "te".chars() {
            m.step(|s, w| key(s, w, c as u32, Some(c), Mods::NONE));
            m.until_quiet(4, 200);
            check(&m, "the results");
        }
        m.step(|s, w| key(s, w, 'z' as u32, Some('z'), Mods::NONE));
        m.until_quiet(4, 200);
        check(&m, "the row saying nothing was found");
        m.step(|s, w| key(s, w, ESCAPE, None, Mods::NONE));
        m.until_quiet(4, 200);
        m.step(|s, w| key(s, w, '1' as u32, Some('1'), Mods::PRIMARY));
        m.until_quiet(4, 200);
        assert_eq!(m.model().nav.view, View::Browse);
        check(&m, "the browser");
        // on an output whose height leaves the browser an uneven room
        m.settle_on(Message::Output(Size::new(1000.0, 703.0)));
        check(&m, "the browser on a short output");
    }
}

/// [`SCALES`], or the comma-separated scales of `CUPERTINO_REPAINT_SCALES`.
fn scales() -> Vec<f64> {
    match std::env::var("CUPERTINO_REPAINT_SCALES") {
        Ok(list) => list.split(',').map(|s| s.trim().parse().unwrap()).collect(),
        Err(_) => SCALES.to_vec(),
    }
}

#[test]
#[ignore = "wall time: run by hand in the release profile"]
fn frame_cost_report() {
    let dir = std::env::var_os("CUPERTINO_REPAINT_DIR").map(std::path::PathBuf::from);
    for scale in scales() {
        let mut runs = interactions(scale, true);
        runs.push(browser_over_many(200, scale, true).0);
        let (head, rule) = Summary::markdown_header().split_once('\n').unwrap();
        let mut table = format!(
            "Launcher at scale {scale}\n\n{head} glass changes |\n{}---|\n",
            rule.trim()
        );
        for run in &runs {
            let row = Summary::of(&run.frames).markdown_row(run.name);
            let row = row.trim_end_matches('|').trim_end();
            let _ = writeln!(table, "{row} | {} |", run.reshapes);
        }
        println!("{table}");
        if let Some(dir) = &dir {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join(format!("launcher-{scale}x.md")), &table).unwrap();
            let rows = runs.iter().map(|r| (r.name, r.frames.as_slice()));
            std::fs::write(dir.join(format!("launcher-{scale}x.csv")), csv(rows)).unwrap();
        }
    }
}

/// The Dock opens the browser over `apps` applications at `scale` and every icon near the view arrives: the frames,
/// and how many icons were near the view.
fn browser_over_many(apps: usize, scale: f64, draw: bool) -> (Measured, usize) {
    APPS.set(Some(apps));
    ICON_THREAD.set(None);
    ICONS_ASKED.set(0);
    MOST_LOADING.set(0);
    LOOP_ICON_NANOS.set(0);
    let mut m = Metered::new(scale, draw);
    APPS.set(None);
    send(Message::Browse);
    m.step(|_, _| {});
    m.until_quiet(10, 200);
    assert_eq!(m.model().nav.view, View::Browse);
    let nearby = m.model().browser.as_ref().unwrap().nearby().len();
    // the icons arrive as the thread decodes them, between frames
    let waited = std::time::Instant::now();
    while m.model().app_icons.len() < nearby || m.model().app_icons_loading() > 0 {
        assert!(
            waited.elapsed() < std::time::Duration::from_secs(30),
            "{} of {nearby} icons arrived",
            m.model().app_icons.len()
        );
        m.step(|_, _| {});
        std::thread::sleep(ICON_DECODE);
    }
    m.until_quiet(10, 200);
    ICON_THREAD.set(None);
    (m.take("dock opens browser, 200 apps"), nearby)
}

/// The Dock opens the browser over 200 applications: every icon near the view is loaded and decoded off the loop, at
/// most [`kit::MAX_APP_ICON_LOADS`] waiting at once, though each costs [`ICON_DECODE`] to decode (a pass decoded on the
/// loop would overrun several frames). What the loop spends asking for them is a small part of one frame budget, and
/// the frames that show them arriving stay within it. The window's first frame, which lays out the browser whole, is held
/// to the budget in an optimised build only (about 4 ms there at 1x); unoptimised it takes several times that.
#[test]
fn opening_the_browser_over_200_applications_keeps_icon_decoding_off_the_frames() {
    use cupertino_ui_test::repaint::FRAME_BUDGET;
    let (run, nearby) = browser_over_many(200, 1.0, false);
    assert!(
        nearby > 2 * kit::MAX_APP_ICON_LOADS,
        "{nearby} icons near the view"
    );
    assert_eq!(
        ICONS_ASKED.get(),
        nearby,
        "each icon near the view asked once"
    );
    assert_eq!(
        MOST_LOADING.get(),
        kit::MAX_APP_ICON_LOADS,
        "the loads waiting are capped"
    );
    let summary = Summary::of(&run.frames);
    eprintln!(
        "{}\n{}\nicon requests on the loop: {} us in all",
        Summary::markdown_header(),
        summary.markdown_row(run.name),
        LOOP_ICON_NANOS.get() / 1000
    );
    assert!(
        LOOP_ICON_NANOS.get() < FRAME_BUDGET.as_nanos() / 8,
        "asking for {nearby} icons took the loop {} us",
        LOOP_ICON_NANOS.get() / 1000
    );
    let first_ms = run.frames[0].nanos as f64 / 1e6;
    eprintln!("first frame: {first_ms:.2} ms");
    if !cfg!(debug_assertions) {
        assert!(
            first_ms < FRAME_BUDGET.as_secs_f64() * 1000.0,
            "the first frame lays out 200 applications in {first_ms:.2} ms"
        );
    }
    let arriving = Summary::of(&run.frames[1..]);
    assert!(
        arriving.frames >= nearby / kit::MAX_APP_ICON_LOADS,
        "the icons show as they arrive: {arriving:?}"
    );
    assert!(
        arriving.p50_ms < FRAME_BUDGET.as_secs_f64() * 1000.0,
        "frames as the icons arrive: {arriving:?}"
    );
}
