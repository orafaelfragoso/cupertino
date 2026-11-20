//! One capture from a shortcut: `cupertino-screenshot --mode screen|selection|toolbar [--clipboard]`, a program of its
//! own that the server starts for a screenshot shortcut on a connection with the capture role (`cupertino_kit_shell::run`).
//! `--mode screen` captures every output at once; `--mode selection` and `--mode toolbar` put an overlay on every
//! output (and the toolbar) and run the [`Controller`] until it captures or is cancelled. The windows reach the screen
//! as layer surfaces ([`Shot::surface`]); the capture is `cupertino_kit_shell::capture`'s, asked for once the overlays
//! are gone, and the images go to files on the I/O pool ([`crate::files`]) or to the clipboard through Kit.

use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use cupertino_geometry::{Rect, Size};
use cupertino_kit::core::{Application, Command, RendererClass, Window, WindowKey};
use cupertino_kit::platform::ClipboardData;
use cupertino_kit::render::ImageResource;
use cupertino_kit::task;
use cupertino_kit_shell::capture::{self, Source};
use cupertino_kit_shell::outputs::{self, OutputId};
use cupertino_kit_shell::{
    Anchor, Event, Keyboard, Layer, LayerSpec, Margin, Program, Role, Surface,
};

use crate::controller::{Controller, Effect};
use crate::files::{self, SETTINGS_DEADLINE, WRITE_DEADLINE};
use crate::pick::PickWindow;
use crate::session::{Style, Target};
use crate::settings::{Problem, Settings};
use crate::values::{THUMBNAIL_MARGIN, THUMBNAIL_SHADOW_OUTSET, TOOLBAR_BOTTOM, TOOLBAR_SIZE};
use crate::view::{self, Input, Kind};

/// Layer namespace of the overlays and the toolbar: the server starts no second capture while one is shown.
pub const OVERLAY_NAMESPACE: &str = "cupertino-screenshot";
/// Namespaces of the thumbnail and of the pixel that takes the keyboard focus the clipboard needs. Neither is the
/// overlay's, so a new shortcut works while they are up.
pub const THUMBNAIL_NAMESPACE: &str = "cupertino-screenshot-thumbnail";
pub const CLIPBOARD_NAMESPACE: &str = "cupertino-screenshot-clipboard";
/// The type the clipboard is offered in.
pub const PNG: &str = "image/png";

/// How the process was started (the server appends these per shortcut).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartMode {
    /// Capture every output at once.
    Screen,
    /// The crosshair selection.
    Selection,
    /// The toolbar.
    Toolbar,
}

/// The command line of a capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub mode: StartMode,
    /// Send the capture to the clipboard instead of a file.
    pub clipboard: bool,
}

impl Options {
    /// Parse the arguments; the problem is returned as the error.
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut mode = None;
        let mut clipboard = false;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--mode" => {
                    let value = args
                        .next()
                        .ok_or("--mode needs screen, selection or toolbar")?;
                    mode = Some(match value.as_str() {
                        "screen" => StartMode::Screen,
                        "selection" => StartMode::Selection,
                        "toolbar" => StartMode::Toolbar,
                        other => {
                            return Err(format!("{other:?} is not screen, selection or toolbar"))
                        }
                    });
                }
                "--clipboard" => clipboard = true,
                _ => return Err(format!("unknown argument {arg}")),
            }
        }
        Ok(Options {
            mode: mode.ok_or("--mode is required")?,
            clipboard,
        })
    }

    fn style(self) -> Option<Style> {
        match self.mode {
            StartMode::Screen => None,
            StartMode::Selection => Some(Style::Simple),
            StartMode::Toolbar => Some(Style::Interactive),
        }
    }
}

/// The options of the run [`run`] starts, taken by [`Shot::init`].
static START: Mutex<Option<Options>> = Mutex::new(None);
/// How the run ended, for [`run`] to return.
static OUTCOME: Mutex<Option<Result<(), String>>> = Mutex::new(None);

/// Run one capture with `options` on the connection the server started the process with, until it is delivered,
/// cancelled or fails.
pub fn run(options: Options) -> Result<(), String> {
    *START.lock().unwrap_or_else(PoisonError::into_inner) = Some(options);
    OUTCOME
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    let environment = cupertino_kit::app::environment().map_err(|e| e.to_string())?;
    cupertino_kit_shell::run::<Shot>(environment).map_err(|e| e.to_string())?;
    OUTCOME
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .unwrap_or_else(|| Err("the screenshot's windows went away".into()))
}

/// An output the overlay covers.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Screen {
    id: OutputId,
    rect: Rect,
}

/// The server's outputs that have a place in the layout.
fn screens() -> Vec<Screen> {
    outputs::all()
        .into_iter()
        .filter_map(|o| o.geometry.map(|rect| Screen { id: o.id, rect }))
        .collect()
}

/// One capture.
pub struct Shot;

pub struct Model {
    start: Instant,
    options: Options,
    screens: Vec<Screen>,
    settings_path: PathBuf,
    /// Made once the settings are read.
    controller: Option<Controller>,
    /// The timer that counts; one scheduled before it is ignored when it fires.
    timer: u64,
    /// The frames of the capture under way, in the order of its sources.
    frames: Vec<Option<ImageResource>>,
    /// Writes of the captures under way.
    writing: usize,
    /// How the run ends once they are written.
    ending: Option<Result<(), String>>,
}

#[derive(Clone, Debug)]
pub enum Message {
    Settings(Result<(Settings, Vec<Problem>), String>),
    Stored(Result<(), String>),
    Input(Input),
    Timer(u64),
    /// The server's outputs changed.
    Outputs,
    /// The windows on screen, front to back.
    Windows(Vec<PickWindow>),
    /// A frame of the capture, by its place among the capture's sources.
    Frame(usize, Result<ImageResource, String>),
    /// The capture's images, and the thumbnail's picture of the first.
    Captured(Vec<ImageResource>, Option<ImageResource>),
    Written(Result<Vec<PathBuf>, String>),
    /// The PNG the clipboard is offered.
    Png(Result<Vec<u8>, String>),
    /// Another selection replaced the offer.
    Replaced,
    Quit,
}

impl Model {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// End the run with `result` once the captures under way are written; the windows go first.
    fn end(&mut self, result: Result<(), String>) -> Vec<Command<Message>> {
        let result = match self.ending.take() {
            // the first failure is the run's
            Some(Err(e)) => Err(e),
            _ => result,
        };
        if self.writing > 0 && result.is_ok() {
            self.ending = Some(result);
            return Vec::new();
        }
        OUTCOME
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert(result);
        self.controller = None;
        vec![Command::Perform(Box::pin(async { Message::Quit }))]
    }

    /// The sources of a capture of `target`, with the region of each in its output's points.
    fn sources(&self, target: Target, pointer: bool) -> Vec<(Source, capture::Options)> {
        let options = |region| capture::Options {
            cursor: pointer,
            region,
        };
        let output = |i: usize| self.screens.get(i).map(|s| Source::Output(s.id));
        match target {
            Target::Screens => self
                .screens
                .iter()
                .map(|s| (Source::Output(s.id), options(None)))
                .collect(),
            Target::Output(i) => output(i).map(|s| (s, options(None))).into_iter().collect(),
            Target::Area { output: i, rect } => {
                let Some(screen) = self.screens.get(i) else {
                    return Vec::new();
                };
                let local = Rect::new(
                    rect.x() - screen.rect.x(),
                    rect.y() - screen.rect.y(),
                    rect.width(),
                    rect.height(),
                );
                vec![(Source::Output(screen.id), options(Some(local)))]
            }
            Target::Window(identifier) => vec![(Source::Window(identifier), options(None))],
        }
    }

    /// Carry out the controller's effects.
    fn effects(&mut self, effects: Vec<Effect>) -> Vec<Command<Message>> {
        let mut commands = Vec::new();
        for effect in effects {
            let Some(controller) = &self.controller else {
                break;
            };
            match effect {
                Effect::StoreSettings => {
                    let (path, settings) =
                        (self.settings_path.clone(), controller.settings().clone());
                    commands.push(task::io(
                        SETTINGS_DEADLINE,
                        move || {
                            files::store_settings(&path, &settings)
                                .map_err(|e| format!("{}: {e}", path.display()))
                        },
                        Message::Stored,
                    ));
                }
                Effect::Capture { target, pointer } => {
                    let sources = self.sources(target, pointer);
                    if sources.is_empty() {
                        let effects = self
                            .controller
                            .as_mut()
                            .map(|c| c.shots(Err("the capture's output is gone".into()), None, 0.0))
                            .unwrap_or_default();
                        commands.extend(self.effects(effects));
                        continue;
                    }
                    self.frames = vec![None; sources.len()];
                    // asked after this update's view, so the requests follow the overlays' destruction
                    commands.extend(sources.into_iter().enumerate().map(
                        |(i, (source, options))| {
                            capture::once(source, options, move |frame| {
                                Message::Frame(i, frame.map_err(|e| e.to_string()))
                            })
                        },
                    ));
                }
                Effect::Write => {
                    self.writing += 1;
                    let location = controller.settings().location.clone();
                    let images = controller.images().to_vec();
                    commands.push(task::io(
                        WRITE_DEADLINE,
                        move || files::write_captures(&location, &images),
                        Message::Written,
                    ));
                }
                Effect::Offer => {
                    let Some(first) = controller.images().first().cloned() else {
                        continue;
                    };
                    commands.push(task::cpu(
                        move || cupertino_kit::image::encode_png(&first).map_err(|e| e.to_string()),
                        Message::Png,
                    ));
                }
                Effect::Exit(result) => {
                    commands.extend(self.end(result));
                    break;
                }
            }
        }
        commands
    }

    /// The timer for the controller's next deadline.
    fn schedule(&mut self) -> Vec<Command<Message>> {
        self.timer += 1;
        let now = self.now();
        let Some(at) = self.controller.as_ref().and_then(|c| c.deadline(now)) else {
            return Vec::new();
        };
        vec![Command::After {
            delay: Duration::from_secs_f64((at - now).max(0.0)),
            message: Message::Timer(self.timer),
        }]
    }

    /// The settings were read: the run starts.
    fn started(&mut self, settings: Settings) -> Vec<Command<Message>> {
        let style = self.options.style();
        let rects = self.screens.iter().map(|s| s.rect).collect();
        let (controller, effects) = Controller::new(style, settings, rects, self.options.clipboard);
        self.controller = Some(controller);
        if style.is_some() {
            // the window pick follows the windows on screen
            capture::windows();
        }
        self.effects(effects)
    }

    /// A frame of the capture came: once every frame is there, the thumbnail's picture of the first is made.
    fn frame(
        &mut self,
        index: usize,
        frame: Result<ImageResource, String>,
    ) -> Vec<Command<Message>> {
        let now = self.now();
        let image = match frame {
            Ok(image) => image,
            Err(e) => {
                self.frames.clear();
                let effects = match self.controller.as_mut() {
                    Some(c) => c.shots(Err(e), None, now),
                    None => Vec::new(),
                };
                return self.effects(effects);
            }
        };
        let Some(slot) = self.frames.get_mut(index) else {
            return Vec::new();
        };
        *slot = Some(image);
        if self.frames.iter().any(Option::is_none) {
            return Vec::new();
        }
        let images: Vec<ImageResource> = std::mem::take(&mut self.frames)
            .into_iter()
            .flatten()
            .collect();
        let Some(controller) = &self.controller else {
            return Vec::new();
        };
        let first = images[0].clone();
        let small = controller
            .wants_thumbnail()
            .then(|| controller.thumbnail_pixels((first.width, first.height)));
        vec![task::cpu(
            move || {
                let small =
                    small.and_then(|(w, h)| cupertino_kit::image::resize(&first, w, h).ok());
                (images, small)
            },
            |(images, small)| Message::Captured(images, small),
        )]
    }
}

impl Application for Shot {
    type Model = Model;
    type Message = Message;
    const RESIDENT: bool = true;
    /// The toolbar is small and the selection overlay is shown for a moment, mostly at rest: a GPU context would only
    /// cost memory.
    const RENDERER: RendererClass = RendererClass::Cpu;

    fn init() -> (Model, Vec<Command<Message>>) {
        let options = START.lock().unwrap_or_else(PoisonError::into_inner).take();
        let settings_path =
            cupertino_defaults::config_file("CUPERTINO_SCREENSHOT_CONFIG", "screenshot.conf");
        let mut model = Model {
            start: Instant::now(),
            options: options.unwrap_or(Options {
                mode: StartMode::Screen,
                clipboard: false,
            }),
            screens: screens(),
            settings_path: settings_path.clone().unwrap_or_default(),
            controller: None,
            timer: 0,
            frames: Vec::new(),
            writing: 0,
            ending: None,
        };
        let commands = match (options, settings_path) {
            (None, _) => model.end(Err("the screenshot was started without its options".into())),
            _ if model.screens.is_empty() => model.end(Err("the server has no output".into())),
            (_, None) => model.end(Err("neither XDG_CONFIG_HOME nor HOME is set".into())),
            (Some(_), Some(path)) => vec![task::io(
                SETTINGS_DEADLINE,
                move || files::read_settings(&path).map_err(|e| format!("{}: {e}", path.display())),
                Message::Settings,
            )],
        };
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        let now = model.now();
        let mut commands = match message {
            Message::Settings(Ok((settings, problems))) => {
                for p in problems {
                    eprintln!(
                        "cupertino-screenshot: {}: {p}",
                        model.settings_path.display()
                    );
                }
                model.started(settings)
            }
            Message::Settings(Err(e)) | Message::Stored(Err(e)) => model.end(Err(e)),
            Message::Stored(Ok(())) => Vec::new(),
            Message::Input(input) => {
                let effects = model
                    .controller
                    .as_mut()
                    .map(|c| c.input(input, now))
                    .unwrap_or_default();
                model.effects(effects)
            }
            Message::Timer(timer) if timer == model.timer => {
                let effects = model
                    .controller
                    .as_mut()
                    .map(|c| c.tick(now))
                    .unwrap_or_default();
                model.effects(effects)
            }
            Message::Timer(_) => return Vec::new(),
            Message::Outputs => {
                if screens() == model.screens {
                    return Vec::new();
                }
                model.end(Err("the outputs changed during the screenshot".into()))
            }
            Message::Windows(windows) => {
                if let Some(c) = model.controller.as_mut() {
                    c.set_windows(windows);
                }
                return Vec::new();
            }
            Message::Frame(index, frame) => model.frame(index, frame),
            Message::Captured(images, small) => {
                let effects = model
                    .controller
                    .as_mut()
                    .map(|c| c.shots(Ok(images), small, now))
                    .unwrap_or_default();
                model.effects(effects)
            }
            Message::Written(written) => {
                model.writing = model.writing.saturating_sub(1);
                match written {
                    Ok(paths) => {
                        for path in paths {
                            println!("{}", path.display());
                        }
                        match model.ending.take() {
                            Some(result) if model.writing == 0 => model.end(result),
                            ending => {
                                model.ending = ending;
                                Vec::new()
                            }
                        }
                    }
                    Err(e) => {
                        model.writing = 0;
                        model.end(Err(e))
                    }
                }
            }
            Message::Png(Ok(bytes)) => {
                if let Some(c) = model.controller.as_mut() {
                    c.offered();
                }
                vec![Command::SetClipboardData {
                    data: vec![ClipboardData {
                        mime: PNG.to_owned(),
                        bytes,
                    }],
                    replaced: Some(Message::Replaced),
                }]
            }
            Message::Png(Err(e)) => model.end(Err(e)),
            Message::Replaced => {
                let effects = model
                    .controller
                    .as_mut()
                    .map(|c| c.clipboard_done(Ok(())))
                    .unwrap_or_default();
                model.effects(effects)
            }
            Message::Quit => return vec![Command::Exit],
        };
        commands.extend(model.schedule());
        commands
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        model
            .controller
            .as_ref()
            .map(|c| c.windows(model.now(), Message::Input))
            .unwrap_or_default()
    }
}

impl Program for Shot {
    /// An overlay on each output, over everything and the exclusive zones; the toolbar at the bottom of the first
    /// output, above the dock's zone; the thumbnail in its bottom-right corner; the clipboard pixel. The direct
    /// shortcut's first overlay takes the keyboard, otherwise the toolbar does, and the clipboard pixel does while the
    /// offer is made; the thumbnail never does. The Options menu is Kit's popup of the toolbar.
    fn surface(model: &Model, key: &WindowKey) -> Surface {
        let first = model.screens.first().map(|s| s.id);
        let layer = |spec: LayerSpec| Surface {
            role: Role::Layer(spec),
            persistent: None,
            holes: Vec::new(),
        };
        let all = Anchor {
            top: true,
            bottom: true,
            left: true,
            right: true,
        };
        match view::kind(key) {
            Some(Kind::Overlay(index)) => {
                let keys = model.options.style() == Some(Style::Simple) && index == 0;
                layer(LayerSpec {
                    namespace: OVERLAY_NAMESPACE,
                    output: model.screens.get(index).map(|s| s.id),
                    layer: Layer::Overlay,
                    anchor: all,
                    size: Size::ZERO,
                    margin: Margin::default(),
                    exclusive_zone: -1,
                    keyboard: if keys {
                        Keyboard::Exclusive
                    } else {
                        Keyboard::None
                    },
                })
            }
            // made after the overlays, so it is stacked above them
            Some(Kind::Toolbar) => layer(LayerSpec {
                namespace: OVERLAY_NAMESPACE,
                output: first,
                layer: Layer::Overlay,
                anchor: Anchor {
                    bottom: true,
                    ..Anchor::default()
                },
                size: Size::new(TOOLBAR_SIZE.0, TOOLBAR_SIZE.1),
                margin: Margin {
                    bottom: TOOLBAR_BOTTOM,
                    ..Margin::default()
                },
                exclusive_zone: 0,
                keyboard: Keyboard::Exclusive,
            }),
            // the picture sits THUMBNAIL_MARGIN inside the corner; its surface carries the shadow's room around it, so
            // the margins are smaller by that much
            Some(Kind::Thumbnail) => {
                let margin = (THUMBNAIL_MARGIN - THUMBNAIL_SHADOW_OUTSET) as i32;
                layer(LayerSpec {
                    namespace: THUMBNAIL_NAMESPACE,
                    output: first,
                    layer: Layer::Overlay,
                    anchor: Anchor {
                        bottom: true,
                        right: true,
                        ..Anchor::default()
                    },
                    size: model
                        .controller
                        .as_ref()
                        .and_then(Controller::thumbnail_size)
                        .unwrap_or(Size::ZERO),
                    margin: Margin {
                        right: margin,
                        bottom: margin,
                        ..Margin::default()
                    },
                    exclusive_zone: 0,
                    keyboard: Keyboard::None,
                })
            }
            Some(Kind::Clipboard) => layer(LayerSpec {
                namespace: CLIPBOARD_NAMESPACE,
                output: None,
                layer: Layer::Overlay,
                anchor: Anchor {
                    top: true,
                    left: true,
                    ..Anchor::default()
                },
                size: Size::new(1.0, 1.0),
                margin: Margin::default(),
                exclusive_zone: -1,
                keyboard: Keyboard::Exclusive,
            }),
            Some(Kind::OptionsMenu) | None => Surface {
                role: Role::Window,
                persistent: None,
                holes: Vec::new(),
            },
        }
    }

    fn event(event: Event) -> Option<Message> {
        match event {
            Event::Outputs => Some(Message::Outputs),
            // the server took one of the tool's surfaces away
            Event::Closed(_) => Some(Message::Input(Input::Closed)),
            Event::Capture(capture::Event::Windows(windows)) => Some(Message::Windows(
                windows
                    .into_iter()
                    .map(|w| PickWindow {
                        identifier: w.identifier,
                        frame: w.frame,
                    })
                    .collect(),
            )),
            _ => None,
        }
    }
}
