//! Glue between the notification model, the D-Bus server and the toolkit surface: draws the
//! banners and the list, requests one backdrop per card and routes pointer and key input.
//! The layout and routing decisions live in `cupertino_notifications::surface`.

use cupertino_assets::{AssetError, Assets};
use cupertino_client::{
    key, Anchor, Backdrop, Button, Client, DisplayOptions, Event, KeyboardInteractivity, Layer,
    LayerSurfaceConfig, Margin, PointerEvent, SurfaceId, Waker,
};
use cupertino_geometry::{Point, Rect, Size};
use cupertino_notifications::dbus::{Server, ServerOptions};
use cupertino_notifications::render::{
    BannerHit, BannerImage, BannerRenderer, BannerStyle, Layers, RenderOptions,
};
use cupertino_notifications::surface::{
    banner_press, card_at, card_press, centre_cards, centre_header, clear_menu, header_press,
    input_region, local_to, surface_width, BannerPress, CardPress, CentreHeader, ClearMenu,
    HeaderPress, MENU_RADIUS,
};
use cupertino_notifications::{
    CenterConfig, ClearScope, GroupStack, Notification, Placement, SwipeOutcome, Wakeup,
};
use cupertino_theme::{Appearance, Theme};
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tiny_skia::{PixmapPaint, Transform};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Role of the surface for the server (`wlr-layer-shell` namespace).
const NAMESPACE: &str = "cupertino-notifications";

/// Banners below this opacity are neither drawn nor given a backdrop.
const MIN_OPACITY: f64 = 0.002;

/// Shadow and content; the material is the server's backdrop.
const LAYERS: Layers = Layers {
    shadow: true,
    background: false,
    content: true,
};

/// Command line of the process.
pub struct Options {
    /// Take `org.freedesktop.Notifications` from a running daemon that allows replacement.
    pub replace_existing: bool,
}

impl Options {
    /// Parse the arguments; the unknown one is returned as the error.
    pub fn parse(args: impl Iterator<Item = String>) -> std::result::Result<Self, String> {
        let mut options = Options {
            replace_existing: false,
        };
        for arg in args {
            match arg.as_str() {
                "--replace" => options.replace_existing = true,
                _ => return Err(arg),
            }
        }
        Ok(options)
    }
}

/// What a card of the surface shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Slot {
    /// A banner of the stack.
    Banner(u32),
    /// A card of the notification centre list.
    Card(u32),
}

/// A card to draw this frame.
struct Item {
    slot: Slot,
    rect: Rect,
    opacity: f64,
    hovered: bool,
    note: Notification,
}

/// A press on a banner body that may turn into a swipe; the model decides click versus swipe.
struct Drag {
    id: u32,
    start_x: f64,
}

/// A model action that needs the activation token still being fetched.
enum Pending {
    Activate(u32),
    Invoke(u32, String),
}

/// What decides whether a rendered card can be reused: id, revision, hover, size and scale.
type Signature = (u32, u32, bool, u64, u64, u32);

struct App {
    client: Client,
    theme: Theme,
    assets: Assets,
    surface: SurfaceId,
    server: Server,
    renderer: BannerRenderer,
    /// Set by the `ToggleCentre` D-Bus call.
    toggle: Arc<AtomicBool>,
    centre_open: bool,
    /// Cards of the open list in surface coordinates.
    cards: Vec<(u32, Rect)>,
    centre_hover: Option<u32>,
    /// Header row of the open list (title and Clear button), set by the last draw.
    header: Option<CentreHeader>,
    /// Whether the Clear menu is open, and its geometry and scopes from the last draw.
    menu_open: bool,
    menu: Option<(ClearMenu, Vec<ClearScope>)>,
    menu_hover: Option<usize>,
    clear_hover: bool,
    menu_backdrop: Option<Backdrop>,
    hovered: Option<u32>,
    drag: Option<Drag>,
    /// Banners of the last drawn frame, for hit testing.
    placements: Vec<Placement>,
    backdrops: HashMap<Slot, Backdrop>,
    images: HashMap<Slot, (Signature, BannerImage)>,
    pending: HashMap<cupertino_client::ActivationRequest, Pending>,
    seen_revision: u64,
}

/// The renderer for the session's appearance and display options.
fn banner_renderer(
    theme: &Theme,
    assets: &Assets,
    dark: bool,
    options: DisplayOptions,
) -> std::result::Result<BannerRenderer, AssetError> {
    let appearance = Appearance::of_session(dark, options.increase_contrast);
    BannerRenderer::new(
        BannerStyle::from_theme(theme, appearance, options.reduce_transparency),
        assets,
    )
}

/// Connect, claim the bus names and serve until the connection or the bus fails.
pub fn run(options: &Options) -> Result<std::convert::Infallible> {
    let assets = Assets::discover();
    let theme = Theme::load()?;
    let mut client = Client::connect()?;
    let renderer = banner_renderer(&theme, &assets, client.is_dark(), client.display_options())?;
    let mut center_config = CenterConfig::from_theme(&theme);
    // banners fade instead of sliding while the session reduces motion
    center_config.motion.reduce_motion = client.display_options().reduce_motion;
    // the shadow margins do not depend on the appearance, so the surface keeps its width when it changes
    let width = surface_width(
        &center_config.layout,
        &center_config.swipe,
        f64::from(renderer.style.shadow.margins_px(1.0).left),
    );

    let waker = client.waker();
    let server = start_server(options, center_config, &waker)?;
    let toggle = Arc::new(AtomicBool::new(false));
    let (flag, wake) = (toggle.clone(), waker);
    server.serve_centre_control(move || {
        flag.store(true, Ordering::SeqCst);
        wake.wake();
    })?;
    // full height at the right edge, over the menu bar's zone (the model insets the stack itself)
    let surface = client.layer_surface(&LayerSurfaceConfig {
        namespace: NAMESPACE.to_string(),
        output: None,
        layer: Layer::Overlay,
        anchor: Anchor::Top | Anchor::Right | Anchor::Bottom,
        size: (width as u32, 0),
        margin: Margin::default(),
        exclusive_zone: -1,
        keyboard: KeyboardInteractivity::None,
    })?;
    let mut app = App {
        client,
        theme,
        assets,
        surface,
        server,
        renderer,
        toggle,
        centre_open: false,
        cards: Vec::new(),
        centre_hover: None,
        header: None,
        menu_open: false,
        menu: None,
        menu_hover: None,
        clear_hover: false,
        menu_backdrop: None,
        hovered: None,
        drag: None,
        placements: Vec::new(),
        backdrops: HashMap::new(),
        images: HashMap::new(),
        pending: HashMap::new(),
        seen_revision: 0,
    };
    let mut timeout = None;
    loop {
        for event in app.client.wait(timeout)? {
            app.handle(event)?;
        }
        timeout = app.housekeeping()?;
    }
}

/// Start the `org.freedesktop.Notifications` server; model changes from the bus wake the loop.
fn start_server(options: &Options, config: CenterConfig, waker: &Waker) -> Result<Server> {
    let wake = waker.clone();
    let opts = ServerOptions {
        config,
        wake: Some(Box::new(move || wake.wake())),
        replace_existing: options.replace_existing,
        ..ServerOptions::default()
    };
    Server::start(opts).map_err(|e| match e {
        zbus::Error::NameTaken => "another notification daemon owns org.freedesktop.Notifications (start with --replace to take it over)".into(),
        e => Box::new(e) as Box<dyn Error>,
    })
}

/// Open a link of a notification body with the default handler.
fn open_link(href: &str) {
    match Command::new("xdg-open").arg(href).spawn() {
        Ok(mut child) => {
            // reap the child without blocking the loop
            std::thread::spawn(move || {
                if let Err(e) = child.wait() {
                    eprintln!("cupertino-notifications: xdg-open: {e}");
                }
            });
        }
        Err(e) => eprintln!("cupertino-notifications: cannot run xdg-open: {e}"),
    }
}

impl App {
    fn handle(&mut self, event: Event) -> Result<()> {
        match event {
            // a draw follows every configure and scale change
            Event::Configured { .. } | Event::ScaleChanged { .. } => {}
            Event::Draw(_) => self.draw()?,
            Event::Closed(_) => return Err("the server closed the notification surface".into()),
            Event::Pointer { event, .. } => self.pointer(event)?,
            Event::Key {
                key: code, pressed, ..
            } => {
                if pressed && code == key::ESCAPE && self.centre_open {
                    if self.menu_open {
                        self.menu_open = false;
                        self.client.request_redraw(self.surface)?;
                    } else {
                        self.toggle_centre()?;
                    }
                }
            }
            Event::ActivationToken { request, token } => self.complete(request, token),
            Event::Appearance { .. } | Event::DisplayOptions(_) => {
                // layouts and rendered cards belong to the style they were made with
                let options = self.client.display_options();
                self.renderer =
                    banner_renderer(&self.theme, &self.assets, self.client.is_dark(), options)?;
                self.server.center().config.motion.reduce_motion = options.reduce_motion;
                self.images.clear();
                self.client.request_redraw(self.surface)?;
            }
            // the server draws the lock screen: only the open list has to go
            Event::SessionLock { locked: true } => {
                if self.centre_open {
                    self.toggle_centre()?;
                }
            }
            Event::SessionLock { locked: false } => {}
            // the surface follows the output the server chose for it; it has no popups or menu-bar role
            Event::OutputsChanged
            | Event::PopupDone(_)
            | Event::MenuBar { .. }
            | Event::Dock { .. }
            | Event::KeyboardLeave(_) => {}
        }
        Ok(())
    }

    /// Expire and animate the model, send its signals, handle `ToggleCentre` and ask for a
    /// frame when something changed. Returns how long the loop may sleep.
    fn housekeeping(&mut self) -> Result<Option<Duration>> {
        if self.toggle.swap(false, Ordering::SeqCst) {
            self.toggle_centre()?;
        }
        let now = self.server.now();
        let (revision, wakeup) = {
            let mut c = self.server.center();
            c.tick(now);
            (c.revision(), c.next_wakeup(now))
        };
        self.server.flush_events()?;
        if revision != self.seen_revision || wakeup == Some(Wakeup::Frame) {
            self.seen_revision = revision;
            self.client.request_redraw(self.surface)?;
        }
        Ok(match wakeup {
            Some(Wakeup::At(t)) => Some(Duration::from_secs_f64((t - now).max(0.0))),
            // frame callbacks drive the animation
            Some(Wakeup::Frame) | None => None,
        })
    }

    /// Open or close the list. While it is open the surface takes the keyboard (Escape closes).
    fn toggle_centre(&mut self) -> Result<()> {
        self.centre_open = !self.centre_open;
        self.centre_hover = None;
        self.menu_open = false;
        self.clear_hover = false;
        if !self.centre_open {
            self.cards.clear();
        }
        let mode = if self.centre_open {
            KeyboardInteractivity::Exclusive
        } else {
            KeyboardInteractivity::None
        };
        self.client.set_keyboard_interactivity(self.surface, mode)?;
        self.client.request_redraw(self.surface)?;
        Ok(())
    }

    /// Lay out this frame's banners and cards, sync their backdrops and the input region, and
    /// draw them.
    fn draw(&mut self) -> Result<()> {
        let now = self.server.now();
        let (width, height) = self.client.size(self.surface)?;
        let surface_size = Size::new(f64::from(width), f64::from(height));
        let mut items = Vec::new();
        {
            let mut c = self.server.center();
            c.tick(now);
            self.placements = c.placements(surface_size, &mut self.renderer, now);
            // the open list replaces the banners on screen; they stay in the model and in the list
            for pl in self.placements.iter().filter(|_| !self.centre_open) {
                if let Some(note) = c.get(pl.id).cloned() {
                    items.push(Item {
                        slot: Slot::Banner(pl.id),
                        rect: pl.rect,
                        opacity: pl.opacity,
                        hovered: pl.hovered,
                        note,
                    });
                }
            }
            if self.centre_open {
                let layout = c.config.layout.clone();
                let history: Vec<Notification> = c.history().into_iter().cloned().collect();
                let renderer = &mut self.renderer;
                let scopes = c.clear_scopes();
                let header = centre_header(&layout, surface_size, renderer.clear_button_width());
                let labels: Vec<&str> = scopes.iter().map(|s| s.label()).collect();
                self.menu = self.menu_open.then(|| {
                    (
                        clear_menu(header.clear, scopes.len(), renderer.widest_label(&labels)),
                        scopes,
                    )
                });
                self.header = Some(header);
                self.cards = centre_cards(
                    history
                        .iter()
                        .map(|n| (n.id, renderer.layout_cached(n, layout.width).size.height, 1)),
                    &GroupStack::default(),
                    &layout,
                    surface_size,
                );
                for (id, rect) in &self.cards {
                    let note = history
                        .iter()
                        .find(|n| n.id == *id)
                        .expect("card of a history entry")
                        .clone();
                    items.push(Item {
                        slot: Slot::Card(*id),
                        rect: *rect,
                        opacity: 1.0,
                        hovered: self.centre_hover == Some(*id),
                        note,
                    });
                }
            }
        }
        if !self.centre_open {
            self.header = None;
            self.menu = None;
        }
        items.retain(|i| i.opacity > MIN_OPACITY);
        let live: HashSet<Slot> = items.iter().map(|i| i.slot).collect();
        self.backdrops.retain(|slot, _| live.contains(slot));
        self.images.retain(|slot, _| live.contains(slot));
        let radius = self.renderer.style.corner_radius;
        for item in &items {
            if !self.backdrops.contains_key(&item.slot) {
                let backdrop = self.client.backdrop(self.surface)?;
                backdrop.set_material(BannerStyle::DEFAULT_MATERIAL);
                self.backdrops.insert(item.slot, backdrop);
            }
            let backdrop = &self.backdrops[&item.slot];
            backdrop.set_shape(&item.rect, radius)?;
            backdrop.set_alpha(item.opacity.clamp(0.0, 1.0))?;
        }
        // the Clear menu is a panel of the same material as a card, under its own backdrop
        match &self.menu {
            Some((menu, _)) => {
                if self.menu_backdrop.is_none() {
                    let backdrop = self.client.backdrop(self.surface)?;
                    backdrop.set_material(BannerStyle::DEFAULT_MATERIAL);
                    self.menu_backdrop = Some(backdrop);
                }
                if let Some(backdrop) = &self.menu_backdrop {
                    backdrop.set_shape(&menu.panel, MENU_RADIUS)?;
                }
            }
            None => self.menu_backdrop = None,
        }
        let banners: Vec<Rect> = items
            .iter()
            .filter(|i| matches!(i.slot, Slot::Banner(_)))
            .map(|i| i.rect)
            .collect();
        self.client.set_input_region(
            self.surface,
            &input_region(&banners, self.centre_open, surface_size),
        )?;

        let (renderer, images) = (&mut self.renderer, &mut self.images);
        let (header, menu, clear_hover, menu_hover) =
            (self.header, &self.menu, self.clear_hover, self.menu_hover);
        self.client.draw(self.surface, |pixmap, scale| {
            let factor = scale.factor();
            if let Some(header) = &header {
                let image = renderer.render_header(header, clear_hover || menu.is_some(), factor);
                let (x, y) = (
                    (header.row.x() * f64::from(factor)).round() as i32,
                    (header.row.y() * f64::from(factor)).round() as i32,
                );
                pixmap.draw_pixmap(
                    x,
                    y,
                    image.as_ref(),
                    &PixmapPaint::default(),
                    Transform::identity(),
                    None,
                );
            }
            for item in &items {
                let layout = renderer
                    .layout_cached(&item.note, item.rect.width())
                    .clone();
                let signature: Signature = (
                    item.note.id,
                    item.note.revision,
                    item.hovered,
                    layout.size.width.to_bits(),
                    layout.size.height.to_bits(),
                    factor.to_bits(),
                );
                if images
                    .get(&item.slot)
                    .is_none_or(|(sig, _)| *sig != signature)
                {
                    let image = renderer.render(
                        &item.note,
                        &layout,
                        &RenderOptions {
                            scale: factor,
                            hovered: item.hovered,
                            backdrop: None,
                            layers: LAYERS,
                        },
                    );
                    images.insert(item.slot, (signature, image));
                }
                let (_, image) = &images[&item.slot];
                let [left, top, ..] = image.margins;
                let x = (item.rect.x() * f64::from(factor)).round() as i32 - left as i32;
                let y = (item.rect.y() * f64::from(factor)).round() as i32 - top as i32;
                let paint = PixmapPaint {
                    opacity: item.opacity.clamp(0.0, 1.0) as f32,
                    ..PixmapPaint::default()
                };
                pixmap.draw_pixmap(
                    x,
                    y,
                    image.pixmap.as_ref(),
                    &paint,
                    Transform::identity(),
                    None,
                );
            }
            if let Some((menu, scopes)) = menu {
                let labels: Vec<&str> = scopes.iter().map(|s| s.label()).collect();
                let image = renderer.render_clear_menu(menu, &labels, menu_hover, factor);
                let (x, y) = (
                    (menu.panel.x() * f64::from(factor)).round() as i32,
                    (menu.panel.y() * f64::from(factor)).round() as i32,
                );
                pixmap.draw_pixmap(
                    x,
                    y,
                    image.as_ref(),
                    &PixmapPaint::default(),
                    Transform::identity(),
                    None,
                );
            }
        })?;
        Ok(())
    }

    fn pointer(&mut self, event: PointerEvent) -> Result<()> {
        let now = self.server.now();
        match event {
            PointerEvent::Enter(p) | PointerEvent::Motion(p) => self.motion(p, now)?,
            PointerEvent::Leave => {
                if self.drag.is_none() {
                    self.hovered = None;
                    self.server.center().set_hover(None, now);
                }
                self.centre_hover = None;
                self.client.request_redraw(self.surface)?;
            }
            PointerEvent::Button {
                button: Button::Left,
                pressed: true,
                position,
            } => self.press(position, now)?,
            PointerEvent::Button {
                button: Button::Left,
                pressed: false,
                ..
            } => self.release(now)?,
            PointerEvent::Button { .. } | PointerEvent::Scroll { .. } => {}
        }
        Ok(())
    }

    /// Hover over a card or a banner (hover pauses the timeout); an active drag swipes.
    fn motion(&mut self, p: Point, now: f64) -> Result<()> {
        if self.centre_open {
            let clear_hover = self.header.is_some_and(|h| h.clear.contains(p));
            let menu_hover = self
                .menu
                .as_ref()
                .and_then(|(m, _)| m.rows.iter().position(|r| r.contains(p)));
            if clear_hover != self.clear_hover || menu_hover != self.menu_hover {
                self.clear_hover = clear_hover;
                self.menu_hover = menu_hover;
                self.client.request_redraw(self.surface)?;
            }
            let under = card_at(&self.cards, p).map(|(id, _)| id);
            if under != self.centre_hover {
                self.centre_hover = under;
                self.client.request_redraw(self.surface)?;
            }
            if under.is_some() {
                return Ok(());
            }
        }
        if let Some(d) = &self.drag {
            self.server
                .center()
                .swipe_update(d.id, p.x - d.start_x, now);
            self.client.request_redraw(self.surface)?;
            return Ok(());
        }
        let under = self.server.center().banner_at(p);
        if under != self.hovered {
            self.hovered = under;
            self.server.center().set_hover(under, now);
            self.client.request_redraw(self.surface)?;
        }
        Ok(())
    }

    fn press(&mut self, p: Point, now: f64) -> Result<()> {
        if self.centre_open {
            let menu = self.menu.as_ref().map(|(m, _)| m);
            let press = self
                .header
                .map_or(HeaderPress::Elsewhere, |h| header_press(&h, menu, p));
            match press {
                HeaderPress::ToggleMenu => self.menu_open = !self.menu_open,
                HeaderPress::Choose(i) => {
                    let scope = self.menu.as_ref().expect("a menu was pressed").1[i];
                    self.server.center().clear_scope(scope, now);
                    self.menu_open = false;
                }
                HeaderPress::CloseMenu => self.menu_open = false,
                HeaderPress::Elsewhere => {}
            }
            if press != HeaderPress::Elsewhere {
                self.client.request_redraw(self.surface)?;
                return Ok(());
            }
            if let Some((id, rect)) = card_at(&self.cards, p) {
                match card_press(self.hit(id, rect, p, true)) {
                    CardPress::Clear => {
                        self.server.center().clear(id, now);
                    }
                    CardPress::Invoke(key) => self.begin_activation(Pending::Invoke(id, key))?,
                    CardPress::OpenLink(href) => open_link(&href),
                    CardPress::Activate => {
                        self.begin_activation(Pending::Activate(id))?;
                        self.toggle_centre()?;
                    }
                }
                self.client.request_redraw(self.surface)?;
                return Ok(());
            }
            // a press outside the cards closes the list and is then handled as a press on the banners
            self.toggle_centre()?;
        }
        let Some(id) = self.server.center().banner_at(p) else {
            return Ok(());
        };
        let Some((rect, hovered)) = self
            .placements
            .iter()
            .find(|pl| pl.id == id)
            .map(|pl| (pl.rect, pl.hovered))
        else {
            return Ok(());
        };
        match banner_press(self.hit(id, rect, p, hovered)) {
            BannerPress::Dismiss => self.server.center().dismiss_banner(id, now),
            BannerPress::Invoke(key) => self.begin_activation(Pending::Invoke(id, key))?,
            BannerPress::OpenLink(href) => open_link(&href),
            BannerPress::BeginSwipe => {
                self.server.center().swipe_begin(id, now);
                self.drag = Some(Drag { id, start_x: p.x });
            }
        }
        self.client.request_redraw(self.surface)?;
        Ok(())
    }

    fn release(&mut self, now: f64) -> Result<()> {
        let Some(d) = self.drag.take() else {
            return Ok(());
        };
        let outcome = self.server.center().swipe_end(d.id, now);
        match outcome {
            SwipeOutcome::Click => self.begin_activation(Pending::Activate(d.id))?,
            SwipeOutcome::Dismissed | SwipeOutcome::Cancelled => {}
        }
        self.client.request_redraw(self.surface)?;
        Ok(())
    }

    /// What a press at `p` on the card `id` at `rect` hit; nothing when the notification is gone.
    fn hit(&mut self, id: u32, rect: Rect, p: Point, close_visible: bool) -> BannerHit {
        let Some(note) = self.server.center().get(id).cloned() else {
            return BannerHit::None;
        };
        self.renderer
            .layout_cached(&note, rect.width())
            .hit(local_to(rect, p), close_visible)
    }

    /// Fetch an activation token for the click; the action runs when it arrives.
    fn begin_activation(&mut self, pending: Pending) -> Result<()> {
        let request = self.client.request_activation_token(self.surface)?;
        self.pending.insert(request, pending);
        Ok(())
    }

    /// Run the action that waited for `token`; the signals carry it to the application.
    fn complete(&mut self, request: cupertino_client::ActivationRequest, token: String) {
        let pending = self
            .pending
            .remove(&request)
            .expect("token for a request this process made");
        let now = self.server.now();
        let mut c = self.server.center();
        // the notification may have expired while the token was fetched: nothing to run then
        match pending {
            Pending::Activate(id) => {
                c.activate(id, Some(token), now);
            }
            Pending::Invoke(id, key) => {
                c.invoke_action(id, &key, Some(token), now);
            }
        }
    }
}
