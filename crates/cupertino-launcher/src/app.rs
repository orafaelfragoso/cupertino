//! Glue between the panel model and the toolkit surface: draws the panel over a server-rendered
//! backdrop, routes keys and pointer input to the model and starts what it activates. The key
//! translation lives in `cupertino_launcher::surface`.

use cupertino_assets::{AssetError, Assets};
use cupertino_client::{
    ActivationRequest, Anchor, Backdrop, Button, Client, DisplayOptions, Event,
    KeyboardInteractivity, Layer, LayerSurfaceConfig, Margin, OutputGeometry, OutputId,
    PointerEvent, SurfaceId,
};
use cupertino_geometry::{Point, Rect, Size};
use cupertino_launcher::control;
use cupertino_launcher::exec::CommandSpec;
use cupertino_launcher::files::{FileIndex, FileSearchConfig};
use cupertino_launcher::panel::{Focus, Now, Panel, PanelCommand, PanelEvent};
use cupertino_launcher::render::{
    Layers, PanelImage, PanelLayout, PanelRenderer, PanelStyle, RenderOptions, SurfaceGeometry,
};
use cupertino_launcher::surface::panel_key;
use cupertino_launcher::{Action, Activation, Launcher, LauncherConfig};
use cupertino_text_field::{KeyInput, Keymap, Response, TextField};
use cupertino_theme::{Appearance, Theme};
use std::collections::HashMap;
use std::error::Error;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tiny_skia::{PixmapPaint, Transform};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Role of the surfaces for the server (`wlr-layer-shell` namespace).
const NAMESPACE: &str = "cupertino-launcher";

/// The panel is neither drawn nor given a backdrop below this opacity.
const MIN_OPACITY: f64 = 0.002;

/// Shadow and content; the material is the server's backdrop.
const LAYERS: Layers = Layers {
    shadow: true,
    background: false,
    content: true,
};

/// Command line of the process.
pub struct Options {
    /// Index the home directory for file results.
    pub files: bool,
    /// Terminal emulator that runs entries with `Terminal=true`; the launcher default when absent.
    pub terminal: Option<String>,
}

impl Options {
    /// Parse the arguments; the problem is returned as the error.
    pub fn parse(args: impl Iterator<Item = String>) -> std::result::Result<Self, String> {
        let mut options = Options {
            files: true,
            terminal: None,
        };
        let mut args = args;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--no-files" => options.files = false,
                "--terminal" => {
                    options.terminal = Some(args.next().ok_or("--terminal needs a program")?)
                }
                _ => return Err(format!("unknown argument {arg}")),
            }
        }
        Ok(options)
    }
}

/// What decides whether the rendered content can be reused: everything the image shows.
#[derive(PartialEq)]
struct Signature {
    layout: PanelLayout,
    query: String,
    caret: usize,
    field_selection: std::ops::Range<usize>,
    field_scroll: u64,
    caret_visible: bool,
    selected: usize,
    focus: Focus,
    titles: Vec<String>,
    scale: u32,
}

struct App {
    client: Client,
    theme: Theme,
    assets: Assets,
    /// Holds the panel at its largest size, anchored at the top; mapped all the time, input only
    /// while the panel is shown.
    surface: SurfaceId,
    /// Full-output surface below the panel that catches a press outside it; exists only while the
    /// panel is shown, once the server has placed the panel.
    catcher: Option<SurfaceId>,
    /// The output of both surfaces: the one the server put the panel on, or the guess the panel
    /// surface was sized for until the server has placed it.
    output: OutputId,
    output_geometry: OutputGeometry,
    geometry: SurfaceGeometry,
    launcher: Launcher,
    panel: Panel,
    renderer: PanelRenderer,
    /// Set by the `Toggle` D-Bus call.
    toggle: Arc<AtomicBool>,
    start: Instant,
    /// Where the file index is built from; `None` with `--no-files`.
    files: Option<FileSearchConfig>,
    index: Option<JoinHandle<FileIndex>>,
    /// Layout and rectangle (panel-surface-local points) of the last drawn frame, for hit testing.
    last: Option<(PanelLayout, Rect)>,
    backdrop: Option<Backdrop>,
    image: Option<(Signature, PanelImage)>,
    /// The user's modifier profile and editing chords, for the query field.
    keymap: Keymap,
    /// The x of each caret position of the query as last drawn (points from the text's start).
    field_offsets: Vec<f64>,
    /// Whether the last frame showed the caret; a blink redraws when it changes.
    caret_drawn: bool,
    /// Commands waiting for the activation token being fetched.
    pending: HashMap<ActivationRequest, CommandSpec>,
    waker: cupertino_client::Waker,
}

/// The first output the server has described: the guess a panel surface without an output is
/// sized for.
fn first_output(client: &Client) -> Result<(OutputId, OutputGeometry)> {
    Ok(client
        .outputs()
        .into_iter()
        .find_map(|o| o.geometry.map(|g| (o.id, g)))
        .ok_or("the server has no output")?)
}

/// The panel renderer for the session's appearance and display options.
fn panel_renderer(
    theme: &Theme,
    assets: &Assets,
    dark: bool,
    options: DisplayOptions,
) -> std::result::Result<PanelRenderer, AssetError> {
    let appearance = Appearance::of_session(dark, options.increase_contrast);
    PanelRenderer::new(
        PanelStyle::from_theme(theme, appearance, options.reduce_transparency),
        assets,
    )
}

fn output_size(geometry: OutputGeometry) -> Size {
    Size::new(f64::from(geometry.width), f64::from(geometry.height))
}

/// The panel surface: over the menu bar's zone, as large as `geometry` says, at the top of `output`;
/// without one the server puts it on the output under the pointer.
fn panel_surface(
    client: &mut Client,
    output: Option<OutputId>,
    geometry: &SurfaceGeometry,
    keyboard: KeyboardInteractivity,
) -> Result<SurfaceId> {
    Ok(client.layer_surface(&LayerSurfaceConfig {
        namespace: NAMESPACE.to_string(),
        output,
        layer: Layer::Overlay,
        anchor: Anchor::Top,
        size: geometry.size,
        margin: Margin {
            top: geometry.top_margin,
            ..Margin::default()
        },
        exclusive_zone: -1,
        keyboard,
    })?)
}

/// Connect, claim the bus name and serve until the connection or the bus fails.
pub fn run(options: &Options) -> Result<std::convert::Infallible> {
    let assets = Assets::discover();
    let theme = Theme::load()?;
    let mut client = Client::connect_shell()?;
    let renderer = panel_renderer(&theme, &assets, client.is_dark(), client.display_options())?;
    let mut panel = Panel::new();
    panel.max_rows = renderer.style.max_rows;
    let mut config = LauncherConfig::default();
    if let Some(terminal) = &options.terminal {
        config.terminal_command = vec![terminal.clone(), "-e".into()];
    }
    let files = if options.files {
        Some(
            FileSearchConfig::home()
                .ok_or("HOME is not set (start with --no-files to skip file results)")?,
        )
    } else {
        None
    };

    let waker = client.waker();
    let toggle = Arc::new(AtomicBool::new(false));
    let (flag, wake) = (toggle.clone(), waker.clone());
    let _control = control::serve(zbus::connection::Builder::session()?, move || {
        flag.store(true, Ordering::SeqCst);
        wake.wake();
    })
    .map_err(|e| match e {
        zbus::Error::NameTaken => "another launcher owns org.cupertino.Launcher1".into(),
        e => Box::new(e) as Box<dyn Error>,
    })?;
    let (output, output_geometry) = first_output(&client)?;
    let geometry = renderer
        .style
        .surface_geometry(output_size(output_geometry));
    let surface = panel_surface(&mut client, None, &geometry, KeyboardInteractivity::None)?;
    let mut app = App {
        client,
        theme,
        assets,
        surface,
        catcher: None,
        output,
        output_geometry,
        geometry,
        launcher: Launcher::from_system(config),
        panel,
        renderer,
        toggle,
        start: Instant::now(),
        files,
        index: None,
        last: None,
        backdrop: None,
        image: None,
        keymap: Keymap::load(),
        field_offsets: Vec::new(),
        caret_drawn: false,
        pending: HashMap::new(),
        waker,
    };
    loop {
        let blink = app.blink_due();
        for event in app.client.wait(blink)? {
            app.handle(event)?;
        }
        app.housekeeping()?;
    }
}

fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the system clock is before the Unix epoch")
        .as_secs_f64()
}

/// Run a command detached from the loop; its exit is reaped on a thread.
fn spawn(mut command: Command) {
    let program = command.get_program().to_string_lossy().into_owned();
    match command.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                if let Err(e) = child.wait() {
                    eprintln!("cupertino-launcher: {program}: {e}");
                }
            });
        }
        Err(e) => eprintln!("cupertino-launcher: cannot run {program}: {e}"),
    }
}

/// Open a path or URL with the default handler.
fn open(target: &str) {
    let mut command = Command::new("xdg-open");
    command.arg(target);
    spawn(command);
}

/// Percent-encode `text` for a URL query.
fn percent_encode(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

impl App {
    fn now(&self) -> Now {
        Now {
            mono: self.start.elapsed().as_secs_f64(),
            unix: unix_now(),
        }
    }

    fn handle(&mut self, event: Event) -> Result<()> {
        match event {
            Event::Configured { surface, .. } if surface == self.surface => self.adopt_output()?,
            // a draw follows every configure and scale change
            Event::Configured { .. } | Event::ScaleChanged { .. } => {}
            Event::Draw(surface) if surface == self.surface => self.draw()?,
            Event::Draw(surface) if Some(surface) == self.catcher => self.draw_catcher(surface)?,
            // of a catcher destroyed since
            Event::Draw(_) => {}
            // their output went away
            Event::Closed(surface) if surface == self.surface || Some(surface) == self.catcher => {
                self.output_gone()?
            }
            Event::Closed(_) => return Err("the server closed a launcher surface".into()),
            Event::Pointer { surface, event } if surface == self.surface => self.pointer(event)?,
            Event::Pointer {
                surface,
                event: PointerEvent::Button { pressed: true, .. },
            } if Some(surface) == self.catcher => self.hide()?,
            Event::Pointer { .. } => {}
            Event::Key {
                key,
                pressed: true,
                mods,
                character,
                ..
            } if self.panel.is_visible() => {
                let now = self.now();
                let outcome = match panel_key(key, mods, character) {
                    Some((panel_key, panel_mods)) => {
                        self.panel
                            .handle_key(panel_key, panel_mods, &mut self.launcher, now)
                    }
                    None => PanelEvent::Edit,
                };
                if outcome == PanelEvent::Edit {
                    let input =
                        KeyInput::new(key, mods.shift, mods.ctrl, mods.alt, mods.logo, character);
                    self.field_key(input)?;
                } else {
                    self.outcome(outcome)?;
                }
            }
            Event::Key { .. } => {}
            // focus moved to a window: the panel loses its reason to stay open
            Event::KeyboardLeave(_) if self.panel.is_visible() => self.hide()?,
            Event::KeyboardLeave(_) => {}
            Event::ActivationToken { request, token } => self.complete(request, token),
            Event::OutputsChanged => self.follow_output()?,
            Event::Appearance { .. } | Event::DisplayOptions(_) => {
                self.renderer = panel_renderer(
                    &self.theme,
                    &self.assets,
                    self.client.is_dark(),
                    self.client.display_options(),
                )?;
                // the material differs between the appearances and options, and the shadow margins may
                if self
                    .renderer
                    .style
                    .surface_geometry(output_size(self.output_geometry))
                    == self.geometry
                {
                    self.backdrop = None;
                    self.last = None;
                    self.image = None;
                    self.client.request_redraw(self.surface)?;
                } else {
                    self.place_surfaces(Some(self.output))?;
                }
            }
            // the server draws the lock screen: only the open panel has to go
            Event::SessionLock { locked: true } if self.panel.is_visible() => self.hide()?,
            Event::SessionLock { .. } | Event::LockRequested { .. } | Event::LogOutRequested => {}
            // the surfaces have no popups or menu-bar role
            Event::PopupDone(_) | Event::MenuBar { .. } | Event::Dock { .. } => {}
        }
        Ok(())
    }

    /// Place the surfaces again when the geometry of the output changed. The panel surface cannot
    /// be moved or resized, so it is replaced, with the catcher if the panel is shown.
    fn follow_output(&mut self) -> Result<()> {
        // a gone output closes the surfaces on it first (`output_gone`)
        let Some(geometry) = self
            .client
            .outputs()
            .into_iter()
            .find(|o| o.id == self.output)
            .and_then(|o| o.geometry)
        else {
            return Ok(());
        };
        if geometry == self.output_geometry {
            return Ok(());
        }
        self.output_geometry = geometry;
        self.place_surfaces(Some(self.output))
    }

    /// The server placed the panel surface. Its output becomes the launcher's; when the surface was
    /// sized for another output size, it is replaced on that output. The catcher of a shown panel
    /// goes on the same output.
    fn adopt_output(&mut self) -> Result<()> {
        let output = *self
            .client
            .surface_outputs(self.surface)?
            .first()
            .ok_or("the server placed the panel on no output")?;
        let geometry = self
            .client
            .outputs()
            .into_iter()
            .find(|o| o.id == output)
            .and_then(|o| o.geometry)
            .ok_or("the output of the panel is not described")?;
        self.output = output;
        self.output_geometry = geometry;
        if self.renderer.style.surface_geometry(output_size(geometry)) != self.geometry {
            return self.place_surfaces(Some(output));
        }
        if self.panel.is_visible() && self.catcher.is_none() {
            self.catcher = Some(self.new_catcher()?);
        }
        Ok(())
    }

    /// The output of the surfaces went away and the server closed them: they start again on the
    /// output the server chooses.
    fn output_gone(&mut self) -> Result<()> {
        (self.output, self.output_geometry) = first_output(&self.client)?;
        self.place_surfaces(None)
    }

    /// Replace the panel surface (and the catcher, if the panel is shown) with ones that fit the geometry the style
    /// gives for the output, on `output`; without one, the server chooses and the catcher follows once it has
    /// (`adopt_output`).
    fn place_surfaces(&mut self, output: Option<OutputId>) -> Result<()> {
        self.geometry = self
            .renderer
            .style
            .surface_geometry(output_size(self.output_geometry));
        // the backdrop goes before its surface
        self.backdrop = None;
        self.last = None;
        self.image = None;
        self.client.destroy_surface(self.surface)?;
        let visible = self.panel.is_visible();
        let keyboard = if visible {
            KeyboardInteractivity::Exclusive
        } else {
            KeyboardInteractivity::None
        };
        self.surface = panel_surface(&mut self.client, output, &self.geometry, keyboard)?;
        if let Some(catcher) = self.catcher.take() {
            self.client.destroy_surface(catcher)?;
            if output.is_some() {
                self.catcher = Some(self.new_catcher()?);
            }
        }
        Ok(())
    }

    /// The outside-click catcher: transparent, over the whole output and just below the panel.
    fn new_catcher(&mut self) -> Result<SurfaceId> {
        Ok(self.client.layer_surface(&LayerSurfaceConfig {
            namespace: NAMESPACE.to_string(),
            output: Some(self.output),
            layer: Layer::Top,
            anchor: Anchor::Top | Anchor::Right | Anchor::Bottom | Anchor::Left,
            size: (0, 0),
            margin: Margin::default(),
            exclusive_zone: -1,
            keyboard: KeyboardInteractivity::None,
        })?)
    }

    /// Handle `Toggle` and the finished file index.
    /// A key of the query field; the clipboard requests go to the system clipboard.
    fn field_key(&mut self, input: KeyInput) -> Result<()> {
        let now = self.now();
        let keymap = &mut self.keymap;
        let response = self.panel.edit(&self.launcher, now, |f| {
            f.key(keymap, input, Instant::now())
        });
        match response {
            Response::Ignored => return Ok(()),
            Response::Moved | Response::Edited => {}
            Response::Copy(text) | Response::Cut(text) => {
                if let Err(e) = self.client.set_clipboard_text(text) {
                    eprintln!("cupertino-launcher: copy: {e}");
                }
            }
            Response::Paste => match self.client.clipboard_text() {
                Ok(Some(text)) => self.panel.edit(&self.launcher, now, |f| f.paste(&text)),
                Ok(None) => {}
                Err(e) => eprintln!("cupertino-launcher: paste: {e}"),
            },
        }
        self.client.request_redraw(self.surface)?;
        Ok(())
    }

    /// Whether the query field's caret shows now: the panel is open with the keyboard in the field and the blink is on.
    fn caret_visible(&self) -> bool {
        self.panel.is_visible()
            && self.panel.focus() == Focus::Field
            && self.panel.field().caret_visible(Instant::now())
    }

    /// Until the caret's next blink while it can show.
    fn blink_due(&self) -> Option<std::time::Duration> {
        (self.panel.is_visible() && self.panel.focus() == Focus::Field)
            .then(|| self.panel.field().next_blink(Instant::now()))
            .flatten()
    }

    fn housekeeping(&mut self) -> Result<()> {
        if self.caret_visible() != self.caret_drawn {
            self.client.request_redraw(self.surface)?;
        }
        if self.toggle.swap(false, Ordering::SeqCst) {
            if self.panel.is_visible() {
                self.hide()?;
            } else {
                self.show()?;
            }
        }
        if self.index.as_ref().is_some_and(JoinHandle::is_finished) {
            let index = self
                .index
                .take()
                .expect("checked above")
                .join()
                .map_err(|_| "the file index thread panicked")?;
            self.launcher.set_file_index(Some(Arc::new(index)));
            let now = self.now();
            self.panel.refresh(&self.launcher, now);
            self.client.request_redraw(self.surface)?;
        }
        Ok(())
    }

    /// Open the panel with an empty query on the output under the pointer; it takes the keyboard
    /// while it is shown.
    fn show(&mut self) -> Result<()> {
        let now = self.now();
        self.panel.show(&self.launcher, now);
        if let (Some(config), None, None) = (&self.files, &self.index, self.launcher.file_index()) {
            let (config, waker) = (config.clone(), self.waker.clone());
            self.index = Some(std::thread::spawn(move || {
                let index = FileIndex::build(config);
                waker.wake();
                index
            }));
        }
        // a new panel surface, with the keyboard, on the output under the pointer
        self.place_surfaces(None)
    }

    /// Close the panel, drop the catcher and give the keyboard back; the launch history is written.
    fn hide(&mut self) -> Result<()> {
        let now = self.now();
        self.panel.hide(now.mono);
        if let Err(e) = self.launcher.frecency.save_if_dirty() {
            eprintln!("cupertino-launcher: launch history not saved: {e}");
        }
        if let Some(catcher) = self.catcher.take() {
            self.client.destroy_surface(catcher)?;
        }
        self.client
            .set_keyboard_interactivity(self.surface, KeyboardInteractivity::None)?;
        self.client.request_redraw(self.surface)?;
        Ok(())
    }

    /// Carry out what the panel asked for.
    fn outcome(&mut self, outcome: PanelEvent) -> Result<()> {
        match outcome {
            // field keys go to `field_key` before an outcome is handled
            PanelEvent::None | PanelEvent::Changed | PanelEvent::Edit => {}
            PanelEvent::Close => self.hide()?,
            PanelEvent::Activate(activation) => {
                // the selection is set while the panel still has the keyboard, with the serial of the key or click
                // that chose the result; the launcher serves it until another selection replaces it
                if let Activation::CopyToClipboard(text) = &activation {
                    if let Err(e) = self.client.set_clipboard_text(text.clone()) {
                        eprintln!("cupertino-launcher: copy: {e}");
                    }
                }
                self.hide()?;
                match activation {
                    Activation::Run(spec) => self.begin_launch(spec)?,
                    Activation::CopyToClipboard(_) => {}
                    Activation::Nothing => {
                        eprintln!("cupertino-launcher: the entry has no usable command")
                    }
                }
            }
            PanelEvent::Command(command) => self.command(command)?,
        }
        self.client.request_redraw(self.surface)?;
        Ok(())
    }

    /// Shortcut commands of the panel that need the host.
    fn command(&mut self, command: PanelCommand) -> Result<()> {
        match command {
            PanelCommand::Reveal => {
                if let Some(Action::OpenPath(path)) =
                    self.panel.selected_item().map(|item| item.action.clone())
                {
                    let directory = if path.is_dir() {
                        path
                    } else {
                        path.parent().map(|p| p.to_path_buf()).unwrap_or(path)
                    };
                    self.hide()?;
                    open(&directory.to_string_lossy());
                }
            }
            PanelCommand::WebSearch => {
                match (&self.launcher.config.web_search_url, self.panel.query()) {
                    (Some(url), query) if !query.is_empty() => {
                        let target = url.replace("{}", &percent_encode(query));
                        self.hide()?;
                        open(&target);
                    }
                    _ => eprintln!(
                        "cupertino-launcher: no web search URL configured (`web_search_url`)"
                    ),
                }
            }
            other => eprintln!("cupertino-launcher: {other:?} is not provided"),
        }
        Ok(())
    }

    /// Fetch an activation token for the launch; the command starts when it arrives. The token of
    /// an application names its desktop entry, which is how the docks learn it is launching.
    fn begin_launch(&mut self, spec: CommandSpec) -> Result<()> {
        let request = match spec.desktop_id.as_deref() {
            Some(id) => self.client.request_launch_token(self.surface, id)?,
            None => self.client.request_activation_token(self.surface)?,
        };
        self.pending.insert(request, spec);
        Ok(())
    }

    /// Start the command that waited for `token`; the application uses it to take the focus.
    fn complete(&mut self, request: ActivationRequest, token: String) {
        let spec = self
            .pending
            .remove(&request)
            .expect("token for a request this process made");
        spawn(spec.with_activation_token(&token).to_command());
    }

    /// Draw the panel with the backdrop of its material, or clear the surface when it is gone.
    fn draw(&mut self) -> Result<()> {
        let now = self.now().mono;
        let opacity = self.panel.progress(now).clamp(0.0, 1.0);
        let animating = self.panel.is_animating(now);
        if !(self.panel.is_visible() || animating) || opacity <= MIN_OPACITY {
            self.last = None;
            self.image = None;
            self.backdrop = None;
            self.client.set_input_region(self.surface, &[])?;
            self.client.draw(self.surface, |_, _| {})?;
            // the first frames of the appear spring are still invisible
            if animating {
                self.client.request_redraw(self.surface)?;
            }
            return Ok(());
        }
        let layout = self.renderer.layout(&self.panel);
        let rect = Rect::from_origin_size(self.geometry.origin, layout.size);
        self.last = Some((layout.clone(), rect));
        if self.backdrop.is_none() {
            let backdrop = self.client.backdrop(self.surface)?;
            backdrop.set_material(&self.renderer.style.material_name);
            self.backdrop = Some(backdrop);
        }
        let backdrop = self.backdrop.as_ref().expect("created above");
        backdrop.set_shape(&rect, self.renderer.style.corner_radius)?;
        backdrop.set_alpha(opacity)?;
        // the panel takes input while shown; a press outside it reaches the catcher
        let region = if self.panel.is_visible() {
            vec![rect]
        } else {
            Vec::new()
        };
        self.client.set_input_region(self.surface, &region)?;

        let caret_visible = self.caret_visible();
        self.caret_drawn = caret_visible;
        let (renderer, panel, image, field_offsets) = (
            &mut self.renderer,
            &mut self.panel,
            &mut self.image,
            &mut self.field_offsets,
        );
        self.client.draw(self.surface, |pixmap, scale| {
            let factor = scale.factor();
            let offsets = renderer.field_offsets(panel, factor);
            let caret_x = caret_x(panel.field(), &offsets);
            let width = renderer.field_area(&layout).width();
            panel.follow_caret(caret_x, offsets.last().copied().unwrap_or(0.0), width);
            *field_offsets = offsets;
            let signature = Signature {
                layout: layout.clone(),
                query: panel.query().to_string(),
                caret: panel.field().caret(),
                field_selection: panel.field().selection(),
                field_scroll: panel.field().scroll().to_bits(),
                caret_visible,
                selected: panel.selected(),
                focus: panel.focus(),
                titles: panel
                    .results()
                    .iter()
                    .map(|item| item.title.clone())
                    .collect(),
                scale: factor.to_bits(),
            };
            if image
                .as_ref()
                .is_none_or(|(cached, _)| *cached != signature)
            {
                let rendered = renderer.render(
                    panel,
                    &layout,
                    &RenderOptions {
                        scale: factor,
                        caret_visible,
                        backdrop: None,
                        layers: LAYERS,
                    },
                );
                *image = Some((signature, rendered));
            }
            let (_, rendered) = image.as_ref().expect("rendered above");
            let [left, top, ..] = rendered.margins;
            let x = (rect.x() * f64::from(factor)).round() as i32 - left as i32;
            let y = (rect.y() * f64::from(factor)).round() as i32 - top as i32;
            let paint = PixmapPaint {
                opacity: opacity as f32,
                ..PixmapPaint::default()
            };
            pixmap.draw_pixmap(
                x,
                y,
                rendered.pixmap.as_ref(),
                &paint,
                Transform::identity(),
                None,
            );
        })?;
        // frame callbacks pace the animation
        if animating {
            self.client.request_redraw(self.surface)?;
        }
        Ok(())
    }

    /// Make the catcher take the input of the whole output; it is drawn once and stays transparent.
    fn draw_catcher(&mut self, catcher: SurfaceId) -> Result<()> {
        let (width, height) = self.client.size(catcher)?;
        let region =
            Rect::from_origin_size(Point::ZERO, Size::new(f64::from(width), f64::from(height)));
        self.client.set_input_region(catcher, &[region])?;
        self.client.draw(catcher, |_, _| {})?;
        Ok(())
    }

    /// Hover selects the row under the pointer; a press on a row activates it, a press elsewhere
    /// in the panel is absorbed and a press outside it closes the panel.
    fn pointer(&mut self, event: PointerEvent) -> Result<()> {
        let Some((layout, rect)) = self.last.as_ref().filter(|_| self.panel.is_visible()) else {
            return Ok(());
        };
        // whether a point is in the panel, and the result row under it
        let probe = |p: Point| {
            (
                rect.contains(p),
                layout.item_at(Point::new(p.x - rect.x(), p.y - rect.y())),
            )
        };
        let area = self
            .renderer
            .field_area(layout)
            .translate(rect.x(), rect.y());
        match event {
            PointerEvent::Motion(p) if self.panel.field().is_dragging() => {
                let index = self.panel.field().index_at(
                    &self.field_offsets,
                    p.x - area.x() + self.panel.field().scroll(),
                );
                let now = self.now();
                self.panel.edit(&self.launcher, now, |f| f.drag_to(index));
                self.client.request_redraw(self.surface)?;
            }
            PointerEvent::Button {
                button: Button::Left,
                pressed: false,
                ..
            } if self.panel.field().is_dragging() => {
                let now = self.now();
                self.panel.edit(&self.launcher, now, TextField::release);
            }
            PointerEvent::Button {
                button: Button::Left,
                pressed: true,
                position,
            } if area.contains(position) => {
                let index = self.panel.field().index_at(
                    &self.field_offsets,
                    position.x - area.x() + self.panel.field().scroll(),
                );
                let extend = self.client.modifiers().shift;
                let now = self.now();
                self.panel.focus_field();
                self.panel.edit(&self.launcher, now, |f| {
                    f.press(index, extend, Instant::now())
                });
                self.client.request_redraw(self.surface)?;
            }
            PointerEvent::Enter(p) | PointerEvent::Motion(p) => {
                if let (true, Some(flat)) = probe(p) {
                    if self.panel.set_selected(flat) == PanelEvent::Changed {
                        self.client.request_redraw(self.surface)?;
                    }
                }
            }
            PointerEvent::Button {
                button: Button::Left,
                pressed: true,
                position,
            } => match probe(position) {
                (false, _) => self.hide()?,
                (true, Some(flat)) => {
                    let now = self.now();
                    let outcome = self.panel.activate_index(flat, &mut self.launcher, now);
                    self.outcome(outcome)?;
                }
                (true, None) => {}
            },
            PointerEvent::Button { pressed: true, .. } => self.hide()?,
            PointerEvent::Leave
            | PointerEvent::Button { pressed: false, .. }
            | PointerEvent::Scroll { .. } => {}
        }
        Ok(())
    }
}

/// The x of `field`'s caret among `offsets` (one per [`TextField::boundaries`]).
fn caret_x(field: &TextField, offsets: &[f64]) -> f64 {
    field
        .boundaries()
        .iter()
        .position(|b| *b == field.caret())
        .and_then(|i| offsets.get(i))
        .copied()
        .unwrap_or(0.0)
}
